# Architecture: `driver-test-cli-v2`

> A newcomer's guide to the Windows driver smoketest CLI.

---

## What This Tool Does

`driver-test-cli-v2` is a **single-command Rust CLI** that automates the process of
smoketesting a pre-built Windows driver on a local Hyper-V virtual machine. Instead
of manually running PowerShell scripts one at a time, the tool orchestrates them in
a fixed pipeline:

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  1. Start    │────▶│  2. Install  │────▶│  3. Verify   │────▶│  4. Collect  │
│  Log Capture │     │    Driver    │     │    Driver    │     │ Captured Logs│
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
  capture.rs           Install-Driver       Verify-Driver       capture.rs
  (DebugView + ETW)    OnVM.ps1             OnVM.ps1            (stop + extract)
```

**Inputs:** A directory containing compiled driver files (`.inf`, `.sys`, and
optionally `.cat`, `.cer`, `.pdb`) and a Hyper-V VM name.

**Outputs:** A PASS/FAIL verdict (human-readable or JSON), plus ETW trace files
copied to the host for post-mortem analysis.

---

## High-Level Architecture

```
                           ┌─────────────────────────────────────┐
                           │           main.rs                   │
                           │  CLI argument parsing (clap)        │
                           │  Tracing/logging init               │
                           │  Hardcoded credential setup         │
                           └───────────────┬─────────────────────┘
                                           │ PipelineConfig
                                           ▼
                           ┌─────────────────────────────────────┐
                           │         pipeline.rs                 │
                           │  Orchestrates the 4-step pipeline   │
                           │  Validates driver package layout    │
                           │  Collects StepResults into Report   │
                           └───┬──────────┬──────────────────────┘
                               │          │
                    runs       │          │  builds
                   scripts     │          │  report
                               ▼          ▼
              ┌────────────────────┐  ┌──────────────────────┐
              │    ps_runner.rs    │  │     report.rs         │
              │  Spawns PowerShell │  │  SmoketestReport      │
              │  Manages timeouts  │  │  JSON & interactive   │
              │  Handles retries   │  │  output formatting    │
              │  write_script()    │  └──────────────────────┘
              └────────┬───────────┘
                       │ std::process::Command
                       ▼
              ┌────────────────────┐
              │  PowerShell.exe    │
              │  scripts/*.ps1     │
              └────────────────────┘

              ┌────────────────────┐
              │    capture.rs      │
              │  DebugView + ETW   │  (writes ad-hoc .ps1 scripts
              │  capture lifecycle │   via PsRunner::write_script)
              └────────────────────┘

              ┌────────────────────┐
              │     error.rs       │
              │  SmoketestError    │   (used throughout)
              │  Exit codes        │
              │  ACTION: guidance  │
              └────────────────────┘
```

---

## Source File Map

```
tools/driver-test-cli-v2/
├── Cargo.toml              # Crate metadata and dependencies
├── src/
│   ├── main.rs             # CLI entry point — argument parsing, tracing init
│   ├── lib.rs              # Public module declarations
│   ├── capture.rs          # DebugView + ETW capture lifecycle (start/wait/stop)
│   ├── pipeline.rs         # Pipeline orchestrator — the core state machine
│   ├── ps_runner.rs        # PowerShell process spawner with timeout & retry
│   ├── error.rs            # Structured error types with exit codes + actions
│   └── report.rs           # Result aggregation, JSON/text output formatting
├── scripts/                # PowerShell scripts (loaded at runtime)
│   ├── Install-DriverOnVM.ps1
│   ├── Verify-DriverOnVM.ps1
│   └── Capture-DriverLogs.ps1
└── tests/
    └── unit_tests.rs       # Integration-style tests exercising public APIs
```

---

## Module Deep Dive

### `main.rs` — CLI Entry Point

**Purpose:** Parse command-line arguments, initialize logging, and wire
everything together.

**Key types:**
- `Cli` — A `clap::Parser` struct defining all CLI flags

**Flow:**
1. Parse CLI args via `Cli::parse()`
2. Initialize `tracing-subscriber` with verbosity level (0=warn → 3=trace).
   Respects the `RUST_LOG` environment variable if set.
3. Set hardcoded VM credentials (`DriverTestAdmin` / `1234`).
4. Build a `PipelineConfig` from the parsed args (including `--output-dir`,
   defaulting to `%TEMP%\driver-test-logs`)
5. Construct a `Pipeline` (validates the driver package on construction)
6. Call `pipeline.run()` to execute all steps
7. Print the report (JSON or interactive) and exit with the appropriate code.
   Exit code 2 is returned when `report.has_infra_failure()` is true.

**Exit codes:** `0` = pass, `1` = test failure, `2` = infrastructure error
(returned both at construction-time *and* at runtime via `has_infra_failure()`).

---

### `pipeline.rs` — The Orchestrator

**Purpose:** Drive the 4-step pipeline in order, collect results, and produce
a `SmoketestReport`.

**Key types:**
- `PipelineConfig` — All configuration needed for a run (driver path, VM name,
  skip flags, credential, etc.)
- `Pipeline` — The orchestrator. Created via `Pipeline::new(config)` which
  validates the driver package and initializes the `PsRunner`.

**Construction validates:**
- Driver package directory exists
- At least one `.inf` file is present
- At least one `.sys` file is present
- Extracts the driver name from the first `.inf` filename stem

**`Pipeline::run()` executes these steps in order:**

| Step | Method | Mechanism | Skippable |
|------|--------|-----------|-----------|
| 1 | `step_start_capture()` | `capture::start_capture()` → `_start_capture.ps1` | `--skip-capture` |
| 2 | `step_install()` | `Install-DriverOnVM.ps1` | No (always runs) |
| 3 | `step_verify()` | `Verify-DriverOnVM.ps1` | `--skip-verify` |
| 4 | `step_stop_capture()` | `capture::wait_capture()` + `capture::stop_and_extract()` → `_stop_capture.ps1`, `_extract_capture.ps1` | `--skip-capture` |

**Short-circuit on failure:** The pipeline stops at the first step that fails.
Any remaining steps are marked `Skipped` with a `reason` detail noting the
prior failure. This avoids wasting time on steps that cannot succeed (e.g.,
don't try to verify a driver that failed to install).

**Capture is started before install** so that DebugView and ETW are already
recording when the driver loads. After verification completes, the pipeline
waits for the configured capture duration (with periodic progress logging),
then stops capture, decodes the ETL file, and copies artefacts to the host.

The pipeline stores a `CaptureSession` handle (returned by `step_start_capture`)
and consumes it in `step_stop_capture`.

Each step method:
1. Constructs the argument list for the PowerShell script
2. Calls `PsRunner::run_script()` with a per-step timeout
3. Interprets the `ScriptResult` (exit code, stdout, stderr)
4. Returns a `StepResult` with `Pass`, `Fail(reason)`, or `Skipped` status

**`DriverInfo` population:** After all steps complete, the pipeline scans step
`details` maps to populate `DriverInfo` fields — `published_name` is read
directly from the details, while `Version`, `Provider`, `Hardware ID`, and
`Instance ID` are extracted from captured stdout using `extract_detail()`.

**Helper:** `extract_detail(stdout, key)` parses `"Key: Value"` or `"Key = Value"`
lines from script stdout to extract structured data like published driver names,
device status, or log paths.

---

### `capture.rs` — DebugView + ETW Capture

**Purpose:** Manage the lifecycle of DebugView and `logman` ETW trace capture
on the Hyper-V VM guest. Writes temporary `.ps1` scripts via
`PsRunner::write_script()` and executes them through the standard runner.

**Key types:**
- `CaptureConfig` — VM name, capture duration, host output directory.
- `CaptureSession` — Handle for a running capture (ETW session name, guest
  paths for log directory, DebugView log, and ETL file, plus host output dir).
- `CaptureResult` — Host paths to copied artefacts (DebugView log, ETL, XML,
  summary), each `Option<PathBuf>`.

**Public API (three functions mirroring the capture lifecycle):**
1. `start_capture(runner, config)` → writes and runs `_start_capture.ps1`:
   configures DbgPrint filter, provisions DebugView (downloads from
   Sysinternals if not on disk), accepts EULA, starts DebugView in
   log-to-file mode, starts a `logman` ETW session. Returns `CaptureSession`.
2. `wait_capture(duration_secs)` → sleeps for the configured duration with
   progress logging every 5 seconds.
3. `stop_and_extract(runner, session)` → writes and runs `_stop_capture.ps1`
   (stops DebugView + logman, decodes ETL → XML via `tracerpt`) then
   `_extract_capture.ps1` (copies artefacts from guest to host output dir).
   Returns `CaptureResult`.

**ETW provider:** Always captures KMDF v1 traces
(`{544D4C9D-942C-46D5-BF50-DF5CD9524A50}`).

---

### `ps_runner.rs` — PowerShell Process Spawner

**Purpose:** Execute PowerShell scripts as child processes with timeout
watchdogs, retry logic, and optional credential injection.

**Key types:**
- `PsRunner` — Main runner. On construction, writes scripts to a temp
  directory so PowerShell can execute them by file path.
- `ScriptResult` — Captured output: exit code, stdout, stderr, duration.
- `VmCredential` — Username + password pair for PS Direct VM connections.
- `PsRunnerError` — Error enum: I/O, ScriptNotFound, Timeout, ScriptFailed,
  RetriesExhausted, EmptyScript.

**Script resolution strategy:**
1. Try to use embedded script content (compile-time `include_str!()`)
2. If empty (fallback mode), load from `scripts/` directory relative to the
   executable or current working directory
3. If the resolved content is still empty, fail hard with `EmptyScript` error
   instead of writing an empty placeholder file
4. Write the resolved content to the temp `scripts_dir`

**Ad-hoc script support:** `PsRunner::write_script(name, content)` writes an
arbitrary script to the scripts directory, used by the `capture` module to
generate `_start_capture.ps1`, `_stop_capture.ps1`, and `_extract_capture.ps1`
at runtime.

**Two invocation modes:**

| Mode | Method | When |
|------|--------|------|
| Direct | `spawn_direct()` | No credentials — uses `powershell.exe -File` |
| Credential | `spawn_with_credential()` | With credentials — uses `-EncodedCommand` to construct a `PSCredential` inline |

**`run_script` signature:** `run_script(name, args, timeout, credential, elevated)`.
The 5th `elevated: bool` parameter wraps the invocation in
`Start-Process -Verb RunAs` when not already running as admin (checked via
`is_running_as_admin()`).

**Credential argument quoting:** When building the argument string for
`-EncodedCommand`, parameter names (strings starting with `-`) are passed
unquoted, while values are single-quoted with embedded quotes escaped. This
prevents PowerShell from misinterpreting parameter names like `-VMName` as
quoted string literals.

**Timeout mechanism:** The child process is wrapped in `Arc<Mutex<Child>>`.
Stdout/stderr pipes are separated from the child and read in dedicated threads,
so they don't require the mutex. A watchdog thread polls a `cancelled` flag in
a 100 ms loop (`Duration::from_millis(100)`) until the deadline; this lets it
exit promptly when the script finishes before the timeout instead of sleeping
for the full duration. If the deadline passes, the watchdog calls
`Child::kill()` through the mutex (handle-based, not PID-based). The main
thread calls `child.wait()` through the same mutex, sets the `cancelled` flag,
and joins the watchdog for clean shutdown.

**Retry mechanism:** `run_with_retry()` implements exponential backoff
(1s, 2s, 4s, …) for transient failures. Only retries on non-zero exit codes
or timeouts.

**Cleanup:** `PsRunner` implements `Drop` to delete the temp scripts directory.
The `no_cleanup` flag is honoured via `ManuallyDrop<PsRunner>` in `Pipeline` —
when the flag is set, `Pipeline`'s `Drop` intentionally leaks the runner so its
cleanup never runs.

---

### `error.rs` — Error Types & Exit Codes

**Purpose:** Provide a comprehensive error taxonomy with human-readable
messages, exit code mapping, and remediation guidance.

**Key type:** `SmoketestError` — an enum with variants for every failure mode:

| Variant | Exit Code | Category |
|---------|-----------|----------|
| `PackageValidation` | 1 (test) | Missing .inf/.sys, bad layout |
| `DriverInstallFailed` | 1 (test) | pnputil failure with sub-exit-codes |
| `DriverVerificationFailed` | 1 (test) | Version/provider mismatch |
| `VmNotFound` | 2 (infra) | Hyper-V VM doesn't exist |
| `VmNotRunning` | 2 (infra) | VM exists but isn't running |
| `ScriptExecution` | 2 (infra) | Script returned non-zero |
| `ScriptTimeout` | 2 (infra) | Script exceeded timeout |
| `SnapshotFailed` | 2 (infra) | Snapshot create/revert failed |
| `CaptureFailed` | 2 (infra) | ETW trace capture failed |
| `Io` | 2 (infra) | Filesystem I/O error |

**Every variant provides:**
- `action()` → an `"ACTION: ..."` string with specific remediation steps
- `exit_code()` → `0`, `1`, or `2`
- `classification()` → `UPPER_SNAKE_CASE` label for structured reporting
- `Display` → human-readable error message (via `thiserror`)

---

### `report.rs` — Output Formatting

**Purpose:** Define the structured report produced at the end of a pipeline run,
with both machine-readable (JSON) and human-readable (interactive) output.

**Key types:**
- `SmoketestReport` — Top-level report: success flag, VM name, driver info,
  list of step results, total duration.
- `StepResult` — One pipeline step's outcome: name, status, exit code,
  duration, a details map for step-specific metadata, and an `is_infra_failure`
  flag that classifies the failure as environmental rather than test-related.
- `StepStatus` — `Pass`, `Fail(String)`, or `Skipped`.
- `DriverInfo` — Metadata about the driver under test (inf name, published
  name, version, provider, hardware/instance IDs).

**Key methods on `SmoketestReport`:**
- `is_success()` — true when every non-skipped step passed.
- `has_infra_failure()` — true when any failed step has `is_infra_failure` set.
  Used by `main.rs` to select exit code 2.
- `to_json()` — pretty-printed JSON with a graceful fallback (returns a JSON
  error object instead of panicking if serialization fails).
- `try_to_json()` — returns `Result<String, serde_json::Error>` for callers
  that want to handle errors explicitly.
- `print_interactive()` — human-readable terminal summary.

**Output modes:**

| Method | Format | Usage |
|--------|--------|-------|
| `to_json()` | Pretty-printed JSON | CI pipelines (`--json` flag) |
| `print_interactive()` | Human-readable summary | Terminal usage (default) |

**Interactive output example:**
```
driver-smoketest v0.1.0
  Package:  sample_kmdf_driver.inf
  VM:       driver-test-vm
  Driver:   oem3.inf v1.0.0

[1/4] Starting log capture... done
[2/4] Installing driver package... done (oem3.inf)
[3/4] Verifying driver on VM... done (OK)
[4/4] Collecting captured logs... done

PASS  oem3.inf verified on driver-test-vm (47s)
```

---

## Data Flow

```
User invokes:
  driver-smoketest --driver-package C:\drivers\my_driver --vm-name test-vm --output-dir C:\logs --json

                        ┌──────────────────────────────┐
                        │  main.rs parses CLI args      │
                        │  Builds PipelineConfig        │
                        └──────────────┬───────────────┘
                                       ▼
                        ┌──────────────────────────────┐
                        │  Pipeline::new(config)        │
                        │  Validates: .inf + .sys exist │
                        │  Creates PsRunner (temp dir)  │
                        └──────────────┬───────────────┘
                                       ▼
              ┌────────────────────────────────────────────────┐
              │  Pipeline::run()                                │
              │                                                │
              │  for each step:                                │
              │    1. Build args for the PowerShell script      │
              │    2. PsRunner::run_script(name, args, timeout)│
              │       └─▶ spawns powershell.exe                │
              │       └─▶ watchdog thread monitors timeout     │
              │       └─▶ captures stdout/stderr/exit_code     │
              │    3. Interpret ScriptResult → StepResult       │
              │    4. Append to steps vec                       │
              │    5. If step failed → skip remaining steps     │
              │  Populate DriverInfo from step details          │
              └────────────────────┬───────────────────────────┘
                                   ▼
              ┌────────────────────────────────────────────────┐
              │  SmoketestReport                                │
              │    .is_success()       → all steps Pass/Skipped│
              │    .has_infra_failure() → any infra failure     │
              │    .to_json()          → JSON string           │
              │    .print_interactive() → terminal output       │
              └────────────────────────────────────────────────┘
                                   ▼
              main.rs prints report and exits with code 0, 1, or 2
              (code 2 when has_infra_failure() is true)
```

---

## PowerShell Scripts

The PowerShell scripts are the workhorses. The Rust CLI orchestrates them in
sequence. Install and Verify use static `.ps1` files (embedded or loaded from
`scripts/`). Capture uses dynamically generated `.ps1` scripts written by
`capture.rs` via `PsRunner::write_script()`.

### Script Responsibilities

| Script | What It Does | Key Parameters |
|--------|-------------|----------------|
| `Install-DriverOnVM.ps1` | Copies driver files to a unique guest directory, imports `.cer` certificates, enables test signing, installs via `pnputil`. Uses PS Direct for file copy and remote execution. | `-VMName`, `-DriverPath`, `-Credential` |
| `Verify-DriverOnVM.ps1` | Parses the `.inf` to extract expected version/provider/hardware IDs. Queries the VM for matching devices. Creates a device node via `devgen` if needed. Can re-invoke install if the wrong driver is bound — credentials are forwarded via `-EncodedCommand` with `PSCredential` reconstruction, and the install runs in a subprocess to prevent `exit` from killing the verify session. Locates `devgen.exe` via `DEVGEN_PATH` env var, registry-based WDK discovery (`KitsRoot10`/`KitsRoot` under `Installed Roots`), or standard `Program Files` paths. | `-VMName`, `-DriverPath`, `-Credential` |
| `_start_capture.ps1` *(generated)* | Configures DbgPrint filter, provisions DebugView (downloads from Sysinternals if needed), accepts EULA, starts DebugView in log-to-file mode, starts `logman` ETW session. | `-VMName`, `-LogDir`, `-DbgViewLog`, `-EtlPath`, `-SessionName`, `-ProviderGuid` |
| `_stop_capture.ps1` *(generated)* | Stops DebugView process, stops `logman` ETW session, decodes ETL → XML via `tracerpt`. | `-VMName`, `-SessionName`, `-EtlPath` |
| `_extract_capture.ps1` *(generated)* | Copies DebugView log, ETL, XML, and summary files from guest to host output directory. | `-VMName`, `-DbgViewLog`, `-EtlPath`, `-XmlPath`, `-SummaryPath`, `-OutputDir` |

### Script Exit Codes (Install)

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | No `.inf` found |
| 2 | VM not found |
| 3 | File copy failure |
| 4 | Certificate import failure |
| 5 | `pnputil` install failure |
| 6 | Timeout |

---

## Driver Package Layout

The tool validates this structure before running any pipeline steps:

```
<driver-package>/
├── *.inf          ← REQUIRED: Driver installation manifest
├── *.sys          ← REQUIRED: Compiled driver binary
├── *.cat          ← Optional: Catalog file for signature verification
├── *.cer          ← Optional: Test-signing certificate
└── *.pdb          ← Optional: Debug symbols for trace decoding
```

Validation is performed in `Pipeline::new()`. The driver name is derived from
the first `.inf` file's stem (e.g., `sample_driver.inf` → `"sample_driver"`).

---

## Error Handling Philosophy

Every error includes actionable remediation guidance. The pattern is:

```
error: <what went wrong>
ACTION: <what the user should do>
```

Errors are categorized into two exit code buckets:
- **Exit 1 (Test Failure):** The driver itself has a problem — bad package,
  install failure, version mismatch.
- **Exit 2 (Infra Error):** The environment has a problem — VM missing, PS
  Direct unavailable, script timeout.

This distinction is important for CI: infra errors should trigger retries or
environment checks, not driver investigations.

---

## Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI argument parsing |
| `tracing` | Structured logging framework |
| `tracing-subscriber` | Log output formatting with env-filter |
| `serde` + `serde_json` | JSON serialization for reports |
| `thiserror` | Derive `Error` + `Display` for error types |

**Dev dependencies:** `assert_cmd`, `predicates`, `tempfile` (for CLI and
integration tests).

---

## Prerequisites

To **build** this tool:
- Rust toolchain (`cargo build`)

To **run** this tool:
- Windows host with **Hyper-V enabled**
- **Administrator privileges** (Hyper-V cmdlets require elevation)
- **WDK** installed on the host (for `devgen.exe`, used by the verify script).
  Discovered automatically via `DEVGEN_PATH` env var or registry (`KitsRoot10`/`KitsRoot`).
- A running Hyper-V VM with Windows, or a `.vhdx` + `--create-vm`
- A compiled driver package directory

---

## Spec vs. Implementation Status

The spec (`driver-smoketest-spec.md`) describes the complete vision. The current
implementation covers the core pipeline but has some gaps:

| Feature | Spec | Implemented |
|---------|------|-------------|
| 4-step pipeline (start capture → install → verify → collect logs) | ✅ | ✅ |
| Pipeline short-circuits on first failure | ✅ | ✅ |
| CLI argument parsing | ✅ | ✅ |
| Skip flags (`--skip-verify`, `--skip-capture`) | ✅ | ✅ |
| `--output-dir` for captured log files | ✅ | ✅ (defaults to `%TEMP%\driver-test-logs`) |
| JSON output (`--json`) | ✅ | ✅ |
| Verbosity levels (`-v`/`-vv`/`-vvv`) | ✅ | ✅ |
| Structured error types with `ACTION:` guidance | ✅ | ✅ |
| Exit code 0/1/2 distinction | ✅ | ✅ |
| `--no-cleanup` flag behavior | ✅ | ✅ (`ManuallyDrop<PsRunner>`) |
| `capture.rs` module for DebugView + ETW lifecycle | ✅ | ✅ |
| Script embedding via `include_str!()` | ✅ | Stub (empty constants, runtime fallback) |
| Snapshot management (`--skip-snapshot`, `--snapshot-name`) | — | ❌ Removed |
| VM credential options (`--vm-credential-user`, `--vm-credential-password`) | — | ❌ Removed (hardcoded) |
| VM auto-detection (first running VM) | ✅ | ❌ Not implemented |
| VM creation (`--create-vm`, `--vhdx`, etc.) | ✅ | ❌ Not implemented |
| `vm.rs` module for VM discovery/state management | ✅ | ❌ Not implemented |

---

## Quick Start for Contributors

### Build

```powershell
cargo build -p driver-test-cli-v2
```

### Run Tests

```powershell
cargo test -p driver-test-cli-v2
```

### Try It (requires Hyper-V + a driver package)

```powershell
cargo run -p driver-test-cli-v2 -- `
    --driver-package C:\path\to\driver\package `
    --vm-name my-test-vm `
    --json -vv
```

### Code Tour (recommended reading order)

1. **`error.rs`** — Start here. Small file, defines every failure mode and its
   exit code. Gives you the vocabulary of the system.
2. **`report.rs`** — Read next. Defines the output structure (`SmoketestReport`,
   `StepResult`, `StepStatus`). Now you know inputs and outputs.
3. **`ps_runner.rs`** — The execution engine. Understand how PowerShell scripts
   are resolved, spawned, timed out, and retried.
4. **`capture.rs`** — DebugView + ETW capture lifecycle. Uses `PsRunner::write_script()`
   to generate ad-hoc `.ps1` scripts at runtime.
5. **`pipeline.rs`** — The orchestrator. Ties everything together: start capture,
   install, verify, then stop capture and extract logs.
6. **`main.rs`** — The thin CLI shell. Just argument parsing and wiring.
7. **`tests/unit_tests.rs`** — Comprehensive test suite organized by module.
