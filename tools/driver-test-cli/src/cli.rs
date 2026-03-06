use crate::debug::{validate_output_patterns, CaptureConfig, DebugCapture, DebugOutputCapture};
use crate::deploy::query_wmi_info;
use crate::deploy::{copy_application, deploy_driver, DriverDeployer, PnpDeployer};
use crate::driver_detect::{
    detect_driver_type, detect_samples_repository, locate_companion_application,
};
use crate::echo_test::{correlate_outputs, run_echo_tests, ApplicationOutput};
use crate::errors::AppError;
use crate::output::{emit_deploy, progress, DeployResult};
use crate::vm::TestVm;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "driver-test",
    version,
    about = "Automate Windows driver build, deployment and verification in a Hyper-V test VM."
)]
pub struct Cli {
    #[arg(long, global = true, help = "Emit JSON structured output")]
    pub json: bool,
    #[arg(short, long, global=true, action=clap::ArgAction::Count, help="Increase verbosity (-v, -vv, -vvv)")]
    pub verbose: u8,
    #[arg(long, global = true, help = "Override VM name (default from config)")]
    pub vm_name: Option<String>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Detect, build, deploy, and verify a driver
    Test(TestCommand),
    /// Create and configure test VM baseline
    Setup(SetupCommand),
    /// Create or revert baseline snapshot
    Snapshot(SnapshotCommand),
    /// Remove test VM and cleanup resources
    Clean(CleanCommand),
    /// Deploy a driver (INF + optional certificate) to a VM and optionally verify version
    Deploy(DeployCommand),
}

#[derive(Args, Debug)]
pub struct TestCommand {
    #[arg(long, help = "Path to driver package (default: current directory)")]
    pub package_path: Option<String>,
    #[arg(long, help = "Revert to baseline snapshot before running")]
    pub revert_snapshot: bool,
    #[arg(long, help = "Force rebuild of VM before testing")]
    pub rebuild_vm: bool,
    #[arg(long, help = "Capture debug output during test")]
    pub capture_output: bool,
    #[arg(long, help = "Manually override detected driver type (KMDF|UMDF|WDM)")]
    pub driver_type: Option<String>,
}

