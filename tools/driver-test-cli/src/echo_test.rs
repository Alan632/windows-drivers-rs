use crate::debug::DebugMessage;
use crate::vm::{CommandOutput, TestVm, VmError, VmProvider};
use serde::Serialize;
use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, info};

const DEFAULT_REMOTE_DIR: &str = r"C:\driver-test\apps";

#[derive(Debug, Clone, Serialize)]
pub struct CompanionApplication {
    pub executable_path: PathBuf,
    pub arguments: Vec<String>,
    pub expected_patterns: Vec<String>,
    pub remote_directory: String,
}

impl CompanionApplication {
    pub fn new(executable_path: PathBuf, expected_patterns: Vec<String>) -> Self {
        Self {
            executable_path,
            arguments: Vec::new(),
            expected_patterns,
            remote_directory: DEFAULT_REMOTE_DIR.into(),
        }
    }

    pub fn file_name(&self) -> String {
        self.executable_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("companion.exe")
            .to_string()
    }

    pub fn remote_path(&self) -> String {
        let trimmed = self.remote_directory.trim_end_matches(['\\', '/']);
        format!("{}\\{}", trimmed, self.file_name())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplicationOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub matched_patterns: Vec<String>,
    pub missing_patterns: Vec<String>,
}

#[derive(Debug, Error)]
pub enum EchoTestError {
    #[error("vm error: {0}")]
    Vm(#[from] VmError),
    #[error("missing expected patterns: {0:?}")]
    MissingPatterns(Vec<String>),
}

pub fn run_application<P: VmProvider>(
    prov: &P,
    vm: &TestVm,
    app: &CompanionApplication,
) -> Result<ApplicationOutput, EchoTestError> {
    info!(vm=%vm.name, exe=%app.file_name(), "running companion application");
    let command = build_remote_command(app);
    debug!(vm=%vm.name, %command, "executing companion app inside VM");
    let output = prov.execute(vm, &command)?;
    Ok(capture_application_output(&output, &app.expected_patterns))
}

fn build_remote_command(app: &CompanionApplication) -> String {
    let mut parts = Vec::new();
    parts.push(format!("& '{}'", app.remote_path().replace('\\', "\\\\")));
    if !app.arguments.is_empty() {
        parts.push(app.arguments.join(" "));
    }
    parts.join(" ")
}

pub fn capture_application_output(
    output: &CommandOutput,
    expected_patterns: &[String],
) -> ApplicationOutput {
    let mut matched = Vec::new();
    let mut missing = Vec::new();
    for pattern in expected_patterns {
        if output.stdout.contains(pattern) {
            matched.push(pattern.clone());
        } else {
            missing.push(pattern.clone());
        }
    }
    ApplicationOutput {
        stdout: output.stdout.clone(),
        stderr: output.stderr.clone(),
        exit_code: output.status,
        matched_patterns: matched,
        missing_patterns: missing,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputCorrelation {
    pub pattern: String,
    pub application_emitted: bool,
    pub driver_emitted: bool,
}

pub fn correlate_outputs(
    messages: &[DebugMessage],
    app_output: &ApplicationOutput,
) -> Vec<OutputCorrelation> {
    let mut correlations = Vec::new();
    let all_patterns: Vec<String> = app_output
        .matched_patterns
        .iter()
        .chain(app_output.missing_patterns.iter())
        .cloned()
        .collect();
    for pattern in all_patterns {
        let driver_emitted = messages.iter().any(|msg| msg.raw.contains(&pattern));
        let app_emitted = app_output.matched_patterns.iter().any(|m| m == &pattern);
        correlations.push(OutputCorrelation {
            pattern,
            application_emitted: app_emitted,
            driver_emitted,
        });
    }
    correlations
}

pub fn validate_echo_behavior(app_output: &ApplicationOutput) -> Result<(), EchoTestError> {
    if app_output.missing_patterns.is_empty() && app_output.exit_code == 0 {
        Ok(())
    } else {
        Err(EchoTestError::MissingPatterns(
            app_output.missing_patterns.clone(),
        ))
    }
}

pub fn run_echo_tests<P: VmProvider>(
    prov: &P,
    vm: &TestVm,
    companion: &CompanionApplication,
) -> Result<ApplicationOutput, EchoTestError> {
    let app_output = run_application(prov, vm, companion)?;
    validate_echo_behavior(&app_output)?;
    Ok(app_output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{SnapshotId, VmError, VmProvider};
    use std::path::Path;

    #[derive(Debug)]
    struct StubProv;

    impl VmProvider for StubProv {
        fn create_vm(&self, _name: &str, _memory_mb: u32, _cpus: u8) -> Result<TestVm, VmError> {
            unimplemented!()
        }
        fn get_vm(&self, _name: &str) -> Result<Option<TestVm>, VmError> {
            Ok(None)
        }
        fn ensure_running(&self, vm: &TestVm) -> Result<TestVm, VmError> {
            Ok(vm.clone())
        }
        fn snapshot_vm(&self, _vm: &TestVm, _label: &str) -> Result<SnapshotId, VmError> {
            unimplemented!()
        }
        fn revert_snapshot(&self, vm: &TestVm, _snap: &SnapshotId) -> Result<TestVm, VmError> {
            Ok(vm.clone())
        }
        fn execute(&self, _vm: &TestVm, _command: &str) -> Result<CommandOutput, VmError> {
            Ok(CommandOutput {
                stdout: "echo success pattern1 pattern2".into(),
                stderr: String::new(),
                status: 0,
            })
        }
        fn copy_file(&self, _vm: &TestVm, _src: &Path, _dest: &str) -> Result<(), VmError> {
            Ok(())
        }
    }

    fn test_vm() -> TestVm {
        TestVm {
            name: "vm".into(),
            state: "Running".into(),
            memory_mb: 0,
            cpus: 1,
        }
    }

    #[test]
    fn capture_output_marks_missing_patterns() {
        let output = CommandOutput {
            stdout: "pattern1 pattern2".into(),
            stderr: String::new(),
            status: 0,
        };
        let patterns = vec!["pattern1".into(), "patternX".into()];
        let summary = capture_application_output(&output, &patterns);
        assert_eq!(summary.missing_patterns, vec!["patternX".to_string()]);
        assert!(summary.matched_patterns.contains(&"pattern1".to_string()));
    }

    #[test]
    fn run_application_uses_vm_provider() {
        let vm = test_vm();
        let companion = CompanionApplication {
            executable_path: PathBuf::from("./echo.exe"),
            arguments: vec!["--run".into()],
            expected_patterns: vec!["pattern1".into(), "pattern2".into()],
            remote_directory: DEFAULT_REMOTE_DIR.into(),
        };
        let prov = StubProv;
        let result = run_application(&prov, &vm, &companion).unwrap();
        assert!(result.missing_patterns.is_empty());
    }
}
