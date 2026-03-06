# driver-test-cli

Rust CLI to automate Windows driver deployment and verification in Hyper-V test VMs.

## Status
**Phase 6 Complete** – Cross-repository testing support (windows-drivers-rs + Windows-Rust-driver-samples) with end-to-end CLI workflows, debug capture, and companion application validation.

**Highlights:**
- ✅ Phases 1-3: Deployment infrastructure, VM lifecycle, CLI command surface
- ✅ Phase 4: Debug output capture with validation and log rotation
- ✅ Phase 5: Echo companion workflow with output correlation
- ✅ Phase 6: Repository detection heuristics, samples-aware INF search, and dedicated docs/tests
- ✅ Driver deployment orchestration (`deploy` module) with pnputil + WMI enrichment
- ✅ Dependency injection for testable architecture (mock deployer + Hyper-V provider)
- ✅ JSON output support (`--json` flag) for CI integration
- ✅ 27 passing tests (unit, integration, doc) + Hyper-V suites (ignored by default)

## Features

### Deployment Module
- **Certificate Installation**: Import test signing certificates to guest VM
- **Driver Installation**: Deploy driver packages via `pnputil /add-driver`
- **Version Verification**: Parse `pnputil /enum-drivers` for exact version matching
- **WMI Enrichment**: Query Win32_PnPSignedDriver for device metadata (manufacturer, signer, etc.)
- **Progress Indicators**: 3-step deployment workflow logging
- **Testable Design**: `DriverDeployer` trait with production/mock implementations

### VM Management
- **Setup**: Create and configure Hyper-V test VMs
- **Snapshot**: Create/revert baseline snapshots for clean state
- **Clean**: Remove test VMs with confirmation
- **State Management**: Ensure VM running state, query VM properties

### CLI Commands
```powershell
# Create test VM
driver-test-cli setup --vm-name driver-test-vm --memory-mb 4096 --cpu-count 4

# Create baseline snapshot
driver-test-cli snapshot --create

# Deploy driver with WMI enrichment
driver-test-cli deploy --vm driver-test-vm --cert driver.cer --inf driver.inf --version 1.0.0.0 --wmi --json

# Run test workflow (build → deploy → verify)
driver-test-cli test --revert-snapshot --capture-output

# Revert to baseline
driver-test-cli snapshot --revert

# Clean up test environment
driver-test-cli clean --yes
```

## Quick Start

### Build
```powershell
cd tools/driver-test-cli
cargo build --release
```

**Prerequisites**: Visual Studio Build Tools with C++ workload
```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --norestart"
```

### Test
```powershell
cargo test                    # All tests (ignores Hyper-V integration)
cargo test --doc             # Documentation examples only
cargo doc --open             # Browse API documentation

# Run real Hyper-V integration tests (requires test VM)
cargo test --test hyperv_integration -- --ignored
```

**Integration Test Requirements:**
- Windows test VM named "driver-test-vm" (or set `DRIVER_TEST_VM_NAME`)
- VM in Running state with Integration Services enabled
- Guest Service Interface enabled for file copy tests
- Run with elevated privileges (Hyper-V access)

### Mock Testing
```powershell
# Enable mock deployer for testing without Hyper-V
$env:DRIVER_TEST_CLI_MOCK = "1"
driver-test-cli deploy --vm mock --inf test.inf
```

## User Guide

### Initial Setup

1. **Create test VM:**
```powershell
# Create VM with default settings (2GB RAM, 2 CPUs)
driver-test-cli setup

# Custom configuration
driver-test-cli setup --vm-name my-test-vm --memory-mb 4096 --cpu-count 4
```

2. **Install Windows and configure VM:**
   - Install Windows on the VM
   - Enable Integration Services (Guest Service Interface)
   - Start the VM: `Start-VM -Name driver-test-vm`

3. **Create baseline snapshot:**
```powershell
driver-test-cli snapshot --create
```

### Development Workflow

**Iterative Testing Cycle (<5min):**
```powershell
# 1. Revert to clean state
driver-test-cli snapshot --revert

# 2. Build and deploy driver
driver-test-cli test --package-path ./my-driver --revert-snapshot

# 3. Deploy driver with verification
driver-test-cli deploy --inf ./my-driver/target/release/driver.inf --cert ./certs/test.cer --version 1.0.0.0

# 4. View enriched metadata
driver-test-cli deploy --inf ./my-driver/target/release/driver.inf --wmi --json
```

