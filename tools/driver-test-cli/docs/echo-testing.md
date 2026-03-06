# Echo Driver End-to-End Testing

This guide explains how to exercise the echo driver sample (and any driver with a companion application) using `driver-test`.

## Prerequisites

1. Hyper-V host configured with a baseline VM (`driver-test setup` + `driver-test snapshot --create`).
2. Driver package that builds both the driver (INF/SYS) and a companion executable (e.g., `echo-app`).
3. The companion app binary appears under `target/release/<name>.exe` or inside a `bin/` or `exe/` folder near the package root.

## Workflow

```bash
# From the driver package directory
cargo build --release

driver-test test --capture-output --revert-snapshot
```

The `test` command now:

1. Detects the driver type, INF path, and optional version.
2. Builds the package (`cargo build --release`).
3. Ensures the Hyper-V VM is running (optionally reverting the baseline snapshot).
4. Deploys the driver via `pnputil` and verifies the version.
5. Detects `echo-app.exe` (or any companion executable) and copies it to `C:\\driver-test\\apps` inside the VM.
6. Executes the companion application with PowerShell Direct.
7. Captures stdout/stderr plus driver debug output (when `--capture-output` is enabled).
8. Verifies that required patterns (default: `"echo: sending packet"`, `"echo: received packet"`) appear in the application logs.

## Expected Output

```
Driver deployed. Published: Some("oem42.inf") Version: Some("1.0.0.0")

Debug Output (12 messages):
  [INFO] MyDriver: echo: sending packet
  [INFO] MyDriver: echo: received packet

Companion Application Output (exit code 0):
  Stdout: echo: sending packet
          echo: received packet
```

If any expected pattern is missing, the command fails with a descriptive error listing the missing entries.

## Customizing Patterns

Place a `echo_patterns.txt` (or `companion_patterns.txt`) file in the driver package root with one pattern per line to override the defaults:

```
# echo_patterns.txt
Ping started
Ping response
```

The loader trims blank lines and comments (`#` prefix).

## Troubleshooting

- **Companion app not detected**: Ensure the executable exists under `target/release`, `bin/`, `exe/`, or `apps/` and rerun `cargo build --release`.
- **Copy failure**: Check Hyper-V Guest Services, Integration Services, and VM connectivity. See `docs/troubleshooting.md` for details.
- **Pattern mismatch**: Review stdout/stderr (printed in the summary) and adjust `echo_patterns.txt` if needed.
- **Debug log empty**: Confirm `--capture-output` flag and that `C:\\debugview_<vm>.log` is writable.

For more examples, see `examples/echo-driver-test/`.
