# Debug Output Capture

## Overview

The driver test CLI supports capturing debug output from drivers running in the test VM. This feature allows developers to monitor `DbgPrint` (kernel-mode) and `OutputDebugString` (user-mode) messages without manually attaching a kernel debugger.

## Usage

Enable debug output capture with the `--capture-output` flag:

```bash
driver-test deploy --inf path/to/driver.inf --capture-output
```

## How It Works

The debug capture system:

1. **Log File Creation**: Creates a log file in the VM at `C:\debugview_{vm_name}.log`
2. **Real-Time Polling**: Polls the log file periodically to capture new messages
3. **Message Classification**: Automatically classifies messages by level (Info, Warning, Error, Verbose)
4. **Source Extraction**: Attempts to extract the source component from message prefixes

## Message Levels

Messages are classified based on content:

- **ERROR**: Contains "error" or "fail"
- **WARN**: Contains "warn" or "deprecated"  
- **VERBOSE**: Contains "verbose" or "trace"
- **INFO**: All other messages (default)

## Output Format

### Human-Readable Output

```
Driver deployed. Published: "oem123.inf" Version: "1.0.0.0"

Debug Output (15 messages):
  [INFO] MyDriver: Driver initialized successfully
  [WARN] MyDriver: Buffer size approaching limit
  [ERROR] MyDriver: Failed to allocate DMA buffer
  ... and 12 more messages
```

### JSON Output

Use `--json` flag for machine-readable output:

```json
{
  "success": true,
  "published_name": "oem123.inf",
  "version": "1.0.0.0",
  "debug_messages": [
    {
      "message": "MyDriver: Driver initialized successfully",
      "level": "info",
      "source": "MyDriver"
    }
  ]
}
```

## Limitations

### Early Boot Messages

**Issue**: Debug messages printed during very early driver initialization (before the VM's debug capture infrastructure is ready) may be missed.

**Workaround**:
- Use `DbgPrint` after `DriverEntry` completes
- Consider using ETW (Event Tracing for Windows) for comprehensive boot logging
- Attach a kernel debugger for complete early-boot capture

### Message Buffering

**Issue**: Messages may be buffered by the OS before being written to the log file.

**Impact**: Small delay between when driver prints message and when capture sees it (typically <200ms).

### Maximum Message Limit

**Default**: 1000 messages retained in memory  
**Behavior**: Older messages are dropped when limit is exceeded (log rotation)

**Configuration**: Cannot be changed via CLI (hardcoded in `CaptureConfig::default()`)

## Performance Impact

- **Minimal**: Polling occurs every 200ms by default
- **File I/O**: Reading log file on each poll cycle
- **Memory**: ~1KB per message retained

## Pattern Validation

Verify expected messages appear in output:

```rust
use driver_test_cli::debug::validate_output_patterns;

let patterns = vec![
    "Driver initialized".to_string(),
    "Device ready".to_string(),
];

let missing = validate_output_patterns(&messages, &patterns);
if !missing.is_empty() {
    println!("Missing expected patterns: {:?}", missing);
}
```

## Troubleshooting

See [troubleshooting.md](./troubleshooting.md) for common debug output capture issues.

## Future Enhancements

- [ ] ETW-based capture for kernel events
- [ ] Configurable polling interval and max messages
- [ ] Real-time streaming output (non-polling)
- [ ] Timestamp synchronization with host
- [ ] Filter by message level in CLI
- [ ] Export to standard log formats (JSON lines, CSV)
