# Troubleshooting Guide

## Debug Output Capture Issues

### No Messages Captured

**Symptoms**: `--capture-output` flag used but "Debug Output (0 messages)" shown

**Possible Causes**:

1. **Log file not created in VM**
   - Solution: Ensure debug capture infrastructure deployed to VM
   - Check: Look for `C:\debugview_{vm_name}.log` in VM

2. **Driver not outputting debug messages**
   - Solution: Add `DbgPrint` calls to your driver code
   - Example: `DbgPrint("MyDriver: Initialized\n");`

3. **Debug output buffering**
   - Solution: Wait a few seconds after driver loads before capturing
   - Use `Sleep()` or delay in test workflow

4. **Wrong VM name**
   - Solution: Verify VM name matches between deploy command and log file path
   - Check: `--vm-name` parameter

### Early Boot Messages Missing

**Symptoms**: Messages from `DriverEntry` not captured

**Explanation**: Debug capture starts after VM is ready; very early messages may be lost.

**Solutions**:
- Attach kernel debugger for boot-time debugging: `bcdedit /debug on`
- Use ETW for comprehensive event tracing
- Log to file from driver code as backup

### Message Classification Incorrect

**Symptoms**: Error messages classified as Info

**Cause**: Classification based on keywords ("error", "warn", "verbose")

**Solution**: Include keywords in your debug messages:
```c
DbgPrint("MyDriver: Error - Failed to allocate buffer\n");  // ✓ Classified as ERROR
DbgPrint("MyDriver: Allocation failed\n");                   // ✗ Classified as INFO
```

### Capture Session Timeout

**Symptoms**: Long-running captures stop receiving messages

**Cause**: Log file rotation dropped messages, or VM communication issue

**Solutions**:
- Increase `max_messages` limit (requires code change)
- Restart capture session periodically
- Check VM is still running: `Get-VM -Name "test-vm"`

## VM Communication Issues

### PowerShell Direct Not Available

**Symptoms**: "PowerShell Direct not available" errors

**Possible Causes**:

1. **VM not running**
   - Solution: `Start-VM -Name "test-vm"`

2. **Guest Service Interface disabled**
   - Solution: `Enable-VMIntegrationService -VMName "test-vm" -Name "Guest Service Interface"`

3. **Integration Services not installed**
   - Solution: Install Integration Services in guest OS
   - For Windows 10+: Built-in, may need Windows Update

4. **Hyper-V version too old**
   - Requirement: Windows 10 / Server 2016 or later
   - Solution: Upgrade Hyper-V host

### File Copy Failures

**Symptoms**: "Failed to copy file to VM" errors

**Solutions**:
- Verify VM is running and responsive
- Check disk space in VM: `Get-VMHardDiskDrive`
- Ensure paths use absolute paths, not relative
- Verify permissions on source files

### Command Execution Timeouts

**Symptoms**: "Command execution timed out" errors

**Causes**:
- VM is busy or unresponsive
- Command hung waiting for input
- Network/communication issue

**Solutions**:
- Increase timeout in code (default: 5 minutes)
- Check VM resource usage (CPU, memory)
- Restart VM if hung
- Use `Test-NetConnection` to verify VM network

## Driver Installation Issues

### Certificate Installation Fails

**Symptoms**: "Failed to install test certificate" errors

**Causes**:
1. **Test signing not enabled**
   - Solution: `bcdedit /set testsigning on` (requires reboot)

2. **Certificate file corrupt or wrong format**
   - Verify: `.cer` file, not `.pfx` or `.p12`
   - Regenerate certificate if needed

3. **Insufficient permissions**
   - Solution: Run with Administrator privileges

### Driver Load Fails

**Symptoms**: pnputil succeeds but driver not loaded

**Checks**:
1. **Device Manager**: Look for yellow exclamation mark
2. **Event Viewer**: Check System log for driver errors
3. **pnputil enumeration**: `pnputil /enum-drivers`

**Common Causes**:
- Missing dependencies (DLLs, other drivers)
- Incompatible driver signature
- Hardware/device not present
- Driver coded for different Windows version

### Version Mismatch

**Symptoms**: "Version mismatch: expected X, got Y"

