use crate::debug::DebugMessage;
use crate::deploy::WmiInfo;
use crate::echo_test::ApplicationOutput;
use serde::Serialize;
use std::time::Instant;

/// Simple TTY-friendly progress reporter for long-running operations.
#[derive(Debug)]
pub struct ProgressHandle {
    label: String,
    started: Instant,
    enabled: bool,
    finished: bool,
}

impl ProgressHandle {
    pub fn new(label: impl Into<String>, enabled: bool) -> Self {
        let label = label.into();
        if enabled {
            println!("▶ {}...", label);
        }
        Self {
            label,
            started: Instant::now(),
            enabled,
            finished: false,
        }
    }

    fn elapsed(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    pub fn info(&self, detail: impl AsRef<str>) {
        if self.enabled {
            println!("  • {}", detail.as_ref());
        }
    }

    pub fn success(&mut self, detail: impl AsRef<str>) {
        if self.enabled {
            println!(
                "✔ {} ({:.1}s) - {}",
                self.label,
                self.elapsed(),
                detail.as_ref()
            );
        }
        self.finished = true;
    }

    pub fn fail(&mut self, detail: impl AsRef<str>) {
        if self.enabled {
            println!("✗ {} - {}", self.label, detail.as_ref());
        }
        self.finished = true;
    }
}

impl Drop for ProgressHandle {
    fn drop(&mut self) {
        if self.enabled && !self.finished {
            println!("✗ {} - aborted", self.label);
        }
    }
}

pub fn progress(label: impl Into<String>, enabled: bool) -> ProgressHandle {
    ProgressHandle::new(label, enabled)
}

// T054: Extended DeployResult with debug_messages field
#[derive(Debug, Serialize)]
pub struct DeployResult {
    pub success: bool,
    pub published_name: Option<String>,
    pub version: Option<String>,
    pub wmi: Option<WmiInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_messages: Option<Vec<DebugMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_output: Option<ApplicationOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inf_used: Option<String>,
    pub error: Option<String>,
}

// T055: Add debug output section to JSON and human-readable output
pub fn emit_deploy(res: &DeployResult, json: bool) {
    if json {
        println!("{}", serde_json::to_string(res).unwrap());
    } else {
        if res.success {
            println!(
                "Driver deployed. Published: {:?} Version: {:?}",
                res.published_name, res.version
            );
            if let Some(inf) = &res.inf_used {
                println!("  INF Used: {}", inf);
            }
            if let Some(wmi) = &res.wmi {
                println!("  Device: {:?}", wmi.device_name);
                println!("  Manufacturer: {:?}", wmi.manufacturer);
                println!("  Provider: {:?}", wmi.driver_provider_name);
                println!("  Signed: {:?} by {:?}", wmi.is_signed, wmi.signer);
            }
            // Debug output section
            if let Some(messages) = &res.debug_messages {
                if !messages.is_empty() {
                    println!("\nDebug Output ({} messages):", messages.len());
                    for msg in messages.iter().take(20) {
                        let level = format!("{:?}", msg.level).to_uppercase();
                        let source = msg.source.as_deref().unwrap_or("<unknown>");
                        let age_ms = msg.ts.elapsed().as_millis();
                        println!("  [{}] {} ({} ms ago): {}", level, source, age_ms, msg.raw);
                    }
                    if messages.len() > 20 {
                        println!("  ... and {} more messages", messages.len() - 20);
                    }
                }
            }
            if let Some(app) = &res.application_output {
                println!(
                    "\nCompanion Application Output (exit code {}):",
                    app.exit_code
                );
                if !app.stdout.is_empty() {
                    println!("  Stdout: {}", app.stdout.trim());
                }
                if !app.stderr.is_empty() {
                    println!("  Stderr: {}", app.stderr.trim());
                }
                if !app.missing_patterns.is_empty() {
                    println!("  Missing patterns: {:?}", app.missing_patterns);
                }
            }
        } else {
            println!(
                "Driver deploy failed: {}",
                res.error.as_deref().unwrap_or("unknown")
            );
        }
    }
}
