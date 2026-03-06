# Repository Detection Heuristics

The driver test CLI now recognizes two repository families automatically:

| Repository                    | Primary Signals                                                                                                                    | Build Output Root                                                                           |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `windows-drivers-rs`          | Default fallback when no samples markers are present                                                                               | `target/release`                                                                            |
| `windows-rust-driver-samples` | Folder name `windows-rust-driver-samples`, `.samples-root`, `samples.json`, `Samples.props`, or `sample-list.json` in any ancestor | `target/wdk/<arch>/Release` (prefers existing directories such as `target/wdk/x64/Release`) |

## Detection Flow

1. **Start from working directory** and walk ancestor folders.
2. **Check for explicit repo name** (`windows-rust-driver-samples`).
3. **Look for marker files** used by the samples repo metadata (`samples.json`, `Samples.props`, `sample-list.json`, `.samples-root`, `samples.yaml`).
4. **Classify** as `WindowsRustDriverSamples` when any signal matches; otherwise default to `WindowsDriversRs`.

The helper `detect_samples_repository(path)` exposes the boolean check for tests and diagnostics.

## INF Discovery Adjustments

Windows-Rust-driver-samples stores INF files in sibling folders (e.g., `general/echo/kmdf/inf/echo.inf`). The detector now:

- Searches multiple candidate roots (`driver`, `inf`, `deployment/package`, `pkg`, plus the parent directory).
- Increases walk depth to 8 when a samples repo is detected.
- Infers driver type from `KmdfLibraryVersion` / `UmdfLibraryVersion` directives even when `[KMDF]` or `[UMDF]` sections are omitted.

## Companion Application Search

Companion executables are resolved from repository-aware locations:

- For windows-drivers-rs we continue to probe `target/release`, `bin/`, `apps/`, etc.
- For samples repositories we additionally scan `driver/bin`, `driver/apps`, `apps/win32`, `host/`, and the Cargo WDK build directory (`target/wdk/<arch>/Release`).

## Build Output Helpers

`RepositoryType::build_output_dir(root)` centralizes target selection:

- Samples repos prefer Cargo WDK output (`target/wdk/x64/Release` → `target/wdk/amd64/Release` → `target/release`).
- The helper returns the first existing directory, falling back to the canonical path even if it hasn't been created yet so the caller can still append filenames.

These heuristics let the CLI run from either repository without manual flags or configuration edits.
