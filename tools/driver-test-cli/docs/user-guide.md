# Driver Test CLI – User Guide

The Driver Test CLI automates Windows driver build, deployment, and validation in a Hyper-V test environment. This guide summarizes topology assumptions, commands, and recommended workflows for both `windows-drivers-rs` and `windows-rust-driver-samples` repositories.

## Prerequisites

- Windows 10/11 Pro or Enterprise with Hyper-V enabled
- Administrator privileges for PowerShell sessions
- Rust toolchain (`rustup`) plus `cargo-wdk` for KMDF/UMDF builds
- Hyper-V test VM prepared with Integration Services (Guest Service Interface) and a baseline snapshot
- Host access to the driver source tree (either windows-drivers-rs or windows-rust-driver-samples)

See `docs/installation.md` for full setup steps.

## Global CLI Flags

| Flag                | Description                                                     |
| ------------------- | --------------------------------------------------------------- |
| `--json`            | Emit structured JSON output (suitable for CI)                   |
| `-v`, `-vv`, `-vvv` | Increase logging verbosity (info, debug, trace)                 |
| `--vm-name <NAME>`  | Override default VM name (`driver-test-vm`) for all subcommands |

## Commands Overview

### `driver-test-cli setup`
Create or reuse a Hyper-V VM for driver testing.

Key options:
- `--vm-name <NAME>` – assign a custom VM identifier
- `--memory-mb <MB>` – RAM allocation (default 2048)
- `--cpu-count <N>` – virtual CPU count (default 2)
- `--disk-gb <GB>` – virtual disk size (default 60 GB)

Typical flow:
1. `driver-test-cli setup --vm-name driver-test-vm --memory-mb 4096 --cpu-count 4`
2. Install Windows in the VM, enable Integration Services, and create a baseline snapshot.

### `driver-test-cli snapshot`
Manage the baseline snapshot used for deterministic test cycles.

- `--create` – capture a new baseline (VM must be prepared and shut down)
- `--revert` – roll the VM back to the snapshot before testing

Example:
```powershell
# Capture after configuring the VM once
driver-test-cli snapshot --create
# Before every driver iteration
driver-test-cli snapshot --revert
```

### `driver-test-cli test`
End-to-end workflow: build driver, deploy via pnputil, verify version, optionally run companion applications, and capture debug output.

Important options:
- `--package-path <path>` – driver crate root (defaults to current directory)
- `--driver-type <KMDF|UMDF|WDM>` – override detected type
- `--revert-snapshot` / `--rebuild-vm` – full VM hygiene controls
- `--capture-output` – stream DebugView output during the run

For `windows-rust-driver-samples`, the command automatically adapts build output paths and INF discovery based on repository markers.

### `driver-test-cli deploy`
Deploy a specific INF (with optional certificate) to the VM without rebuilding the crate.

Options:
- `--inf <path>` – required INF path
- `--cert <path>` – optional `.cer` to install beforehand
- `--expected-version <x.y.z.w>` – cause the tool to verify pnputil-reported version
- `--wmi` – enrich output with `Win32_PnPSignedDriver` metadata
- `--capture-output` – capture debug output for the deployment window

### `driver-test-cli clean`
Unmount and delete the Hyper-V test VM. Use `--yes` to bypass confirmation.

## Recommended Workflows

### Iterative Driver Validation

```powershell
# Start each cycle from a clean snapshot
driver-test-cli snapshot --revert

# Run the automated workflow from the driver crate root
cd windows-drivers-rs/examples/sample-kmdf-driver
cargo build --release  # optional, test command will build if needed
driver-test-cli test --revert-snapshot --capture-output
```

Outputs:
- Deployment progress + pnputil results
- Optional DebugView transcript
- Optional companion application output (echo sample)
- WMI metadata when available

### Companion Application Testing

For drivers with a companion executable (e.g., echo sample):

1. Layout the application in `target/release`, `bin/`, or repository-specific app folders.
2. `driver-test-cli test --capture-output` automatically copies the executable into the VM.
3. The CLI runs the app, captures stdout, and validates echo semantics, reporting mismatches.

### Samples Repository Usage (`windows-rust-driver-samples`)

- Run commands from the driver crate (`general/echo/kmdf/driver`).
- The tool detects the samples repo using marker files (`samples.json`, `.samples-root`, etc.).
- Build outputs resolve to `target/wdk/<arch>/Release` when present.
- INF search automatically walks the `inf/` and `deployment/package/` folders.

## JSON Output Schema

`--json` returns a single JSON document matching `DeployResult`:

```json
{
  "success": true,
  "published_name": "oem123.inf",
  "version": "2.3.4.5",
  "wmi": {
    "device": "Contoso Echo", 
    "manufacturer": "Contoso", 
    "provider": "Microsoft", 
    "is_signed": true
  },
  "debug_messages": [
    { "level": "info", "source": "driver", "message": "echo: sending packet" }
  ],
  "application_output": {
    "stdout": ["ping", "pong"],
    "stderr": []
  },
  "error": null
}
```

Use this in CI to gate on driver deployment success or to archive debug evidence.

## Troubleshooting & Support

- `docs/troubleshooting.md` – common VM, deployment, and repository-detection issues
- `docs/repository-detection.md` – heuristic details for multi-repo support
- `docs/error-handling.md` (coming soon) – remediation playbooks for installer failures

For reproducible bug reports include: CLI command, `-vvv` logs, host OS, VM configuration, and output artifacts.
