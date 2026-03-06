<!--
Sync Impact Report:
- Version: NONE → 1.0.0
- Modified principles: Initial creation
- Added sections: Core Principles (4 principles), Performance Standards, Quality Gates, Governance
- Templates requiring updates:
  ✅ plan-template.md (reviewed - compatible with constitution check section)
  ✅ spec-template.md (reviewed - compatible with user story and requirements approach)
  ✅ tasks-template.md (reviewed - compatible with phased testing approach)
- Follow-up TODOs: None
-->

# Driver Deploy Test Tool Constitution

## Core Principles

### I. Rust Idiomatic Code Quality (NON-NEGOTIABLE)

All code MUST adhere to Rust best practices and idiomatic patterns:
- Clippy lints MUST pass with zero warnings (use `#[expect]` with documented rationale for overrides)
- Code MUST be formatted with `rustfmt` before commits
- Error handling MUST use `Result<T, E>` with meaningful error types (never `unwrap()` in library code)
- Public APIs MUST follow Rust API Guidelines (RFC 430 naming conventions, conversion traits)
- Dependencies MUST be minimal, well-maintained, and security-audited
- All public types MUST implement `Debug`; user-facing types MUST implement `Display`

**Rationale**: Idiomatic Rust ensures maintainability, prevents common bugs through type safety,
and provides consistent experience for contributors familiar with Rust ecosystem standards.

### II. Test-First Development (NON-NEGOTIABLE)

Test-driven development is mandatory for all features:
- Unit tests MUST be written BEFORE implementation (Red-Green-Refactor cycle)
- Tests MUST fail initially, proving they exercise the new code
- Integration tests REQUIRED for CLI commands and driver deployment workflows
- Contract tests REQUIRED for any external interfaces (driver APIs, system calls)
- Code coverage targets: minimum 80% for critical paths, 60% overall
- Tests MUST be self-contained, deterministic, and run in parallel safely

**Rationale**: TDD ensures requirements are testable, prevents regressions, and serves as
living documentation. Critical for a deployment tool where failures impact production systems.

### III. User Experience Consistency

CLI interface MUST provide consistent, predictable user experience:
- Error messages MUST be actionable with clear next steps
- All commands MUST support `--help` with comprehensive usage examples
- Output formats: human-readable (default) and `--json` for automation
- Exit codes MUST follow conventions (0=success, 1=user error, 2=system error)
- Progress indicators REQUIRED for operations >2 seconds
- Verbose mode (`-v`, `-vv`, `-vvv`) for debugging without code changes
- Confirmation prompts for destructive operations (override with `--yes`)

**Rationale**: Deployment tools are often used in stressful scenarios (production issues).
Clear, consistent UX reduces cognitive load and prevents operator errors.

### IV. Performance & Reliability Standards

Tool MUST operate reliably under typical deployment conditions:
- Startup time: <200ms for basic commands (cached dependencies)
- Memory footprint: <50MB for standard operations
- Driver deployment operations MUST be idempotent and safely retryable
- Network timeouts MUST be configurable with sensible defaults (30s connect, 5m total)
- File I/O MUST handle partial writes and corrupted data gracefully
- All operations MUST log structured diagnostic information for troubleshooting

**Rationale**: Deployment automation requires predictable performance and resilience.
Slow or unreliable tools create bottlenecks in CI/CD pipelines and frustrate users.

## Performance Standards

**Build Performance**:
- Debug build time: <10 seconds incremental, <2 minutes clean
- Release build time: <5 minutes clean with LTO enabled
- Binary size: <10MB stripped release binary

**Runtime Performance**:
- Command parsing and validation: <50ms
- File system operations: async I/O for >1MB files
- Concurrent driver operations: support 4+ parallel deployments
- Resource cleanup: proper Drop implementations, no leaked handles

**Benchmarking Requirements**:
- Performance-critical paths MUST have criterion benchmarks
- Regression tests in CI for operations >100ms baseline
- Profile before optimizing; document tradeoffs in code comments

## Quality Gates

**Pre-Commit Requirements**:
- `cargo clippy --all-targets --all-features` passes with zero warnings
- `cargo fmt --check` passes
- `cargo test` passes all tests
- `cargo doc --no-deps` generates documentation without warnings

**Pre-Merge Requirements** (CI enforced):
- All quality gates above PLUS:
- `cargo audit` passes (no known vulnerabilities)
- Integration tests pass on Windows (primary target platform)
- Code review approval from maintainer
- No `TODO` or `FIXME` comments without tracking issue references

**Release Requirements**:
- All dependencies up-to-date (or exceptions documented)
- Changelog updated following Keep a Changelog format
- Semver compliance verified (cargo-semver-checks)
- Manual smoke test on clean Windows environment

## Governance

This constitution supersedes all other development practices and conventions. All code reviews,
feature specifications, and implementation plans MUST verify compliance with these principles.

**Amendment Process**:
- Proposed changes MUST be documented in a specification under `.specify/memory/`
- Amendments require rationale explaining why current principles are insufficient
- Version MUST increment: MAJOR (removing/weakening principle), MINOR (adding principle),
  PATCH (clarification only)
- Migration plan REQUIRED for changes affecting existing code

**Compliance Verification**:
- Plan template includes mandatory "Constitution Check" section
- Tasks failing compliance MUST justify in "Complexity Tracking" table
- Quarterly reviews to identify systematic violations and address root causes

**Version**: 1.0.0 | **Ratified**: 2025-11-10 | **Last Amended**: 2025-11-10
