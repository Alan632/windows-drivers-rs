# Release Checklist

Use this checklist whenever publishing `driver-test-cli` to crates.io or tagging an internal milestone. The process assumes development happens on the `001-driver-test-tools` branch with Hyper-V access.

## 1. Freeze Changes
- Land all planned features and documentation for the release scope.
- Ensure `Cargo.toml` version is bumped according to semver (patch for bug fixes, minor for new features, etc.).
- Update `CHANGELOG.md` with a new section summarizing highlights.

## 2. Validate Code Quality
1. Format and lint:
   ```powershell
   cargo fmt
   cargo clippy --all-targets --all-features -- -D warnings
   ```
2. Run the full test matrix:
   ```powershell
   cargo test                          # unit + integration tests (Hyper-V tests ignored)
   cargo test --doc                    # doctests only
   cargo test --test hyperv_integration -- --ignored   # run when VM available
   ```
3. Verify there are no compiler warnings. If certain fields are intentionally unused, annotate with `#[allow(dead_code)]` + rationale.

## 3. Validate Packaging & Docs
- Regenerate README badges/sections if needed.
- `cargo package --allow-dirty` (local smoke check) or `cargo publish --dry-run` to verify crate completeness.
- Confirm the following docs are up to date:
  - `README.md`
  - `docs/user-guide.md`
  - `docs/installation.md`
  - `docs/troubleshooting.md`
  - `docs/repository-detection.md`
  - `docs/release.md` (this file)
  - `CHANGELOG.md`

## 4. Publish
1. Tag the commit:
   ```powershell
   git tag -a vX.Y.Z -m "driver-test-cli vX.Y.Z"
   git push origin vX.Y.Z
   ```
2. Publish to crates.io (requires API token in `~/.cargo/credentials`):
   ```powershell
   cargo publish
   ```
3. Update GitHub Releases (if applicable) with highlights, installation steps, and link to changelog section.

## 5. Post-Release
- Reset development version in `Cargo.toml` (e.g., `0.2.0-alpha.0`).
- File follow-up issues for deferred items (e.g., additional samples, telemetry, etc.).
- Archive Hyper-V VM snapshots used for validation if they need to be shared with other developers.
