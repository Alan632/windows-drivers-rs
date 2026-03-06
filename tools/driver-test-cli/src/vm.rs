use crate::ps::{run_ps_json, PsError};
use serde_json::Value;
use std::{path::Path, thread, time::Duration};
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct TestVm {
    pub name: String,
    pub state: String,
    pub memory_mb: u32,
    pub cpus: u8,
}

#[derive(Debug, Clone)]
pub struct SnapshotId(pub String);

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

#[derive(Debug, Error)]
pub enum VmError {
    #[error("vm not found: {0}")]
    NotFound(String),
    #[error("powershell error: {0}")]
    Ps(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("transient failure: {0}")]
    Transient(String),
    #[error("timeout waiting for operation")]
    Timeout,
}

pub trait VmProvider {
    fn create_vm(&self, name: &str, memory_mb: u32, cpus: u8) -> Result<TestVm, VmError>;
    fn get_vm(&self, name: &str) -> Result<Option<TestVm>, VmError>;
    fn ensure_running(&self, vm: &TestVm) -> Result<TestVm, VmError>;
    fn snapshot_vm(&self, vm: &TestVm, label: &str) -> Result<SnapshotId, VmError>;
    fn revert_snapshot(&self, vm: &TestVm, snap: &SnapshotId) -> Result<TestVm, VmError>;
    fn execute(&self, vm: &TestVm, command: &str) -> Result<CommandOutput, VmError>;
    fn copy_file(&self, vm: &TestVm, src: &Path, dest: &str) -> Result<(), VmError>;
}

pub struct HypervProvider {
    pub max_retries: usize,
    pub retry_delay: Duration,
}

impl Default for HypervProvider {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
        }
    }
}

fn classify_transient(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("the virtual machine is not ready")
        || s.contains("failed to establish a connection")
        || s.contains("transient")
}

fn ps_to_vm(v: &Value) -> Option<TestVm> {
    let name = v.get("Name")?.as_str()?.to_string();
    let state = v
        .get("State")
        .and_then(|s| s.as_str())
        .unwrap_or("Unknown")
        .to_string();
    // MemoryStartup is bytes; convert to MB if present
    let mem_bytes = v.get("MemoryStartup").and_then(|m| m.as_u64()).unwrap_or(0);
    let memory_mb = (mem_bytes / (1024 * 1024)) as u32;
    let cpus = v
        .get("ProcessorCount")
        .and_then(|c| c.as_u64())
        .unwrap_or(1) as u8;
    Some(TestVm {
        name,
        state,
        memory_mb,
        cpus,
    })
}

fn run_with_backoff<F, T>(max_retries: usize, delay: Duration, mut f: F) -> Result<T, VmError>
where
    F: FnMut() -> Result<T, VmError>,
{
    let mut attempt = 0;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(VmError::Transient(_)) => {
                if attempt >= max_retries {
                    return Err(VmError::Timeout);
                }
                warn!(attempt, "transient VM operation error; backing off");
                thread::sleep(delay);
                attempt += 1;
            }
            Err(other) => return Err(other),
        }
    }
}

impl HypervProvider {
    fn ps(&self, script: &str) -> Result<Value, VmError> {
        run_ps_json(script).map_err(|e| match e {
            PsError::Fail { ref stderr, .. } => {
                if classify_transient(stderr) {
                    VmError::Transient(e.to_string())
                } else {
                    VmError::Ps(e.to_string())
                }
            }
            PsError::Io(io) => VmError::Io(io),
            other => VmError::Ps(other.to_string()),
        })
    }
}

impl VmProvider for HypervProvider {
    fn create_vm(&self, name: &str, memory_mb: u32, cpus: u8) -> Result<TestVm, VmError> {
        info!(%name, memory_mb, cpus, "creating vm");
        let script = format!("New-VM -Name '{name}' -MemoryStartupBytes {bytes} -Generation 2 | Select-Object Name,State,MemoryStartup,ProcessorCount | ConvertTo-Json", bytes = memory_mb as u64 * 1024 * 1024);
        let val = run_with_backoff(self.max_retries, self.retry_delay, || self.ps(&script))?;
        ps_to_vm(&val).ok_or_else(|| VmError::Ps("unexpected vm object shape".into()))
    }

