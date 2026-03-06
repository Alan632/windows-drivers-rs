# Echo Driver Test Workflow

This example demonstrates a complete test cycle for the echo driver sample plus its companion application.

## Directory Layout

```
examples/echo-driver-test/
├─ driver/
│  ├─ Cargo.toml
│  ├─ src/
│  └─ driver.inf
├─ companion/
│  ├─ Cargo.toml
│  └─ src/main.rs
└─ echo_patterns.txt
```

The `echo_patterns.txt` file customizes the expected strings that must appear in both the driver debug output and the companion application stdout.

## Steps

```powershell
# 1. Build driver + companion
cargo build --release

# 2. Ensure VM baseline (one-time)
driver-test setup --vm-name echo-vm
driver-test snapshot --vm-name echo-vm --create

# 3. Run end-to-end validation (driver deploy + app interaction)
driver-test test --package-path . --vm-name echo-vm --revert-snapshot --capture-output
```

The `test` command will:

1. Detect the INF, driver type, and optional certificate.
2. Deploy the driver into the Hyper-V VM and verify the version.
3. Copy `target/release/echo-companion.exe` to `C:\\driver-test\\apps` inside the VM.
4. Execute the companion application with PowerShell Direct.
5. Validate the patterns listed in `echo_patterns.txt`.
6. Emit a combined summary containing driver metadata, debug output, and application stdout.

## Expected Result

Successful runs print:

- `Driver deployed. Published: Some("oem###.inf")` with version information.
- A debug output section listing at least the `sending packet` and `received packet` messages.
- A companion section with `exit code 0` and no missing patterns.

If any pattern is missing, the command exits with a non-zero status and lists the missing entries under "Missing patterns".