**Causes**:
- Old driver still cached by Windows
- Multiple driver versions installed
- Build output not updated

**Solutions**:
- Revert VM to baseline snapshot: `driver-test snapshot --revert`
- Uninstall old driver: `pnputil /delete-driver oem123.inf`
- Clean build: `cargo clean && cargo build`
- Verify version in INF file matches binary

## VM Management Issues

### VM Creation Fails

**Symptoms**: "Failed to create VM" errors

**Checks**:
1. **Hyper-V enabled**: `Get-WindowsOptionalFeature -FeatureName Microsoft-Hyper-V -Online`
2. **Administrator rights**: Run PowerShell as Administrator
3. **Disk space**: Ensure sufficient space for VHD
4. **Memory available**: Check system has enough free RAM

### Snapshot Operations Fail

**Symptoms**: "Failed to create/revert snapshot" errors

**Solutions**:
- Stop VM before snapshot operations
- Check disk space on Hyper-V host
- Remove old snapshots if limit reached
- Verify VM exists: `Get-VM -Name "test-vm"`

### VM Cleanup Fails

**Symptoms**: "Failed to remove VM" errors

**Solutions**:
- Force stop VM first: `Stop-VM -Name "test-vm" -Force`
- Manually delete via Hyper-V Manager if CLI fails
- Remove VHD files from `C:\ProgramData\Microsoft\Windows\Hyper-V\`

## Build Issues

### Cargo Build Fails

**Symptoms**: "Build failed" errors during test workflow

**Solutions**:
- Verify cargo-wdk installed: `cargo install cargo-wdk`
- Check Rust toolchain updated: `rustup update`
- Review build errors in output
- Try manual build: `cargo build --release`

### INF File Not Found

**Symptoms**: "INF file not found" errors

**Checks**:
- Verify INF exists in expected location
- Check build output directory structure
- Use `--inf` flag to specify exact path

## Repository Detection Issues

### Wrong Repository Type Detected

**Symptoms**: Tool assumes `windows-drivers-rs` layout when running inside `windows-rust-driver-samples` (or vice versa); build artifacts copied from unexpected directories.

**Checks & Solutions**:
1. **Marker files present?** The detector looks for `.samples-root`, `samples.json`, `sample-list.json`, or `Samples.props` in ancestor folders. Ensure at least one exists in the samples repo root.
2. **Folder name verification**: Root folder should be named `windows-rust-driver-samples` (case-insensitive). Renaming the directory can break auto-detection.
3. **Run from the driver crate**: The current working directory must be the driver Cargo package (e.g., `general/echo/kmdf/driver`). Running from higher-level folders may confuse INF search heuristics.
4. **Manual override**: Until detection is fixed, use `--driver-type <KMDF|UMDF|WDM>` and `--inf <path>` to unblock testing.
5. **Diagnostics**: `driver-test --json detect` (or `cargo test samples_repo_detection_identifies_layout`) helps confirm what RepositoryType the code resolved. See `docs/repository-detection.md` for heuristic details.

## General Tips

### Enable Verbose Logging

Use `-vvv` flag for maximum verbosity:
```bash
driver-test -vvv deploy --inf driver.inf --capture-output
```

### Check Logs

Review trace logs for detailed error information.

### Test Incrementally

1. Test VM creation separately: `driver-test setup`
2. Test file copy separately: use PowerShell `Copy-VMFile`
3. Test driver install manually in VM first
4. Then automate with CLI

### Clean State

When in doubt, start fresh:
```bash
driver-test clean --yes
driver-test setup
driver-test snapshot --create
```

## Getting Help

If issues persist:

1. Review specification: [spec.md](../specs/001-driver-test-tools/spec.md)
2. Check implementation plan: [plan.md](../specs/001-driver-test-tools/plan.md)
3. Review task list: [tasks.md](../specs/001-driver-test-tools/tasks.md)
4. Enable verbose logging and capture output
5. Check Windows Event Viewer (System and Application logs)
6. File an issue with:
   - Full command used
   - Complete error output
   - `-vvv` verbose logs
   - VM state and configuration
   - Host OS and Hyper-V version