impl TestCommand {
    pub fn run(&self, vm_name: Option<&str>) -> Result<()> {
        use crate::vm::{HypervProvider, SnapshotId, VmProvider};
        use std::process::Command;

        let vm_name = vm_name.unwrap_or("driver-test-vm");
        info!(vm=%vm_name, revert=self.revert_snapshot, "starting test workflow");

        let package_root = PathBuf::from(self.package_path.as_deref().unwrap_or("."));
        if detect_samples_repository(&package_root) {
            info!(root=%package_root.display(), "detected Windows Rust driver samples repository layout");
        }
        let driver_package = detect_driver_type(&package_root, self.driver_type.as_deref())
            .map_err(|e| AppError::Detection(format!("failed to detect driver package: {e}")))?;
        println!("Detected driver type: {:?}", driver_package.driver_type);

        let provider = HypervProvider::default();

        let vm = provider.get_vm(vm_name)?.ok_or_else(|| {
            AppError::Vm(format!(
                "VM '{}' not found. Run 'driver-test setup' first.",
                vm_name
            ))
        })?;

        if self.revert_snapshot {
            info!("reverting to baseline snapshot");
            let snapshot = SnapshotId("baseline".to_string());
            let _ = provider.revert_snapshot(&vm, &snapshot)?;
            println!("VM reverted to baseline snapshot");
        }

        let vm = provider.ensure_running(&vm)?;
        println!("VM '{}' is running", vm.name);

        let mut build_progress = progress(
            format!("Building driver package in {}", package_root.display()),
            true,
        );
        build_progress.info("cargo build --release");
        let build_output = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(&package_root)
            .output();

        match build_output {
            Ok(output) if output.status.success() => {
                build_progress.success("cargo build --release")
            }
            Ok(output) => {
                build_progress.fail("cargo build failed");
                eprintln!("Build failed:");
                eprintln!("{}", String::from_utf8_lossy(&output.stderr));
                anyhow::bail!("Build failed");
            }
            Err(e) => {
                build_progress.fail(format!("failed to run cargo: {}", e));
                eprintln!("Failed to run cargo build: {}", e);
                anyhow::bail!("cargo build required for test workflow");
            }
        }

        let inf_path = driver_package
            .inf_path
            .clone()
            .or_else(|| find_inf_fallback(&package_root))
            .ok_or_else(|| anyhow::anyhow!("unable to locate INF file for package"))?;

        let mut capture_session = if self.capture_output {
            let log_path = PathBuf::from(format!("C:\\debugview_{}.log", vm.name));
            let cfg = CaptureConfig {
                path: log_path,
                ..Default::default()
            };
            let mut backend = DebugCapture;
            let session = backend.start_capture(cfg).map_err(|e| anyhow::anyhow!(e))?;
            Some((session, backend))
        } else {
            None
        };

        let deployer = PnpDeployer::default();
        let mut deploy_progress = progress("Deploying driver package", true);
        deploy_progress.info(format!("INF: {}", inf_path.display()));
        let install = match deploy_driver(
            &deployer,
            &vm,
            None,
            &inf_path,
            driver_package.version.as_deref(),
        ) {
            Ok(install) => {
                deploy_progress.success("Driver installed");
                install
            }
            Err(err) => {
                deploy_progress.fail(err.to_string());
                return Err(err.into());
            }
        };

        let companion = locate_companion_application(&driver_package)
            .map_err(|e| AppError::Detection(format!("companion detection failed: {e}")))?;

        let mut application_output: Option<ApplicationOutput> = None;
        if let Some(ref companion) = companion {
            info!("copying companion application to VM");
            let mut copy_progress = progress(format!("Copying {}", companion.file_name()), true);
            copy_progress.info(format!("Source: {}", companion.executable_path.display()));
            match copy_application(&provider, &vm, &companion) {
                Ok(remote) => {
                    copy_progress.success(format!("Copied to {}", remote));
                }
                Err(err) => {
                    copy_progress.fail(err.to_string());
                    return Err(err.into());
                }
            }
            let mut run_progress = progress(format!("Running {}", companion.file_name()), true);
            run_progress.info("Executing Hyper-V guest command");
            match run_echo_tests(&provider, &vm, &companion) {
                Ok(output) => {
                    run_progress.success("Companion application completed");
                    application_output = Some(output);
                }
                Err(err) => {
                    run_progress.fail(err.to_string());
                    return Err(err.into());
                }
            }
        }

        let debug_messages = if let Some((mut session, mut backend)) = capture_session.take() {
            let _ = backend.read_messages(&mut session);
            let duration_ms = session.duration_ms();
            let messages = backend.stop_capture(session).unwrap_or_else(|e| {
                warn!("failed stopping debug capture: {}", e);
                Vec::new()
            });
            if !messages.is_empty() {
                info!(duration_ms, count = messages.len(), "captured debug output");
            }
            Some(messages)
        } else {
            None
        };

        if let (Some(messages), Some(ref output), Some(ref companion)) =
            (&debug_messages, &application_output, &companion)
        {
            let driver_missing = validate_output_patterns(messages, &companion.expected_patterns);
            if !driver_missing.is_empty() {
                println!("Driver missing debug patterns: {:?}", driver_missing);
            }
            let correlations = correlate_outputs(messages, output);
            if !correlations.is_empty() {
                println!("Correlated patterns (application vs driver):");
                for corr in correlations.iter().take(10) {
                    println!(
                        "  {} → app:{} driver:{}",
                        corr.pattern, corr.application_emitted, corr.driver_emitted
                    );
                }
            }
        }
        let wmi = if let Some(published) = install.published_name.as_deref() {
            let mut wmi_progress =
                progress(format!("Collecting WMI metadata for {}", published), true);
            wmi_progress.info("Query: Win32_PnPSignedDriver");
            match query_wmi_info(&vm, published) {
                Ok(info) => {
                    wmi_progress.success("Metadata retrieved");
                    Some(info)
                }
                Err(err) => {
                    wmi_progress.fail(err.to_string());
                    None
                }
            }
        } else {
            None
        };

        let result = DeployResult {
            success: true,
            published_name: install.published_name,
            version: install.version,
            wmi,
            debug_messages,
            application_output,
            inf_used: Some(install.inf_used),
            error: None,
        };

        emit_deploy(&result, false);
        println!("Test workflow complete");
        Ok(())
    }
}

