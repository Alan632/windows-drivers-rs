# Error Handling & Recovery

## Error Categories

`driver-test-cli` distinguishes three primary error categories:

### 1. User Errors (Exit Code 1)
**Cause**: Invalid input, missing files, or misconfiguration  
**Recovery**: User action required

**Examples**:
- Driver package not found in current directory
- Invalid INF file format
- Missing required arguments
- VM name not found (suggest `setup` command)
- Unsupported driver type override

**Typical Messages**:
```
Error: Driver package not detected in current directory
Hint: Run from within a driver crate directory, or use --package-path
```

### 2. System Errors (Exit Code 2)
**Cause**: External system failures or unavailable prerequisites  
**Recovery**: Fix environment or retry

**Examples**:
- Hyper-V not enabled or inaccessible
- PowerShell Direct communication failure
- VM creation failed (resource limits)
- Insufficient disk space or memory
- Guest Integration Services not running

**Typical Messages**:
```
Error: Hyper-V PowerShell module not available
Hint: Enable Hyper-V feature: Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All
```

### 3. Operation Warnings (No Exit)
**Cause**: Recoverable issues or optional features unavailable  
**Recovery**: Operation continues with degraded functionality

**Examples**:
- Config file not found (uses defaults)
- Debug capture unavailable (continues without capture)
- WMI metadata query failed (deployment succeeds)
- Companion application not found (driver-only workflow)

**Typical Messages**:
```
Warning: Config file not found at config.toml, using defaults
Warning: Debug output capture unavailable, continuing without capture
```

---

## Error Recovery Workflows

### VM Creation Failures

**Symptom**: `setup` command fails with "Access denied" or "Hyper-V not available"

**Recovery Steps**:
1. Verify Hyper-V is enabled:
   ```powershell
   Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V
   ```
2. If disabled, enable and reboot:
   ```powershell
   Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All
   ```
3. Verify user is in "Hyper-V Administrators" group:
   ```powershell
   net localgroup "Hyper-V Administrators"
   ```
4. Retry `driver-test setup`

**VM Creation Hangs**:
- Check available RAM (needs ≥2GB free for default 2GB VM)
- Check disk space on Hyper-V default VHD location
- Review Hyper-V event logs: `Get-WinEvent -LogName Microsoft-Windows-Hyper-V-*`

---

### PowerShell Direct Failures

**Symptom**: "PowerShell Direct session could not be created"

**Common Causes**:
1. **Guest Integration Services not running**
   - Verify VM is fully booted: `Get-VM <name> | Select-Object State, IntegrationServicesState`
   - Expected: `IntegrationServicesState = "Up to date"`

2. **Credentials missing or invalid**
   - Verify VM has default credentials configured
   - Ensure Guest Service Interface integration component enabled

3. **VM network isolation**
   - PowerShell Direct uses VM bus (no network required)
   - Check integration services version matches host

**Recovery**:
```powershell
# Verify integration services
Get-VM "driver-test-vm" | Get-VMIntegrationService

# Enable Guest Service Interface
Get-VM "driver-test-vm" | Get-VMIntegrationService -Name "Guest Service Interface" | Enable-VMIntegrationService

# Restart VM if needed
Restart-VM "driver-test-vm"
```

---

### Driver Installation Failures

**Symptom**: `pnputil /add-driver` fails with "The hash for the file is not present in the specified catalog file"

**Cause**: Driver not properly signed or catalog file missing

**Recovery**:
1. Verify driver artifacts present:
   ```powershell
   ls target\x86_64-pc-windows-msvc\release\*.{sys,inf,cat,cer}
   ```
2. Check test certificate installed:
   ```powershell
   # Inside VM
   certutil -store Root | findstr "Test"
   ```
3. If catalog missing, rebuild with `cargo build --release`
4. If test cert missing, tool auto-installs on next deployment

**Device Not Starting After Install**:
- Check Device Manager status code
- Review driver logs in Event Viewer → System
- Verify hardware compatibility (virtual device availability)
- Check INF hardware ID matches virtual device

---

### Debug Output Capture Issues

**Symptom**: `--capture-output` flag produces no messages

**Common Causes**:
1. **DebugView not deployed to VM**
   - Tool copies DebugView.exe on first capture attempt
   - Verify `C:\driver-test\debugview.exe` exists in VM

2. **Driver not emitting debug output**
   - Verify driver uses `DbgPrint` (kernel) or `OutputDebugString` (user-mode)
   - Check kernel debugger attachment doesn't suppress DbgPrint

3. **Capture started after driver loaded**
   - Use `--revert-snapshot` to reset VM state before test
   - Capture session starts *before* driver deployment

**Troubleshooting**:
```powershell
# Inside VM - verify DebugView running
Get-Process -Name Dbgview -ErrorAction SilentlyContinue

# Check driver debug level registry
reg query HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print Filter
```

---

### Repository Detection False Positives

**Symptom**: Tool detects wrong repository type or fails to find driver package

**Recovery**:
1. **Explicitly specify driver type**:
   ```bash
   driver-test --driver-type KMDF
   ```

