/// Real Hyper-V integration tests requiring actual VM infrastructure.
///
/// These tests are automatically skipped if:
/// - No test VM named "driver-test-vm" exists
/// - VM is not in Running state
/// - Hyper-V is not available
///
/// To run these tests:
/// 1. Create a Windows test VM named "driver-test-vm"
/// 2. Ensure VM is running with Integration Services enabled
/// 3. Run: `cargo test --test hyperv_integration -- --ignored`
///
/// Environment variables:
/// - `DRIVER_TEST_VM_NAME`: Override default VM name (default: "driver-test-vm")
/// - `DRIVER_TEST_SKIP_CLEANUP`: Keep test artifacts after run (default: cleanup)
use driver_test_cli::vm::{HypervProvider, TestVm, VmProvider};
use std::env;

/// Get configured test VM name from environment or use default
fn get_test_vm_name() -> String {
    env::var("DRIVER_TEST_VM_NAME").unwrap_or_else(|_| "driver-test-vm".to_string())
}

/// Check if test VM exists and is accessible
fn check_vm_available() -> Option<TestVm> {
    let provider = HypervProvider::default();
    let vm_name = get_test_vm_name();

    match provider.get_vm(&vm_name) {
        Ok(Some(vm)) => {
            eprintln!("✓ Found test VM: {} (state: {})", vm.name, vm.state);
            Some(vm)
        }
        Ok(None) => {
            eprintln!(
                "✗ Test VM '{}' not found - skipping integration tests",
                vm_name
            );
            eprintln!("  Create VM or set DRIVER_TEST_VM_NAME environment variable");
            None
        }
        Err(e) => {
            eprintln!(
                "✗ Cannot access Hyper-V: {} - skipping integration tests",
                e
            );
            eprintln!("  Run tests with elevated privileges or enable Hyper-V");
            None
        }
    }
}

#[test]
#[ignore] // Only run explicitly with: cargo test --test hyperv_integration -- --ignored
fn test_vm_query() {
    let vm = match check_vm_available() {
        Some(v) => v,
        None => return, // Skip test if VM not available
    };

    assert!(!vm.name.is_empty(), "VM should have a name");
    assert!(!vm.state.is_empty(), "VM should have a state");
    eprintln!("VM details: {} MB RAM, {} CPUs", vm.memory_mb, vm.cpus);
}

#[test]
#[ignore]
fn test_vm_ensure_running() {
    let provider = HypervProvider::default();
    let vm = match check_vm_available() {
        Some(v) => v,
        None => return,
    };

    let running_vm = provider
        .ensure_running(&vm)
        .expect("Failed to ensure VM is running");

    assert_eq!(
        running_vm.state.to_ascii_lowercase(),
        "running",
        "VM should be in Running state after ensure_running"
    );
}

