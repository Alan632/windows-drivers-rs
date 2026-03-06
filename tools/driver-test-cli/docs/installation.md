# Installation & Environment Setup

Follow this guide to prepare a development workstation for `driver-test-cli`.

## 1. Host Requirements
- Windows 10/11 Pro or Enterprise (Hyper-V capable)
- Administrator PowerShell session
- ≥16 GB RAM recommended (host + VM)
- SSD storage for VM disk images

## 2. Enable Hyper-V
```powershell
Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All -NoRestart
Restart-Computer
```
Verify installation:
```powershell
Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V
```

## 3. Install Toolchains
1. **Rust toolchain**
   ```powershell
   winget install Rustlang.Rustup -s winget
   rustup default stable
   rustup component add rustfmt clippy
   ```
2. **cargo-wdk** (for KMDF/UMDF builds)
   ```powershell
   cargo install cargo-wdk
   ```
3. **Visual Studio Build Tools** (C++ workload)
   ```powershell
   winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --norestart"
   ```

## 4. Clone Repositories
```powershell
mkdir d:\git-repos
cd d:\git-repos
# Core driver repo
git clone https://github.com/microsoft/windows-drivers-rs.git
# Optional samples repo
git clone https://github.com/microsoft/Windows-Rust-driver-samples.git
```

## 5. Build the CLI
```powershell
cd d:\git-repos\github\windows-drivers-rs.git\driver-deploy-test-tool\tools\driver-test-cli
cargo build --release
```
Optional global install:
```powershell
cargo install --path .
```
Ensure `%USERPROFILE%\.cargo\bin` is on `PATH` (Rust installer usually configures this automatically).

## 6. Prepare Hyper-V Test VM
1. Create a VM with Windows 10/11 guest OS:
   ```powershell
   driver-test-cli setup --vm-name driver-test-vm --memory-mb 4096 --cpu-count 4
   ```
2. Install Windows using an ISO, enable Integration Services, and turn on Guest Service Interface:
   ```powershell
   Enable-VMIntegrationService -VMName driver-test-vm -Name "Guest Service Interface"
   ```
3. Configure baseline environment inside the VM:
   - Install Visual Studio Build Tools (if compiling inside guest)
   - Enable test signing: `bcdedit /set testsigning on`
   - Install DebugView or dependent tools as needed
4. Capture a baseline snapshot:
   ```powershell
   driver-test-cli snapshot --create
   ```

## 7. Verify PowerShell Direct
Run a simple command:
```powershell
Invoke-Command -VMName driver-test-vm -ScriptBlock { hostname }
```
If it fails, ensure the VM is running and Integration Services are enabled.

## 8. First Test Run
```powershell
cd windows-drivers-rs\examples\sample-kmdf-driver
driver-test-cli test --capture-output --revert-snapshot
```
You should see build output, driver deployment logs, and (if enabled) debug messages.

## 9. Environment Variables (Optional)
- `DRIVER_TEST_VM_NAME` – override default VM name in tests/integration suites
- `DRIVER_TEST_CLI_MOCK=1` – force CLI to use mock deployer for local dry runs

Your workstation is now ready for iterative driver testing with the CLI.