fn find_inf_fallback(root: &Path) -> Option<PathBuf> {
    for entry in WalkDir::new(root)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("inf"))
            .unwrap_or(false)
        {
            return Some(path.to_path_buf());
        }
    }
    None
}

#[derive(Args, Debug)]
pub struct SetupCommand {
    #[arg(long, help = "VM name")]
    pub vm_name: Option<String>,
    #[arg(long, help = "Memory (MB)", default_value = "2048")]
    pub memory_mb: u32,
    #[arg(long, help = "CPU count", default_value = "2")]
    pub cpu_count: u32,
    #[arg(long, help = "Disk size (GB)", default_value = "60")]
    pub disk_gb: u32,
}
impl SetupCommand {
    pub fn run(&self) -> Result<()> {
        use crate::vm::{HypervProvider, VmProvider};
        let vm_name = self.vm_name.as_deref().unwrap_or("driver-test-vm");
        info!(vm=%vm_name, memory_mb=self.memory_mb, cpus=self.cpu_count, "creating test VM");

        let provider = HypervProvider::default();

        // Check if VM already exists
        if let Some(existing) = provider.get_vm(vm_name)? {
            info!(vm=%vm_name, state=%existing.state, "VM already exists");
            println!(
                "VM '{}' already exists (state: {})",
                vm_name, existing.state
            );
            return Ok(());
        }

        // Create VM
        let mut progress_handle = progress(format!("Creating VM '{}'", vm_name), true);
        progress_handle.info("Allocating Hyper-V resources");
        let vm = match provider.create_vm(vm_name, self.memory_mb, self.cpu_count as u8) {
            Ok(vm) => {
                progress_handle.success("VM created");
                vm
            }
            Err(err) => {
                progress_handle.fail(err.to_string());
                return Err(err.into());
            }
        };
        info!(vm=%vm.name, state=%vm.state, "VM created successfully");
        println!("VM '{}' created successfully", vm.name);
        println!("  State: {}", vm.state);
        println!("  Memory: {} MB", vm.memory_mb);
        println!("  CPUs: {}", vm.cpus);
        println!("\nNext steps:");
        println!("  1. Install Windows on the VM");
        println!("  2. Enable Integration Services (Guest Service Interface)");
        println!("  3. Start VM: Start-VM -Name '{}'", vm.name);
        println!("  4. Create baseline snapshot: driver-test-cli snapshot --create");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct SnapshotCommand {
    #[arg(long, help = "Create new baseline snapshot")]
    pub create: bool,
    #[arg(long, help = "Revert to existing baseline snapshot")]
    pub revert: bool,
}
impl SnapshotCommand {
    pub fn run(&self, vm_name: Option<&str>) -> Result<()> {
        use crate::vm::{HypervProvider, SnapshotId, VmProvider};

        if !self.create && !self.revert {
            anyhow::bail!("Must specify either --create or --revert");
        }

        let vm_name = vm_name.unwrap_or("driver-test-vm");
        let provider = HypervProvider::default();

        let vm = provider
            .get_vm(vm_name)?
            .ok_or_else(|| anyhow::anyhow!("VM '{}' not found", vm_name))?;

        if self.create {
            info!(vm=%vm.name, "creating baseline snapshot");
            let snapshot = provider.snapshot_vm(&vm, "baseline")?;
            println!(
                "Baseline snapshot '{}' created for VM '{}'",
                snapshot.0, vm.name
            );
        }

        if self.revert {
            info!(vm=%vm.name, "reverting to baseline snapshot");
            let snapshot = SnapshotId("baseline".to_string());
            let reverted_vm = provider.revert_snapshot(&vm, &snapshot)?;
            println!("VM '{}' reverted to baseline snapshot", reverted_vm.name);
            println!("  State: {}", reverted_vm.state);
        }

        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct CleanCommand {
    #[arg(long, help = "Skip confirmation")]
    pub yes: bool,
}
impl CleanCommand {
    pub fn run(&self, vm_name: Option<&str>) -> Result<()> {
        use crate::vm::{HypervProvider, VmError, VmProvider};
        use std::io::{self, Write};

        let vm_name = vm_name.unwrap_or("driver-test-vm");
        let provider = HypervProvider::default();

        // Check if VM exists
        let vm = match provider.get_vm(vm_name)? {
            Some(v) => v,
            None => {
                let err = VmError::NotFound(vm_name.to_string());
                println!("{}", err);
                return Ok(());
            }
        };

        // Confirm deletion unless --yes flag
        if !self.yes {
            print!("Remove VM '{}' and all its data? [y/N]: ", vm.name);
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Cancelled");
                return Ok(());
            }
        }

        info!(vm=%vm.name, "removing test VM");

        // Stop VM if running
        if vm.state.to_ascii_lowercase() == "running" {
            let stop_script = format!("Stop-VM -Name '{}' -Force", vm.name);
            crate::ps::run_ps_json(&stop_script)
                .map_err(|e| anyhow::anyhow!("Failed to stop VM: {}", e))?;
            println!("VM '{}' stopped", vm.name);
        }

        // Remove VM
        let remove_script = format!("Remove-VM -Name '{}' -Force", vm.name);
        crate::ps::run_ps_json(&remove_script)
            .map_err(|e| anyhow::anyhow!("Failed to remove VM: {}", e))?;

        println!("VM '{}' removed successfully", vm.name);
        Ok(())
    }
}

// T052: Add --capture-output flag to DeployCommand
#[derive(Args, Debug)]
pub struct DeployCommand {
    #[arg(long, help = "VM name (overrides global --vm-name)")]
    pub vm_name: Option<String>,
    #[arg(long, help = "Path to INF file to install")]
    pub inf: String,
    #[arg(long, help = "Optional certificate (.cer) path")]
    pub cert: Option<String>,
    #[arg(long, help = "Expected driver version for verification")]
    pub expected_version: Option<String>,
    #[arg(long, help = "Query WMI metadata for enriched driver information")]
    pub wmi: bool,
    #[arg(long, help = "Capture and display debug output during deployment")]
    pub capture_output: bool,
}
impl DeployCommand {
    fn execute<D: DriverDeployer>(
        &self,
        deployer: &D,
        vm_name_override: Option<&str>,
        json: bool,
    ) -> Result<DeployResult> {
        let vm_name = self
            .vm_name
            .as_deref()
            .or(vm_name_override)
            .unwrap_or("test-vm");
        info!(vm=%vm_name, inf=%self.inf, cert=?self.cert, expected=?self.expected_version, wmi=self.wmi, capture=self.capture_output, "deploy command started");
        let vm = TestVm {
            name: vm_name.to_string(),
            state: "Running".into(),
            memory_mb: 0,
            cpus: 1,
        };
        let inf_path = Path::new(&self.inf);
        let cert_path = self.cert.as_ref().map(|c| Path::new(c));

        // T048: read_messages with real-time streaming (via polling)
        // Start debug capture if requested
        let mut capture_session = if self.capture_output {
            use std::path::PathBuf;
            let log_path = PathBuf::from(format!("C:\\debugview_{}.log", vm_name));
            let cfg = CaptureConfig {
                path: log_path,
                ..Default::default()
            };
            let mut backend = DebugCapture;
            let session = backend.start_capture(cfg).map_err(|e| anyhow::anyhow!(e))?;
            Some((session, backend))
        } else {
            None
        };

        let show_progress = !json;
        let mut deploy_progress =
            progress(format!("Deploying {}", inf_path.display()), show_progress);
        let result = match deploy_driver(
            deployer,
            &vm,
            cert_path,
            inf_path,
            self.expected_version.as_deref(),
        ) {
            Ok(inst) => {
                deploy_progress.success("Driver installed");
                let wmi = if self.wmi && inst.published_name.is_some() {
                    let published = inst.published_name.as_ref().unwrap().clone();
                    let mut wmi_progress = progress(
                        format!("Collecting WMI metadata for {}", published),
                        show_progress,
                    );
                    match query_wmi_info(&vm, &published) {
                        Ok(info) => {
                            wmi_progress.success("Metadata retrieved");
                            Some(info)
                        }
                        Err(err) => {
                            wmi_progress.fail(err.to_string());
                            None
                        }
                    }
                } else {
                    None
                };

                // Collect debug messages if capture was enabled
                let debug_messages =
                    if let Some((mut session, mut backend)) = capture_session.take() {
                        let _ = backend.read_messages(&mut session);
                        let messages = backend.stop_capture(session).unwrap_or_else(|e| {
                            warn!("failed stopping debug capture: {}", e);
                            Vec::new()
                        });
                        if !json && !messages.is_empty() {
                            println!("\nCaptured {} debug messages", messages.len());
                        }
                        Some(messages)
                    } else {
                        None
                    };

                DeployResult {
                    success: true,
                    published_name: inst.published_name,
                    version: inst.version,
                    wmi,
                    debug_messages,
                    application_output: None,
                    inf_used: Some(inst.inf_used),
                    error: None,
                }
            }
            Err(e) => {
                deploy_progress.fail(e.to_string());
                let err = AppError::from(e);
                DeployResult {
                    success: false,
                    published_name: None,
                    version: None,
                    wmi: None,
                    debug_messages: None,
                    application_output: None,
                    inf_used: None,
                    error: Some(err.to_string()),
                }
            }
        };
        emit_deploy(&result, json);
        Ok(result)
    }
    pub fn run(&self, global_vm: Option<&str>, json: bool) -> Result<()> {
        // Optional test hook: use mock deployer if env var set
        let use_mock = std::env::var("DRIVER_TEST_CLI_MOCK").ok().as_deref() == Some("1");
        if use_mock {
            struct MockDeployer;
            impl DriverDeployer for MockDeployer {
                fn install_certificate(
                    &self,
                    _vm: &TestVm,
                    _cert: &Path,
                ) -> Result<(), crate::deploy::DeployError> {
                    Ok(())
                }
                fn install_driver(
                    &self,
                    _vm: &TestVm,
                    _inf: &Path,
                ) -> Result<crate::deploy::DriverInstallResult, crate::deploy::DeployError>
                {
                    Ok(crate::deploy::DriverInstallResult {
                        inf_used: "mock.inf".into(),
                        published_name: Some("oem123.inf".into()),
                        version: Some("1.2.3.4".into()),
                    })
                }
                fn verify_driver_version(
                    &self,
                    _vm: &TestVm,
                    _driver: &str,
                    _expected: &str,
                ) -> Result<(), crate::deploy::DeployError> {
                    Ok(())
                }
            }
            let res = self.execute(&MockDeployer, global_vm, json)?;
            return if res.success {
                Ok(())
            } else {
                anyhow::bail!(res.error.unwrap_or("deploy failed".into()))
            };
        }
        let real = PnpDeployer::default();
        let res = self.execute(&real, global_vm, json)?;
        if res.success {
            Ok(())
        } else {
            anyhow::bail!(res.error.unwrap_or("deploy failed".into()))
        }
    }
}