2. **Specify package path if not in driver directory**:
   ```bash
   driver-test --package-path ./crates/sample-kmdf
   ```

3. **Check repository structure**:
   - `windows-drivers-rs`: Drivers in `crates/*/`
   - `Windows-Rust-driver-samples`: Drivers in `general/*/`

**Known Limitation**: Mixed repository structures (both layouts present) may confuse detection. Use explicit `--package-path` in hybrid scenarios.

---

## Diagnostic Commands

### VM Health Check
```powershell
# Check VM existence and state
Get-VM "driver-test-vm" | Select-Object Name, State, IntegrationServicesState, CPUUsage, MemoryAssigned

# Verify snapshot present
Get-VM "driver-test-vm" | Get-VMSnapshot

# Test PowerShell Direct connectivity
Invoke-Command -VMName "driver-test-vm" -ScriptBlock { hostname }
```

### Driver Package Validation
```bash
# Verify driver artifacts present
ls target/x86_64-pc-windows-msvc/release/

# Check INF validity
cargo metadata --format-version 1 | jq '.packages[] | select(.name == "your-driver")'
```

### Debug Output Validation
```bash
# Run with maximum verbosity to see debug capture internals
driver-test -vvv --capture-output
```

---

## Error Message Interpretation

### Common Error Patterns

| Error Message | Root Cause | Recovery Action |
|---------------|------------|-----------------|
| `VM 'driver-test-vm' not found` | VM not created yet | Run `driver-test setup` |
| `PowerShell module 'Hyper-V' not available` | Hyper-V not enabled | Enable Hyper-V Windows feature |
| `Failed to parse INF file` | Corrupted or non-standard INF | Verify INF syntax with `InfVerif.exe` |
| `Driver version mismatch` | Stale VM state or old driver | Use `--revert-snapshot` to reset VM |
| `Certificate installation failed` | Insufficient VM permissions | Ensure VM admin credentials configured |
| `Companion application not found` | Package structure unexpected | Use `--package-path` with explicit path |
| `DebugView.exe transfer failed` | VM disk full or access denied | Check VM disk space, verify Guest Services |
| `Process execution timeout` | Application hung inside VM | Increase timeout in config or check app logs |

---

## Exit Code Reference

| Code | Meaning | Typical Scenario | User Action |
|------|---------|------------------|-------------|
| 0 | Success | All operations completed successfully | None (continue workflow) |
| 1 | User Error | Invalid arguments, missing files, config issue | Fix input or configuration |
| 2 | System Error | Hyper-V failure, VM unavailable, resource exhaustion | Fix environment prerequisites |

**Note**: Current implementation (v0.1.0) uses Rust's default `anyhow::Result` behavior (exit code 1 for all errors). Future versions will distinguish user vs. system errors per this table.

---

## Logging for Diagnostics

### Verbosity Levels

- **Default (no flags)**: Errors and warnings only
- **`-v`**: Info-level messages (operation progress)
- **`-vv`**: Debug-level (PowerShell commands, VM operations)
- **`-vvv`**: Trace-level (full PowerShell output, JSON parsing)

### Log Output Locations

**Console Output** (STDOUT):
- Deployment results (human-readable or JSON)
- Progress indicators
- Success confirmations

**Console Errors** (STDERR):
- All errors and warnings
- Tracing subscriber output (when `-v` flags used)

**VM Logs** (Guest):
- Debug output captured to `C:\debugview_<vmname>.log`
- Retrieved via PowerShell Direct after capture session

### Recommended Diagnostic Commands

**Basic Troubleshooting**:
```bash
driver-test -vv test
```

**Full Diagnostic Trace**:
```bash
driver-test -vvv --json test > output.json 2> trace.log
```

**Isolate PowerShell Issues**:
```powershell
# Manually test PowerShell Direct
Invoke-Command -VMName "driver-test-vm" -ScriptBlock { Get-Process }
```

---

## Support Resources

- **Troubleshooting Guide**: `docs/troubleshooting.md`
- **User Guide**: `docs/user-guide.md`
- **Installation Prerequisites**: `docs/installation.md`
- **Repository Issues**: https://github.com/wmmc88/windows-drivers-rs/issues

---

## Known Limitations & Workarounds

### Early Boot Debug Output
**Limitation**: DbgPrint messages emitted during driver initialization (before DebugView starts) are not captured.

**Workaround**: Use kernel debugger for early boot debugging, or add delay in driver entry point for testing.

### Multi-VM Parallelism
**Limitation**: v0.1.0 does not support parallel testing across multiple VMs.

**Workaround**: Run multiple `driver-test` instances sequentially with different `--vm-name` arguments.

### Network-Isolated VMs
**Limitation**: PowerShell Direct requires working VM integration services. Network isolation is supported, but VM must be fully booted and integration services operational.

**Workaround**: Ensure VM completes boot sequence before attempting deployment.

### Large Driver Packages
**Limitation**: File transfer via `Copy-VMFile` may be slow for large symbol files (PDB).

**Workaround**: Use network share or exclude PDB from deployment (debugging impact acceptable for functional testing).

---

*This guide is version-specific to v0.1.0. For updates and community contributions, see the repository's issue tracker.*
