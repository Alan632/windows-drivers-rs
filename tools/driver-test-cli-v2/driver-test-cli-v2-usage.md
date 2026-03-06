# driver-test-cli-v2 Usage Guide

A single-command CLI that smoketests a pre-built Windows driver package on a local Hyper-V VM.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Command Reference](#command-reference)
- [Examples](#examples)
- [Pipeline Steps](#pipeline-steps)
- [Driver Package Requirements](#driver-package-requirements)
- [Exit Codes](#exit-codes)
- [Output Formats](#output-formats)
- [Environment Variables](#environment-variables)
- [CI/CD Integration](#cicd-integration)
- [Troubleshooting](#troubleshooting)

---

## Installation

### Build from source

```powershell
cd tools\driver-test-cli-v2
cargo build --release
```

The binary is at `target\release\driver-test-cli-v2.exe`.

### Prerequisites

| Requirement | Why |
|-------------|-----|
| Windows host with Hyper-V enabled | VM management and PowerShell Direct |
| Administrator privileges | Required for Hyper-V cmdlets |
| A running Hyper-V VM with Windows | Target for driver installation |
| WDK installed on host | Provides `devgen.exe` for device creation (discovered via registry; overridable with `DEVGEN_PATH`) |
| Driver package (.inf + .sys) | The driver to test |

---

## Quick Start

**Simplest usage** -- smoketest a driver on the default VM (`driver-test-vm`):

```powershell
driver-test-cli-v2 --driver-package C:\path\to\my_driver_package
```

**With explicit VM name:**

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    --vm-name my-test-vm
```

**With custom output directory for logs:**

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    --output-dir C:\logs\smoketest-output
```

---

## Command Reference

```
driver-test-cli-v2 [OPTIONS] --driver-package <PATH>
```

### Required Arguments

| Argument | Description |
|----------|-------------|
| `--driver-package <PATH>` | Path to the driver package directory. Must contain at least one `.inf` and one `.sys` file. |

### VM Target Options

| Option | Default | Description |
|--------|---------|-------------|
| `--vm-name <NAME>` | `driver-test-vm` | Name of the Hyper-V VM to target. |

> **Credentials:** The tool uses hardcoded test credentials (`DriverTestAdmin` / `1234`).
> There are no CLI options or environment variables for credentials.

### Pipeline Control Options

| Option | Default | Description |
|--------|---------|-------------|
| `--skip-verify` | off | Skip post-install driver verification. |
| `--skip-capture` | off | Skip log capture (both starting and collecting). |
| `--capture-duration <SECS>` | `30` | How many seconds to capture ETW traces. |
| `--no-cleanup` | off | Leave temporary files and scripts on the VM after completion. |
| `--output-dir <PATH>` | `%TEMP%\driver-test-logs` | Host directory where captured log files will be placed. |

### Output Options

| Option | Default | Description |
|--------|---------|-------------|
| `--json` | off | Print results as JSON instead of human-readable text. |
| `-v` | warn | Verbosity: `-v` = info, `-vv` = debug, `-vvv` = trace. |

### Info Options

| Option | Description |
|--------|-------------|
| `-h, --help` | Print help and exit. |
| `-V, --version` | Print version and exit. |

---

## Examples

### 1. Basic smoketest (all steps)

Runs the full pipeline: start log capture, install, verify, collect logs.

```powershell
driver-test-cli-v2 --driver-package .\target\debug\sample_kmdf_driver_package
```

### 2. Specify a VM

Target a specific Hyper-V VM instead of the default (`driver-test-vm`):

```powershell
driver-test-cli-v2 `
    --driver-package C:\drivers\my_driver_package `
    --vm-name my-test-vm
```

### 3. Install only (skip verify and capture)

Just install the driver without checking if it loaded or capturing traces:

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    --skip-verify `
    --skip-capture
```

### 4. Quick capture with shorter duration

Reduce trace capture to 10 seconds for faster feedback:

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    --capture-duration 10
```

### 5. Custom output directory for logs

Save captured logs to a specific directory instead of the default (`%TEMP%\driver-test-logs`):

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    --output-dir C:\logs\smoketest-output
```

### 6. JSON output for CI pipelines

```powershell
driver-test-cli-v2 `
    --driver-package .\target\release\my_driver_package `
    --vm-name ci-test-vm `
    --json
```

### 7. Verbose output for debugging

See detailed tracing of what each step is doing:

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    -vv
```

### 8. Keep temporary files after completion

Leave scripts and files on the VM for manual investigation:

```powershell
driver-test-cli-v2 `
    --driver-package .\target\debug\sample_kmdf_driver_package `
    --no-cleanup
```

### 9. Full pipeline with all options

```powershell
driver-test-cli-v2 `
    --driver-package C:\build\output\my_driver_package `
    --vm-name driver-test-vm `
    --capture-duration 60 `
    --output-dir C:\test-results\logs `
    --no-cleanup `
    -v
```

### 10. Using the sample KMDF driver from this repo

```powershell
# Build the sample driver first
cargo make --cwd examples\sample-kmdf-driver

# Run the smoketest
driver-test-cli-v2 `
    --driver-package examples\sample-kmdf-driver\target\debug\sample_kmdf_driver_package `
    --vm-name driver-test-vm
```

---

## Pipeline Steps

The tool runs 4 steps in sequence. The pipeline **stops on the first failure** -- any
remaining steps are marked as **Skipped** in the output.

```
Step  What it does
----  ------------
  1   Start log capture (DebugView + ETW)
  2   Install driver package (copy files, import certs, pnputil)
  3   Verify driver on VM (check binding, devgen if needed)
  4   Collect captured logs (stop traces, copy to host)
```

### Step 1: Start Log Capture

- Launches DebugView and starts an ETW trace session on the VM
- Captures for the configured duration (default 30 seconds)
- **Skip with:** `--skip-capture`

### Step 2: Driver Installation

- Copies all driver files (.inf, .sys, .cat, .cer, .pdb) to the VM
- Imports test certificates using `Import-Certificate` with thumbprint verification
- Enables test signing mode and reboots if needed
- Installs via `pnputil /add-driver /install` with a 120-second timeout
- **Always runs** (cannot be skipped)

### Step 3: Driver Verification

- Parses the .inf to extract expected version, provider, and hardware IDs
- Queries the VM for a matching device via `Get-PnpDeviceProperty`
- If no device exists, uses `devgen.exe` to create a device node
- If the wrong driver is bound, removes it and triggers re-binding
- When re-installing, VM credentials are automatically forwarded to the subprocess (no interactive prompts in CI)
- `devgen.exe` is located via WDK registry lookup; override with the `DEVGEN_PATH` environment variable
- **Skip with:** `--skip-verify`

### Step 4: Collect Captured Logs

- Stops the DebugView and ETW sessions started in Step 1
- Decodes the ETL file to XML via `tracerpt`
- Copies ETL, XML, summary, and PDB to the host output directory (default `%TEMP%\driver-test-logs`; override with `--output-dir`)
- **Skip with:** `--skip-capture`

---

## Driver Package Requirements

Your `--driver-package` directory must contain:

```
my_driver_package/
  *.inf          (REQUIRED - driver installation manifest)
  *.sys          (REQUIRED - driver binary)
  *.cat          (optional - catalog file for signature)
  *.cer          (optional - test-signing certificate)
  *.pdb          (optional - debug symbols for trace decoding)
```

The tool validates this layout before starting and exits immediately with a clear error if `.inf` or `.sys` files are missing.

---

## Exit Codes

| Code | Meaning | When |
|------|---------|------|
| `0` | **Pass** | All steps succeeded. Driver is installed, verified, and traces captured. |
| `1` | **Test failure** | Driver issue -- install failed, version mismatch, verification failed. |
| `2` | **Infrastructure error** | Construction-time or runtime infrastructure problem. |

Exit code `2` examples:

- **Construction-time:** Invalid `--driver-package` path, missing `.inf`/`.sys` files, invalid argument combinations.
- **Runtime:** VM not found or unreachable, PowerShell Direct session failures, script timeouts, I/O errors copying files to the VM.

---

## Output Formats

### Interactive (default)

```
driver-smoketest v0.1.0
  Package:  C:\drivers\sample_kmdf_driver_package\
  VM:       driver-test-vm
  Driver:   sample_kmdf_driver v14.8.40.302

[1/4] Starting log capture... done
[2/4] Installing driver package... done (oem3.inf)
[3/4] Verifying driver on VM... done (TODO-Set-Provider v14.8.40.302)
[4/4] Collecting captured logs... done (16 KB ETL, 5 events)

PASS  sample_kmdf_driver verified on driver-test-vm (47s)
  Trace output: C:\Users\you\AppData\Local\Temp\driver-test-logs\
```

### JSON (`--json`)

```json
{
  "success": true,
  "vm_name": "driver-test-vm",
  "driver": {
    "inf": "sample_kmdf_driver.inf",
    "published_name": "oem3.inf",
    "version": "14.8.40.302",
    "provider": "TODO-Set-Provider",
    "hardware_id": "root\\SAMPLE_KMDF_HW_ID",
    "instance_id": "ROOT\\DEVGEN\\{DEB8CFEF-5F79-FA44-A184-F8C9D051C71D}"
  },
  "steps": {
    "capture_start": { "status": "pass" },
    "install": { "status": "pass", "exit_code": 0 },
    "verify": { "status": "pass", "version_match": true, "provider_match": true },
    "capture_collect": { "status": "pass", "etl_size_kb": 16, "event_count": 5, "output_dir": "..." }
  },
  "duration_secs": 47
}
```

---

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `DEVGEN_PATH` | Custom path to `devgen.exe`. Overrides automatic WDK registry discovery. |
| `DBGVIEW_PATH` | Custom path to `Dbgview.exe`. Skips automatic download from the internet. |

> **Air-gapped / restricted environments:** If the test host cannot reach the internet,
> set `DBGVIEW_PATH` to a local copy of `Dbgview.exe`. Without this variable the capture
> step will attempt to download it and fail in offline environments.

---

## CI/CD Integration

### GitHub Actions

```yaml
jobs:
  driver-smoketest:
    runs-on: [self-hosted, windows, hyper-v]
    steps:
      - uses: actions/checkout@v4

      - name: Build driver
        run: cargo make --cwd examples/sample-kmdf-driver

      - name: Smoketest
        run: |
          driver-test-cli-v2 `
            --driver-package examples\sample-kmdf-driver\target\debug\sample_kmdf_driver_package `
            --vm-name ci-test-vm `
            --output-dir ${{ runner.temp }}\driver-logs `
            --json `
            --capture-duration 10

      - name: Upload traces
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: driver-traces
          path: ${{ runner.temp }}\driver-logs
```

### Azure DevOps

```yaml
steps:
  - script: |
      driver-test-cli-v2 ^
        --driver-package $(Build.ArtifactStagingDirectory)\driver_package ^
        --vm-name $(VM_NAME) ^
        --output-dir $(Build.ArtifactStagingDirectory)\driver-logs ^
        --json
    displayName: 'Smoketest driver'
```

### Key CI features

- `--json` for machine-parseable results
- Non-zero exit codes for automatic pipeline failure detection
- `--output-dir` for deterministic log output paths
- `--capture-duration 10` for shorter CI runs

---

## Troubleshooting

### Error: "Package validation failed: no .inf file found"

Your `--driver-package` directory doesn't contain a `.inf` file.

```powershell
# Check what's in the package:
Get-ChildItem .\target\debug\sample_kmdf_driver_package\

# Expected: *.inf, *.sys, and optionally *.cat, *.cer, *.pdb
```

**Fix:** Build the driver package first with `cargo make`.

### Error: "VM 'driver-test-vm' not found"

The specified VM doesn't exist in Hyper-V.

```powershell
# List available VMs:
Get-VM | Select-Object Name, State
```

**Fix:** Use `--vm-name` with a valid VM name, or create the VM first.

### Error: "Script timed out after 120s"

The driver installation is hanging, possibly waiting for user input inside the VM.

**Fix:**
1. Connect to the VM and check for interactive prompts
2. Ensure test signing is enabled: `bcdedit /set testsigning on`
3. Run `Install-DriverOnVM.ps1` manually to debug

### Error: "Driver verification failed"

The installed driver's version or provider doesn't match what's in the .inf file.

**Fix:**
1. Rebuild the driver package
2. Check that no other version of the driver is already installed
3. Use `--skip-verify` temporarily to investigate manually

### Verbose debugging

Use `-vv` or `-vvv` to see detailed logs of what each pipeline step is doing:

```powershell
driver-test-cli-v2 --driver-package .\package -vvv 2>&1 | Tee-Object debug.log
```
