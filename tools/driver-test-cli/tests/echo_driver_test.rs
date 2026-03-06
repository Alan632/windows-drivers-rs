use assert_fs::prelude::*;
use assert_fs::TempDir;
use driver_test_cli::debug::classify;
use driver_test_cli::deploy::copy_application;
use driver_test_cli::driver_detect::locate_companion_application;
use driver_test_cli::echo_test::{
    correlate_outputs, ApplicationOutput, CompanionApplication, OutputCorrelation,
};
use driver_test_cli::package::{DriverPackage, DriverType, RepositoryType};
use driver_test_cli::vm::{CommandOutput, SnapshotId, TestVm, VmError, VmProvider};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[test]
fn locate_companion_from_bin_entry() {
    let temp = TempDir::new().unwrap();
    temp.child("Cargo.toml")
        .write_str("[[bin]]\nname = \"echo-app\"\n")
        .unwrap();
    temp.child("target/release").create_dir_all().unwrap();
    temp.child("target/release/echo-app.exe")
        .write_str("stub")
        .unwrap();
    temp.child("driver.inf")
        .write_str("[Version]\nSignature = \"$Windows NT$\"")
        .unwrap();

    let package = DriverPackage::new(
        temp.path().to_path_buf(),
        RepositoryType::WindowsDriversRs,
        DriverType::Kmdf,
        Some("1.0.0.0".into()),
        Some(temp.child("driver.inf").path().to_path_buf()),
    );

    let companion = locate_companion_application(&package).unwrap();
    assert!(companion.is_some());
    let companion = companion.unwrap();
    assert!(companion.file_name().contains("echo-app"));
}

struct RecordingProvider {
    copied: Mutex<Vec<String>>,
}

impl Default for RecordingProvider {
    fn default() -> Self {
        Self {
            copied: Mutex::new(Vec::new()),
        }
    }
}

impl VmProvider for RecordingProvider {
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
            stdout: String::new(),
            stderr: String::new(),
            status: 0,
        })
    }
    fn copy_file(&self, _vm: &TestVm, _src: &Path, dest: &str) -> Result<(), VmError> {
        self.copied.lock().unwrap().push(dest.to_string());
        Ok(())
    }
}

#[test]
fn copy_application_records_destination() {
    let provider = RecordingProvider::default();
    let vm = TestVm {
        name: "vm".into(),
        state: "Running".into(),
        memory_mb: 1024,
        cpus: 2,
    };
    let companion = CompanionApplication::new(PathBuf::from("host.exe"), vec!["ok".into()]);
    copy_application(&provider, &vm, &companion).unwrap();
    let copied = provider.copied.lock().unwrap();
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0], companion.remote_path());
}

#[test]
fn correlate_outputs_flags_driver_messages() {
    let messages = vec![
        classify("echo: sending packet"),
        classify("driver complete"),
    ];
    let app_output = ApplicationOutput {
        stdout: "echo: sending packet".into(),
        stderr: String::new(),
        exit_code: 0,
        matched_patterns: vec!["echo: sending packet".into()],
        missing_patterns: Vec::new(),
    };
    let correlations: Vec<OutputCorrelation> = correlate_outputs(&messages, &app_output);
    assert!(correlations
        .iter()
        .any(|c| c.driver_emitted && c.application_emitted));
}
