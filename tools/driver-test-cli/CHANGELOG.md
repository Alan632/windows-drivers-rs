# Changelog

All notable changes to `driver-test-cli` will be documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]
### Added
- Dedicated guides: `docs/user-guide.md`, `docs/installation.md`, `docs/release.md`
- Repository-detection troubleshooting playbooks
- Phase 6 validation summary in `specs/001-driver-test-tools/tasks.md`

### Changed
- README status and doc links will be refreshed alongside the next release.

## [0.1.0] - 2025-11-14
### Added
- Hyper-V orchestration commands (`setup`, `snapshot`, `test`, `deploy`, `clean`)
- PowerShell Direct-based VM management
- Driver detection via Cargo metadata + INF heuristics (KMDF/UMDF/WDM)
- Companion application discovery and echo test validation
- Debug output capture + validation pipeline
- WMI enrichment for deployed drivers
- Cross-repository support for `windows-rust-driver-samples`
- Integration test suites covering pnputil parsing, deployment, VM ops, samples repository detection, and echo workflows

### Fixed
- INF discovery false-positives when running outside samples repository (limit parent traversal to samples layout)

### Security
- No security fixes; tool is intended for local, trusted environments.