**Query WMI Metadata:**
```powershell
driver-test-cli deploy --inf driver.inf --wmi
# Output:
#   Device: "My Test Device"
#   Manufacturer: "Contoso"
#   Provider: "Microsoft Corporation"
#   Signed: true by "Microsoft Code Signing PCA"
```

### Troubleshooting

**VM Not Found:**
```powershell
# List available VMs
Get-VM

# Create test VM if missing
driver-test-cli setup --vm-name driver-test-vm
```

**Integration Services Not Available:**
- Ensure VM has Guest Service Interface enabled
- Check VM settings: `Get-VMIntegrationService -VMName driver-test-vm`
- Enable: `Enable-VMIntegrationService -VMName driver-test-vm -Name "Guest Service Interface"`

**Certificate Import Fails:**
- Verify certificate file exists and is valid .cer format
- Check VM is in Running state
- Ensure PowerShell Direct connectivity

**WMI Query Returns Empty:**
- Driver must be installed and enumerated by pnputil first
- Run without `--wmi` flag first to confirm installation
- Check driver is loaded: `pnputil /enum-drivers` in guest VM

**Test Failures:**
- Check VM state: `Get-VM -Name driver-test-vm | Select-Object State`
- Verify Integration Services: `Get-VMIntegrationService -VMName driver-test-vm`
- Run with verbose logging: `driver-test-cli -vvv deploy ...`

**Performance Issues:**
- Use SSD for VM storage
- Allocate sufficient memory (≥4GB recommended)
- Enable nested virtualization if testing in nested environment

## Architecture

```
deploy_driver (orchestration)
    ↓
DriverDeployer (trait)
    ↓
├─► PnpDeployer (production: PowerShell Direct)
└─► MockDeployer (testing: in-memory stub)

query_wmi_info (metadata enrichment)
    ↓
PowerShell: Get-WmiObject Win32_PnPSignedDriver
    ↓
WmiInfo (device, manufacturer, signer, provider)
```

## Documentation

- **Installation**: `docs/installation.md` – Host requirements, Hyper-V enablement, first-run checklist
- **User Guide**: `docs/user-guide.md` – Command reference, workflows, JSON output schema
- **Troubleshooting**: `docs/troubleshooting.md` – Common VM, deployment, and repository detection issues
- **Repository Detection**: `docs/repository-detection.md` – Heuristics for multi-repo layouts
- **Parser Notes**: `docs/parser-notes.md` – pnputil parsing strategy
- **Release Checklist**: `docs/release.md`
- **CHANGELOG**: `CHANGELOG.md`
- **API Docs**: `cargo doc --open` – Comprehensive rustdoc with examples

## Testing

- **Unit Tests**: `tests/pnputil_parse.rs` - Parser validation
- **Integration Tests**: `tests/deploy_integration.rs` - Mock deployer workflows
- **CLI Tests**: `tests/deploy_cli.rs` - End-to-end JSON output
- **Doc Tests**: Embedded examples in rustdoc (validated on every build)
- **Hyper-V Integration**: `tests/hyperv_integration.rs` - Real VM operations (ignored by default)
  - VM query and state management
  - Snapshot create/revert workflows
  - PowerShell Direct command execution
  - Guest file copy operations
  - Complete deployment workflow validation

## Phase 3 Roadmap

**Completed:**
- ✅ Real Hyper-V integration test suite (environment-gated, 6 tests)
- ✅ WMI metadata enrichment (Win32_PnPSignedDriver integration)
- ✅ Complete CLI command implementations (setup, snapshot, clean, test)
- ✅ User guide with troubleshooting section
- ✅ Enhanced deployment workflow with progress indicators
- ✅ Performance optimization (OnceCell-based pnputil enumeration caching)

**Performance Characteristics:**
- **Deployment with WMI**: ~5-10 seconds (first call), ~3-5 seconds (cached)
- **Version verification**: ~1-2 seconds (first call), <100ms (cached)
- **Cache lifetime**: Per-VM, per-PnpDeployer instance
- **Memory overhead**: Minimal (~10KB per cached VM enumeration)

**Future Enhancements:**
- Advanced test scenarios (stress testing, multi-driver deployment)
- Debug capture automation (WPP trace integration)
- Echo test suite for driver I/O validation
