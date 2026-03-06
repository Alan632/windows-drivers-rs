use driver_test_cli::vm::TestVm;
use driver_test_cli::vm::{SnapshotId, VmError, VmProvider};
use serde_json::json;
use std::sync::{Arc, Mutex};

// Fake executor by temporarily shadowing run_ps_json logic via a shim wrapper.
// We simulate responses based on script prefixes.

struct ScenarioState {
    started: bool,
}

fn fake_response(script: &str, state: &mut ScenarioState) -> Result<serde_json::Value, VmError> {
    if script.starts_with("New-VM") {
        return Ok(
            json!({"Name":"testvm","State":"Off","MemoryStartup": 2147483648u64, "ProcessorCount":2}),
        );
    }
    if script.starts_with("Get-VM") {
        return Ok(
            json!({"Name":"testvm","State": if state.started {"Running"} else {"Off"}, "MemoryStartup": 2147483648u64, "ProcessorCount":2 }),
        );
    }
    if script.starts_with("Start-VM") {
        state.started = true;
        return Ok(
            json!({"Name":"testvm","State":"Running","MemoryStartup": 2147483648u64, "ProcessorCount":2}),
        );
    }
    if script.starts_with("Checkpoint-VM") {
        return Ok(json!({"Name":"snap1"}));
    }
    if script.starts_with("Restore-VMSnapshot") {
        return Ok(
            json!({"Name":"testvm","State":"Running","MemoryStartup": 2147483648u64, "ProcessorCount":2}),
        );
    }
    if script.starts_with("Copy-VMFile") {
        return Ok(json!({}));
    }
    if script.starts_with("Invoke-Command") {
        return Ok(json!("echo output"));
    }
    Err(VmError::Ps(format!("unhandled script: {script}")))
}

// Wrapper provider that overrides ps method for tests
struct TestProvider {
    state: Arc<Mutex<ScenarioState>>,
}
impl TestProvider {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ScenarioState { started: false })),
        }
    }
}

fn to_vm(v: &serde_json::Value) -> Option<TestVm> {
    Some(TestVm {
        name: v.get("Name")?.as_str()?.to_string(),
        state: v.get("State")?.as_str()?.to_string(),
        memory_mb: (v.get("MemoryStartup")?.as_u64()? / (1024 * 1024)) as u32,
        cpus: v.get("ProcessorCount")?.as_u64()? as u8,
    })
}

impl VmProvider for TestProvider {
    fn create_vm(&self, name: &str, memory_mb: u32, cpus: u8) -> Result<TestVm, VmError> {
        assert_eq!(name, "testvm");
        assert_eq!(memory_mb, 2048);
        assert_eq!(cpus, 2);
        let mut s = self.state.lock().unwrap();
        let v = fake_response("New-VM", &mut s)?;
        to_vm(&v).ok_or(VmError::Ps("shape".into()))
    }
    fn get_vm(&self, _name: &str) -> Result<Option<TestVm>, VmError> {
        let mut s = self.state.lock().unwrap();
        let v = fake_response("Get-VM", &mut s)?;
        Ok(to_vm(&v))
    }
    fn ensure_running(&self, vm: &TestVm) -> Result<TestVm, VmError> {
        if vm.state == "Running" {
            return Ok(vm.clone());
        }
        let mut s = self.state.lock().unwrap();
        let v = fake_response("Start-VM", &mut s)?;
        to_vm(&v).ok_or(VmError::Ps("shape".into()))
    }
    fn snapshot_vm(&self, _vm: &TestVm, label: &str) -> Result<SnapshotId, VmError> {
        let mut s = self.state.lock().unwrap();
        let _ = fake_response("Checkpoint-VM", &mut s)?;
        Ok(SnapshotId(label.to_string()))
    }
    fn revert_snapshot(&self, _vm: &TestVm, snap: &SnapshotId) -> Result<TestVm, VmError> {
        let mut s = self.state.lock().unwrap();
        let v = fake_response("Restore-VMSnapshot", &mut s)?;
        let mut vm = to_vm(&v).ok_or(VmError::Ps("shape".into()))?;
        vm.state = format!("Running (reverted {})", snap.0);
        Ok(vm)
    }
    fn execute(
        &self,
        _vm: &TestVm,
        _command: &str,
    ) -> Result<driver_test_cli::vm::CommandOutput, VmError> {
        let mut s = self.state.lock().unwrap();
        let v = fake_response("Invoke-Command", &mut s)?;
        Ok(driver_test_cli::vm::CommandOutput {
            stdout: v.to_string(),
            stderr: String::new(),
            status: 0,
        })
    }
    fn copy_file(&self, _vm: &TestVm, _src: &std::path::Path, _dest: &str) -> Result<(), VmError> {
        let mut s = self.state.lock().unwrap();
        let _ = fake_response("Copy-VMFile", &mut s)?;
        Ok(())
    }
}

#[test]
fn create_and_start_and_snapshot_flow() {
    let prov = TestProvider::new();
    let vm = prov.create_vm("testvm", 2048, 2).expect("create");
    assert_eq!(vm.state, "Off");
    let vm_running = prov.ensure_running(&vm).expect("run");
    assert!(vm_running.state.to_ascii_lowercase().contains("running"));
    let snap = prov.snapshot_vm(&vm_running, "baseline").unwrap();
    assert_eq!(snap.0, "baseline");
    let reverted = prov.revert_snapshot(&vm_running, &snap).unwrap();
    assert!(reverted.state.contains("reverted baseline"));
    prov.copy_file(
        &reverted,
        std::path::Path::new("C:/Host/file.txt"),
        "C:/Guest/file.txt",
    )
    .unwrap();
    let out = prov.execute(&reverted, "echo hi").unwrap();
    assert!(out.stdout.contains("echo"));
}
