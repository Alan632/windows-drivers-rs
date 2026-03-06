// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Report types and rendering for driver smoketest results.
//!
//! This module defines the structured output produced at the end of a
//! smoketest pipeline run, including both machine-readable JSON and a
//! human-readable interactive summary.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;

/// Outcome of a single pipeline step.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", content = "message")]
pub enum StepStatus {
    Pass,
    Fail(String),
    Skipped,
}

/// Result captured for one pipeline step (snapshot, install, verify, …).
#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub name: String,
    pub status: StepStatus,
    pub exit_code: Option<i32>,
    #[serde(serialize_with = "serialize_duration")]
    pub duration: Duration,
    pub details: HashMap<String, String>,
    /// When `true`, this step failure represents an infrastructure problem
    /// (VM unreachable, timeout, PS Direct error) rather than a test-logic failure.
    #[serde(default)]
    pub is_infra_failure: bool,
}

/// Metadata about the driver under test.
#[derive(Debug, Clone, Serialize)]
pub struct DriverInfo {
    pub inf: String,
    pub published_name: Option<String>,
    pub version: Option<String>,
    pub provider: Option<String>,
    pub hardware_id: Option<String>,
    pub instance_id: Option<String>,
}

/// Top-level report for a complete smoketest run.
#[derive(Debug, Clone, Serialize)]
pub struct SmoketestReport {
    pub success: bool,
    pub vm_name: String,
    pub driver: DriverInfo,
    pub steps: Vec<StepResult>,
    #[serde(serialize_with = "serialize_duration")]
    pub duration: Duration,
}

// ---------------------------------------------------------------------------
// Duration serialization helper (seconds as f64)
// ---------------------------------------------------------------------------

fn serialize_duration<S: serde::Serializer>(dur: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(dur.as_secs_f64())
}

// ---------------------------------------------------------------------------
// SmoketestReport implementation
// ---------------------------------------------------------------------------

impl SmoketestReport {
    /// Returns `true` if any step failure is classified as an infrastructure
    /// error (VM unreachable, timeout, PS Direct failure, etc.).
    pub fn has_infra_failure(&self) -> bool {
        self.steps
            .iter()
            .any(|s| s.is_infra_failure && matches!(s.status, StepStatus::Fail(_)))
    }

    /// Returns `true` when every non-skipped step passed.
    pub fn is_success(&self) -> bool {
        self.steps.iter().all(|step| {
            matches!(step.status, StepStatus::Pass | StepStatus::Skipped)
        })
    }

    /// Serialize the report to pretty-printed JSON.
    ///
    /// Returns a JSON string, or a JSON error object if serialization fails.
    pub fn to_json(&self) -> String {
        self.try_to_json().unwrap_or_else(|e| {
            format!("{{\"error\": \"Failed to serialize report: {}\"}}", e)
        })
    }

