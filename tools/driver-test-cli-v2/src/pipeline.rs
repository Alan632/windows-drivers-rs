// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Pipeline orchestrator for driver smoketest runs.
//!
//! [`Pipeline`] drives a sequence of PowerShell-backed steps (driver install,
//! verification, trace capture) and collects results into a
//! [`SmoketestReport`].

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::info;

use crate::capture::{self, CaptureConfig, CaptureSession};
use crate::error::{Result, SmoketestError};
use crate::ps_runner::{PsRunner, VmCredential};
use crate::report::{DriverInfo, SmoketestReport, StepResult, StepStatus};

// ---------------------------------------------------------------------------
// PipelineConfig
// ---------------------------------------------------------------------------

/// Configuration for a single smoketest pipeline run.
pub struct PipelineConfig {
    /// Path to the driver package directory (must contain .inf file).
    pub driver_package: PathBuf,
    /// Hyper-V VM name to target.
    pub vm_name: String,
    /// Optional credentials for PS Direct VM connections.
    pub credential: Option<VmCredential>,
    /// Skip the driver verification step.
    pub skip_verify: bool,
    /// Skip the trace capture step.
    pub skip_capture: bool,
    /// Duration in seconds for ETW trace capture.
    pub capture_duration: u32,
    /// If true, do not clean up temporary resources on completion.
    pub no_cleanup: bool,
    /// Host directory where captured log files will be placed.
    pub output_dir: PathBuf,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            driver_package: PathBuf::new(),
            vm_name: String::new(),
            credential: None,
            skip_verify: false,
            skip_capture: false,
            capture_duration: 30,
            no_cleanup: false,
            output_dir: std::env::temp_dir().join("driver-test-logs"),
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Orchestrates the smoketest pipeline steps in order and produces a report.
pub struct Pipeline {
    // NOTE: PsRunner does not derive Debug, so we use a manual impl below.
    config: PipelineConfig,
    /// Wrapped in `ManuallyDrop` so we can honour the `no_cleanup` flag.
    /// See the `Drop` impl for `Pipeline` below.
    runner: ManuallyDrop<PsRunner>,
    driver_name: String,
    /// Active capture session, set by `step_start_capture` and consumed by
    /// `step_stop_capture`.
    capture_session: Option<CaptureSession>,
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("driver_name", &self.driver_name)
            .field("vm_name", &self.config.vm_name)
            .finish_non_exhaustive()
    }
}