    fn get_vm(&self, name: &str) -> Result<Option<TestVm>, VmError> {
        let script = format!("Get-VM -Name '{name}' | Select-Object Name,State,MemoryStartup,ProcessorCount | ConvertTo-Json");
        match self.ps(&script) {
            Ok(v) => {
                if v.is_array() {
                    return v
                        .as_array()
                        .and_then(|a| a.get(0))
                        .and_then(ps_to_vm)
                        .map(Some)
                        .ok_or(VmError::Ps("unexpected array shape".into()));
                }
                Ok(ps_to_vm(&v))
            }
            Err(VmError::Ps(s)) if is_vm_missing(&s) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn ensure_running(&self, vm: &TestVm) -> Result<TestVm, VmError> {
        if vm.state.to_ascii_lowercase() == "running" {
            return Ok(vm.clone());
        }
        let start_script = format!("Start-VM -Name '{name}' -WarningAction SilentlyContinue; Get-VM -Name '{name}' | Select-Object Name,State,MemoryStartup,ProcessorCount | ConvertTo-Json", name = vm.name);
        let val = run_with_backoff(self.max_retries, self.retry_delay, || {
            self.ps(&start_script)
        })?;
        ps_to_vm(&val).ok_or_else(|| VmError::Ps("unexpected vm object after start".into()))
    }

    fn snapshot_vm(&self, vm: &TestVm, label: &str) -> Result<SnapshotId, VmError> {
        info!(vm=%vm.name, %label, "creating snapshot");
        let script = format!(
            "Checkpoint-VM -Name '{name}' -SnapshotName '{label}'; echo '{{\"Name\":\"{label}\"}}'",
            name = vm.name,
            label = label
        );
        let _ = run_with_backoff(self.max_retries, self.retry_delay, || self.ps(&script))?;
        Ok(SnapshotId(label.to_string()))
    }

    fn revert_snapshot(&self, vm: &TestVm, snap: &SnapshotId) -> Result<TestVm, VmError> {
        info!(vm=%vm.name, snapshot=%snap.0, "reverting snapshot");
        let script = format!("Restore-VMSnapshot -VMName '{name}' -Name '{snap}'; Get-VM -Name '{name}' | Select-Object Name,State,MemoryStartup,ProcessorCount | ConvertTo-Json", name = vm.name, snap = snap.0);
        let val = run_with_backoff(self.max_retries, self.retry_delay, || self.ps(&script))?;
        ps_to_vm(&val).ok_or_else(|| VmError::Ps("unexpected vm object after revert".into()))
    }

    fn execute(&self, vm: &TestVm, command: &str) -> Result<CommandOutput, VmError> {
        debug!(vm=%vm.name, cmd=%command, "invoke powershell direct");
        // Simplified; real implementation would use Invoke-Command -VMName and capture exit code.
        let script = format!(
            "Invoke-Command -VMName '{name}' -ScriptBlock {{ {command} }} | ConvertTo-Json",
            name = vm.name,
            command = command
        );
        match self.ps(&script) {
            Ok(v) => Ok(CommandOutput {
                stdout: v.to_string(),
                stderr: String::new(),
                status: 0,
            }),
            Err(VmError::Transient(s)) => Err(VmError::Transient(s)),
            Err(VmError::Ps(s)) => Err(VmError::Ps(s)),
            Err(e) => Err(e),
        }
    }

    fn copy_file(&self, vm: &TestVm, src: &Path, dest: &str) -> Result<(), VmError> {
        let src_str = src.display();
        let script = format!("Copy-VMFile -Name '{name}' -SourcePath '{src}' -DestinationPath '{dest}' -FileSource Host; echo '{{}}'", name = vm.name, src = src_str, dest = dest);
        run_with_backoff(self.max_retries, self.retry_delay, || self.ps(&script))?;
        Ok(())
    }
}

fn is_vm_missing(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("hyper-v was unable to find a virtual machine")
        || s.contains("unable to find a virtual machine with name")
        || s.contains("could not find a virtual machine with name")
        || s.contains("cannot find a virtual machine with name")
        || (s.contains("was not found") && s.contains("virtual machine"))
        || s.contains("not find a virtual machine with name")
}

#[cfg(test)]
mod tests {
    use super::is_vm_missing;

    #[test]
    fn detects_unable_to_find_message() {
        let msg = "Hyper-V was unable to find a virtual machine with name \"driver-test-vm\"";
        assert!(is_vm_missing(msg));
    }

    #[test]
    fn ignores_other_errors() {
        let msg = "PowerShell Direct is not supported on this machine";
        assert!(!is_vm_missing(msg));
    }
}