    /// Serialize the report to pretty-printed JSON, returning any
    /// serialization error to the caller.
    pub fn try_to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Print a human-readable summary suitable for interactive terminals.
    ///
    /// ```text
    /// driver-smoketest v0.1.0
    ///   Package:  sample_kmdf_driver
    ///   VM:       driver-test-vm (Running)
    ///   Driver:   sample_kmdf_driver v1.0.0
    ///
    /// [1/4] Reverting to baseline snapshot... done (smoketest-baseline)
    /// [2/4] Installing driver package... done (oem3.inf)
    /// ...
    ///
    /// PASS  sample_kmdf_driver verified on driver-test-vm (47s)
    /// ```
    pub fn print_interactive(&self) {
        let version = env!("CARGO_PKG_VERSION");
        let driver_display = self
            .driver
            .published_name
            .as_deref()
            .unwrap_or(&self.driver.inf);
        let driver_version = self.driver.version.as_deref().unwrap_or("unknown");

        // Header block
        println!("driver-smoketest v{version}");
        println!("  Package:  {}", self.driver.inf);
        println!("  VM:       {}", self.vm_name);
        println!("  Driver:   {driver_display} v{driver_version}");
        println!();

        // Step results
        let total = self.steps.len();
        for (i, step) in self.steps.iter().enumerate() {
            let idx = i + 1;
            let (outcome, detail) = match &step.status {
                StepStatus::Pass => {
                    let detail = step_detail(step);
                    ("done", detail)
                }
                StepStatus::Fail(msg) => ("FAILED", msg.clone()),
                StepStatus::Skipped => ("skipped", String::new()),
            };

            if detail.is_empty() {
                println!("[{idx}/{total}] {}... {outcome}", step.name);
            } else {
                println!("[{idx}/{total}] {}... {outcome} ({detail})", step.name);
            }
        }
        println!();

        // Summary line
        let secs = self.duration.as_secs();
        if self.is_success() {
            println!(
                "PASS  {driver_display} verified on {} ({secs}s)",
                self.vm_name
            );
        } else {
            let failed: Vec<&str> = self
                .steps
                .iter()
                .filter(|s| matches!(s.status, StepStatus::Fail(_)))
                .map(|s| s.name.as_str())
                .collect();
            println!(
                "FAIL  {driver_display} on {} ({secs}s) — failed step(s): {}",
                self.vm_name,
                failed.join(", ")
            );
        }
    }
}

/// Extract a short detail string from a step's `details` map.
fn step_detail(step: &StepResult) -> String {
    // Prefer well-known keys in display order.
    for key in &["snapshot_name", "published_name", "device_status", "log_path"] {
        if let Some(val) = step.details.get(*key) {
            return val.clone();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report(fail: bool) -> SmoketestReport {
        let steps = vec![
            StepResult {
                name: "Reverting to baseline snapshot".into(),
                status: StepStatus::Pass,
                exit_code: Some(0),
                duration: Duration::from_secs(5),
                details: HashMap::from([("snapshot_name".into(), "smoketest-baseline".into())]),
                is_infra_failure: false,
            },
            StepResult {
                name: "Installing driver package".into(),
                status: if fail {
                    StepStatus::Fail("pnputil returned 1".into())
                } else {
                    StepStatus::Pass
                },
                exit_code: Some(if fail { 1 } else { 0 }),
                duration: Duration::from_secs(12),
                details: HashMap::from([("published_name".into(), "oem3.inf".into())]),
                is_infra_failure: false,
            },
        ];

        let success = steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Pass | StepStatus::Skipped));

        SmoketestReport {
            success,
            vm_name: "driver-test-vm".into(),
            driver: DriverInfo {
                inf: "sample_kmdf_driver.inf".into(),
                published_name: Some("oem3.inf".into()),
                version: Some("1.0.0".into()),
                provider: Some("Contoso".into()),
                hardware_id: None,
                instance_id: None,
            },
            steps,
            duration: Duration::from_secs(47),
        }
    }

    #[test]
    fn is_success_all_pass() {
        let r = sample_report(false);
        assert!(r.is_success());
    }

    #[test]
    fn is_success_with_failure() {
        let r = sample_report(true);
        assert!(!r.is_success());
    }

    #[test]
    fn to_json_roundtrip() {
        let r = sample_report(false);
        let json = r.to_json();
        assert!(json.contains("\"success\": true"));
        assert!(json.contains("driver-test-vm"));
    }

    #[test]
    fn skipped_does_not_fail() {
        let r = SmoketestReport {
            success: true,
            vm_name: "vm".into(),
            driver: DriverInfo {
                inf: "test.inf".into(),
                published_name: None,
                version: None,
                provider: None,
                hardware_id: None,
                instance_id: None,
            },
            steps: vec![StepResult {
                name: "optional step".into(),
                status: StepStatus::Skipped,
                exit_code: None,
                duration: Duration::ZERO,
                details: HashMap::new(),
                is_infra_failure: false,
            }],
            duration: Duration::from_secs(1),
        };
        assert!(r.is_success());
    }
}