impl Pipeline {
    /// Create a new pipeline, validating the driver package layout.
    ///
    /// The driver package directory must exist and contain at least one `.inf`
    /// and one `.inf` file. The driver name is extracted from the first `.inf`
    /// filename found.
    pub fn new(config: PipelineConfig) -> Result<Self> {
        // Validate directory exists
        if !config.driver_package.is_dir() {
            return Err(SmoketestError::PackageValidation {
                reason: format!(
                    "driver package directory does not exist: {}",
                    config.driver_package.display()
                ),
            });
        }

        // Scan for .inf files
        let entries: Vec<_> = std::fs::read_dir(&config.driver_package)
            .map_err(|e| SmoketestError::PackageValidation {
                reason: format!(
                    "cannot read driver package directory {}: {e}",
                    config.driver_package.display()
                ),
            })?
            .filter_map(|e| e.ok())
            .collect();

        let inf_file = entries.iter().find(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("inf"))
        });

        let inf_entry = inf_file.ok_or_else(|| SmoketestError::PackageValidation {
            reason: format!(
                "no .inf file found in driver package: {}",
                config.driver_package.display()
            ),
        })?;

        // Extract driver name from the .inf filename (stem)
        let driver_name = inf_entry
            .path()
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Create the PsRunner with a unique temp scripts directory.
        // Append PID and time-based suffix to avoid conflicts between concurrent
        // instances.
        let random_suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let scripts_dir = std::env::temp_dir().join(format!(
            "driver-test-cli-v2-scripts-{}-{:x}",
            std::process::id(),
            random_suffix
        ));
        let runner = PsRunner::new(scripts_dir).map_err(|e| match e {
            crate::ps_runner::PsRunnerError::Io(io_err) => SmoketestError::Io(io_err),
            other => SmoketestError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                other.to_string(),
            )),
        })?;

        Ok(Self {
            config,
            runner: ManuallyDrop::new(runner),
            driver_name,
            capture_session: None,
        })
    }

    /// Execute all pipeline steps and return a [`SmoketestReport`].
    pub fn run(&mut self) -> SmoketestReport {
        let pipeline_start = Instant::now();
        let mut steps = Vec::new();

        let step_names = [
            "Starting log capture",
            "Installing driver package",
            "Verifying driver on VM",
            "Collecting captured logs",
        ];

        // Execute steps in order, stopping on first failure.
        loop {
            // 1. Start logging (before install)
            if self.config.skip_capture {
                steps.push(StepResult {
                    name: "Starting log capture".into(),
                    status: StepStatus::Skipped,
                    exit_code: None,
                    duration: Duration::ZERO,
                    details: HashMap::new(),
                    is_infra_failure: false,
                });
            } else {
                steps.push(self.step_start_capture());
            }
            if matches!(steps.last().unwrap().status, StepStatus::Fail(_)) {
                break;
            }

            // 2. Driver install (always)
            steps.push(self.step_install());
            if matches!(steps.last().unwrap().status, StepStatus::Fail(_)) {
                break;
            }

            // 3. Driver verify
            if self.config.skip_verify {
                steps.push(StepResult {
                    name: "Verifying driver on VM".into(),
                    status: StepStatus::Skipped,
                    exit_code: None,
                    duration: Duration::ZERO,
                    details: HashMap::new(),
                    is_infra_failure: false,
                });
            } else {
                steps.push(self.step_verify());
            }
            if matches!(steps.last().unwrap().status, StepStatus::Fail(_)) {
                break;
            }

            // 4. Wait for capture duration, stop + extract
            if self.config.skip_capture {
                steps.push(StepResult {
                    name: "Collecting captured logs".into(),
                    status: StepStatus::Skipped,
                    exit_code: None,
                    duration: Duration::ZERO,
                    details: HashMap::new(),
                    is_infra_failure: false,
                });
            } else {
                capture::wait_capture(self.config.capture_duration);
                steps.push(self.step_stop_capture());
            }

            break; // all steps completed
        }

        // Mark remaining steps as skipped if a failure short-circuited the
        // pipeline.
        if steps.len() < step_names.len() {
            let fail_reason = steps.last().map_or_else(
                || "unknown failure".to_string(),
                |s| {
                    if let StepStatus::Fail(ref msg) = s.status {
                        format!("skipped due to prior failure: {msg}")
                    } else {
                        "skipped".to_string()
                    }
                },
            );
            for name in &step_names[steps.len()..] {
                steps.push(StepResult {
                    name: (*name).into(),
                    status: StepStatus::Skipped,
                    exit_code: None,
                    duration: Duration::ZERO,
                    details: HashMap::from([("reason".into(), fail_reason.clone())]),
                    is_infra_failure: false,
                });
            }
        }

        let total_duration = pipeline_start.elapsed();

        // Populate driver info from step details
        let mut driver_info = DriverInfo {
            inf: format!("{}.inf", self.driver_name),
            published_name: None,
            version: None,
            provider: None,
            hardware_id: None,
            instance_id: None,
        };
        for step in &steps {
            if let Some(name) = step.details.get("published_name") {
                driver_info.published_name = Some(name.clone());
            }
            if let Some(stdout) = step.details.get("stdout") {
                if let Some(v) = extract_detail(stdout, "Version") {
                    driver_info.version = Some(v);
                }
                if let Some(p) = extract_detail(stdout, "Provider") {
                    driver_info.provider = Some(p);
                }
                if let Some(h) = extract_detail(stdout, "Hardware ID") {
                    driver_info.hardware_id = Some(h);
                }
                if let Some(i) = extract_detail(stdout, "Instance ID") {
                    driver_info.instance_id = Some(i);
                }
            }
        }

        let success = steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Pass | StepStatus::Skipped));

        SmoketestReport {
            success,
            vm_name: self.config.vm_name.clone(),
            driver: driver_info,
            steps,
            duration: total_duration,
        }
    }

    // ── Private step methods ─────────────────────────────────────────

    /// Install the driver package on the VM.
    fn step_install(&self) -> StepResult {
        info!(
            vm = %self.config.vm_name,
            driver = %self.driver_name,
            "installing driver package"
        );

        let start = Instant::now();
        let args = vec![
            "-VMName".into(),
            self.config.vm_name.clone(),
            "-DriverPath".into(),
            self.config.driver_package.display().to_string(),
        ];

        let timeout = Duration::from_secs(120);
        let result = self
            .runner
            .run_script("Install-DriverOnVM.ps1", &args, timeout, self.credential(), false);

        let duration = start.elapsed();

        match result {
            Ok(sr) if sr.success() => {
                let mut details = HashMap::new();
                if let Some(name) = extract_detail(&sr.stdout, "Published Name") {
                    details.insert("published_name".into(), name);
                }
                details.insert("stdout".into(), sr.stdout.clone());
                StepResult {
                    name: "Installing driver package".into(),
                    status: StepStatus::Pass,
                    exit_code: Some(0),
                    duration,
                    details,
                    is_infra_failure: false,
                }
            }
            Ok(sr) => StepResult {
                name: "Installing driver package".into(),
                status: StepStatus::Fail(format!(
                    "install failed (exit code {}): {}",
                    sr.exit_code,
                    sr.stderr.trim()
                )),
                exit_code: Some(sr.exit_code),
                duration,
                details: HashMap::from([("stdout".into(), sr.stdout)]),
                is_infra_failure: false,
            },
            Err(e) => StepResult {
                name: "Installing driver package".into(),
                status: StepStatus::Fail(format!("install error: {e}")),
                exit_code: None,
                duration,
                details: HashMap::new(),
                is_infra_failure: false,
            },
        }
    }

    /// Verify the driver is correctly installed on the VM.
    fn step_verify(&self) -> StepResult {
        info!(
            vm = %self.config.vm_name,
            driver = %self.driver_name,
            "verifying driver on VM"
        );

        let start = Instant::now();
        let args = vec![
            "-VMName".into(),
            self.config.vm_name.clone(),
            "-DriverPath".into(),
            self.config.driver_package.display().to_string(),
        ];

        let timeout = Duration::from_secs(60);
        let result = self
            .runner
            .run_script("Verify-DriverOnVM.ps1", &args, timeout, self.credential(), false);

        let duration = start.elapsed();

        match result {
            Ok(sr) if sr.success() => {
                let mut details = HashMap::new();
                if let Some(status) = extract_detail(&sr.stdout, "Device Status") {
                    details.insert("device_status".into(), status);
                }
                details.insert("stdout".into(), sr.stdout.clone());
                StepResult {
                    name: "Verifying driver on VM".into(),
                    status: StepStatus::Pass,
                    exit_code: Some(0),
                    duration,
                    details,
                    is_infra_failure: false,
                }
            }
            Ok(sr) => StepResult {
                name: "Verifying driver on VM".into(),
                status: StepStatus::Fail(format!(
                    "verification failed (exit code {}): {}",
                    sr.exit_code,
                    sr.stderr.trim()
                )),
                exit_code: Some(sr.exit_code),
                duration,
                details: HashMap::from([("stdout".into(), sr.stdout)]),
                is_infra_failure: false,
            },
            Err(e) => StepResult {
                name: "Verifying driver on VM".into(),
                status: StepStatus::Fail(format!("verify error: {e}")),
                exit_code: None,
                duration,
                details: HashMap::new(),
                is_infra_failure: false,
            },
        }
    }

    /// Start DebugView + ETW capture on the VM (before driver install).
    fn step_start_capture(&mut self) -> StepResult {
        info!(
            vm = %self.config.vm_name,
            "starting log capture (DebugView + ETW)"
        );

        let start = Instant::now();
        let cap_config = CaptureConfig {
            vm_name: self.config.vm_name.clone(),
            capture_duration: self.config.capture_duration,
            output_dir: self.config.output_dir.clone(),
        };

        let result = capture::start_capture(&*self.runner, &cap_config);
        let duration = start.elapsed();

        match result {
            Ok(session) => {
                self.capture_session = Some(session);
                StepResult {
                    name: "Starting log capture".into(),
                    status: StepStatus::Pass,
                    exit_code: Some(0),
                    duration,
                    details: HashMap::new(),
                    is_infra_failure: false,
                }
            }
            Err(e) => StepResult {
                name: "Starting log capture".into(),
                status: StepStatus::Fail(format!("start capture error: {e}")),
                exit_code: None,
                duration,
                details: HashMap::new(),
                is_infra_failure: false,
            },
        }
    }

    /// Stop capture, decode ETL, and copy artefacts to the host.
    fn step_stop_capture(&self) -> StepResult {
        info!(
            vm = %self.config.vm_name,
            "stopping capture and extracting logs"
        );

        let start = Instant::now();

        let session = match self.capture_session.as_ref() {
            Some(s) => s,
            None => {
                return StepResult {
                    name: "Collecting captured logs".into(),
                    status: StepStatus::Fail(
                        "no active capture session to stop".into(),
                    ),
                    exit_code: None,
                    duration: start.elapsed(),
                    details: HashMap::new(),
                    is_infra_failure: false,
                };
            }
        };

        let result = capture::stop_and_extract(&*self.runner, session);
        let duration = start.elapsed();

        match result {
            Ok(cr) => {
                let mut details = HashMap::new();
                details.insert(
                    "output_dir".into(),
                    cr.output_dir.display().to_string(),
                );
                if let Some(ref p) = cr.dbgview_log {
                    details.insert("dbgview_log".into(), p.display().to_string());
                }
                if let Some(ref p) = cr.etl_file {
                    details.insert("etl_file".into(), p.display().to_string());
                }
                if let Some(ref p) = cr.xml_file {
                    details.insert("xml_file".into(), p.display().to_string());
                }
                if let Some(ref p) = cr.summary_file {
                    details.insert("summary_file".into(), p.display().to_string());
                }
                StepResult {
                    name: "Collecting captured logs".into(),
                    status: StepStatus::Pass,
                    exit_code: Some(0),
                    duration,
                    details,
                    is_infra_failure: false,
                }
            }
            Err(e) => StepResult {
                name: "Collecting captured logs".into(),
                status: StepStatus::Fail(format!("stop/extract error: {e}")),
                exit_code: None,
                duration,
                details: HashMap::new(),
                is_infra_failure: false,
            },
        }
    }

    /// Convenience accessor for the optional credential reference.
    fn credential(&self) -> Option<&VmCredential> {
        self.config.credential.as_ref()
    }
}