#[test]
#[ignore]
fn test_vm_snapshot_create_and_revert() {
    let provider = HypervProvider::default();
    let vm = match check_vm_available() {
        Some(v) => v,
        None => return,
    };

    let running_vm = provider
        .ensure_running(&vm)
        .expect("VM must be running for snapshot tests");

    // Create snapshot
    let snapshot_name = format!(
        "test-snapshot-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let snapshot = provider
        .snapshot_vm(&running_vm, &snapshot_name)
        .expect("Failed to create snapshot");

    assert_eq!(snapshot.0, snapshot_name, "Snapshot name should match");

    // Revert snapshot
    let reverted_vm = provider
        .revert_snapshot(&running_vm, &snapshot)
        .expect("Failed to revert snapshot");

    assert_eq!(
        reverted_vm.name, running_vm.name,
        "VM name should not change after revert"
    );

    // Cleanup: Remove test snapshot (best effort)
    if env::var("DRIVER_TEST_SKIP_CLEANUP").is_err() {
        let cleanup_script = format!(
            "Remove-VMSnapshot -VMName '{}' -Name '{}'",
            running_vm.name, snapshot_name
        );
        let _ = driver_test_cli::ps::run_ps_json(&cleanup_script);
        eprintln!("✓ Cleaned up test snapshot: {}", snapshot_name);
    }
}

#[test]
#[ignore]
fn test_vm_execute_command() {
    let provider = HypervProvider::default();
    let vm = match check_vm_available() {
        Some(v) => v,
        None => return,
    };

    let running_vm = provider
        .ensure_running(&vm)
        .expect("VM must be running for command execution");

    // Execute simple PowerShell command via Invoke-Command
    let output = provider
        .execute(&running_vm, "Get-ComputerInfo | Select-Object OsName")
        .expect("Failed to execute command in VM");

    assert_eq!(output.status, 0, "Command should exit successfully");
    assert!(!output.stdout.is_empty(), "Command should produce output");
    eprintln!("Command output: {}", output.stdout.trim());
}

#[test]
#[ignore]
fn test_vm_file_copy() {
    use std::fs;

    let provider = HypervProvider::default();
    let vm = match check_vm_available() {
        Some(v) => v,
        None => return,
    };

    let running_vm = provider
        .ensure_running(&vm)
        .expect("VM must be running for file copy");

    // Create temporary test file
    let temp_dir = env::temp_dir();
    let test_file = temp_dir.join(format!("driver-test-{}.txt", std::process::id()));
    let test_content = format!("Test file created at {:?}", std::time::SystemTime::now());

    fs::write(&test_file, &test_content).expect("Failed to create test file");

    // Copy to VM (requires Guest Service Interface enabled)
    let dest_path = format!("C:\\Windows\\Temp\\driver-test-{}.txt", std::process::id());

    match provider.copy_file(&running_vm, &test_file, &dest_path) {
        Ok(_) => {
            eprintln!("✓ File copied to VM: {}", dest_path);

            // Verify file exists in VM
            let verify_cmd = format!("Test-Path '{}'", dest_path);
            match provider.execute(&running_vm, &verify_cmd) {
                Ok(output) => {
                    let exists = output.stdout.trim().to_lowercase() == "true";
                    assert!(exists, "File should exist in VM after copy");
                    eprintln!("✓ File verified in VM");
                }
                Err(e) => eprintln!("⚠ Could not verify file in VM: {}", e),
            }

            // Cleanup VM file (best effort)
            if env::var("DRIVER_TEST_SKIP_CLEANUP").is_err() {
                let cleanup_cmd =
                    format!("Remove-Item '{}' -ErrorAction SilentlyContinue", dest_path);
                let _ = provider.execute(&running_vm, &cleanup_cmd);
            }
        }
        Err(e) => {
            eprintln!("⚠ File copy failed: {}", e);
            eprintln!("  Ensure Guest Service Interface is enabled in VM Integration Services");
            panic!("File copy test failed - check VM Integration Services");
        }
    }

    // Cleanup host file
    let _ = fs::remove_file(&test_file);
}

/// Integration test demonstrating complete deployment workflow
/// Requires:
/// - Test VM with Guest Service Interface enabled
/// - Test driver package (INF + certificate)
#[test]
#[ignore]
fn test_complete_deployment_workflow() {
    // This test validates the end-to-end deployment flow but requires
    // actual driver artifacts. For now, we just validate the infrastructure.

    let provider = HypervProvider::default();
    let vm = match check_vm_available() {
        Some(v) => v,
        None => return,
    };

    let running_vm = provider.ensure_running(&vm).expect("VM must be running");

    // Validate PowerShell Direct is working
    let ps_test = provider
        .execute(&running_vm, "Write-Output 'PowerShell Direct OK'")
        .expect("PowerShell Direct must be functional");

    assert!(
        ps_test.stdout.contains("PowerShell Direct OK"),
        "PowerShell Direct communication failed"
    );

    eprintln!("✓ Complete deployment infrastructure validated");
    eprintln!("  VM: {} ({})", running_vm.name, running_vm.state);
    eprintln!(
        "  Memory: {} MB, CPUs: {}",
        running_vm.memory_mb, running_vm.cpus
    );
    eprintln!("  PowerShell Direct: operational");

    // TODO: Add actual driver deployment when test artifacts are available
    // let deployer = driver_test_cli::deploy::PnpDeployer;
    // let result = driver_test_cli::deploy::deploy_driver(
    //     &deployer, &running_vm, Some(cert_path), inf_path, Some(version)
    // )?;
}