/// Drop impl that honours `no_cleanup`: when the flag is set, the
/// `PsRunner` is intentionally leaked so its `Drop` (which deletes the
/// temporary scripts directory) never runs.
///
/// TODO: Once `PsRunner` accepts a `no_cleanup` flag directly, this
/// `ManuallyDrop` workaround can be removed.
impl Drop for Pipeline {
    fn drop(&mut self) {
        if !self.config.no_cleanup {
            // SAFETY: We only drop the runner once, here, because it is wrapped
            // in `ManuallyDrop` and this is the only place that calls
            // `ManuallyDrop::drop`.
            unsafe {
                ManuallyDrop::drop(&mut self.runner);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a value from script stdout matching `"Key: Value"` or `"Key = Value"`.
fn extract_detail(stdout: &str, key: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        // Match "Key: Value" or "Key = Value"
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=')) {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_detail_colon_format() {
        let stdout = "Published Name: oem3.inf\nVersion: 1.0.0\n";
        assert_eq!(
            extract_detail(stdout, "Published Name"),
            Some("oem3.inf".into())
        );
        assert_eq!(extract_detail(stdout, "Version"), Some("1.0.0".into()));
    }

    #[test]
    fn extract_detail_equals_format() {
        let stdout = "Device Status = OK\n";
        assert_eq!(
            extract_detail(stdout, "Device Status"),
            Some("OK".into())
        );
    }

    #[test]
    fn extract_detail_missing_key() {
        let stdout = "some random output\n";
        assert_eq!(extract_detail(stdout, "Published Name"), None);
    }

    #[test]
    fn default_config_values() {
        let cfg = PipelineConfig::default();
        assert_eq!(cfg.capture_duration, 30);
        assert!(!cfg.skip_verify);
        assert!(!cfg.skip_capture);
        assert!(!cfg.no_cleanup);
    }

    #[test]
    fn new_rejects_missing_directory() {
        let cfg = PipelineConfig {
            driver_package: PathBuf::from(r"C:\nonexistent\path\12345"),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let err = Pipeline::new(cfg).unwrap_err();
        assert!(
            matches!(err, SmoketestError::PackageValidation { .. }),
            "expected PackageValidation, got: {err}"
        );
    }

    #[test]
    fn new_rejects_directory_without_inf() {
        let dir = std::env::temp_dir().join("pipeline-test-no-inf");
        let _ = std::fs::create_dir_all(&dir);
        // Create a .sys but no .inf
        std::fs::write(dir.join("driver.sys"), b"fake").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.clone(),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let err = Pipeline::new(cfg).unwrap_err();
        assert!(
            matches!(err, SmoketestError::PackageValidation { .. }),
            "expected PackageValidation, got: {err}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_succeeds_with_valid_package() {
        let dir = std::env::temp_dir().join("pipeline-test-valid");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("mydriver.inf"), b"fake").unwrap();

        let cfg = PipelineConfig {
            driver_package: dir.clone(),
            vm_name: "test-vm".into(),
            ..PipelineConfig::default()
        };
        let pipeline = Pipeline::new(cfg).unwrap();
        assert_eq!(pipeline.driver_name, "mydriver");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
