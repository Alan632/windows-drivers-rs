// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Error types and exit codes for driver smoketest orchestration.

/// Process exit code: all tests passed.
pub const EXIT_SUCCESS: i32 = 0;
/// Process exit code: a test-level failure (driver install, verification, etc.).
pub const EXIT_TEST_FAILURE: i32 = 1;
/// Process exit code: an infrastructure / environmental error.
pub const EXIT_INFRA_ERROR: i32 = 2;

/// Errors that can occur during a driver smoketest run.
#[derive(Debug, thiserror::Error)]
pub enum SmoketestError {
    /// Missing .inf, .sys, or invalid driver package layout.
    #[error("Package validation failed: {reason}")]
    PackageValidation { reason: String },

    /// The requested Hyper-V VM does not exist.
    #[error("VM '{name}' not found")]
    VmNotFound { name: String },

    /// The VM exists but is not in the Running state.
    #[error("VM '{name}' is not running (state: {state})")]
    VmNotRunning { name: String, state: String },

    /// A PowerShell script returned a non-zero exit code.
    #[error("Script '{script}' failed (exit code {exit_code}): {stderr}")]
    ScriptExecution {
        script: String,
        exit_code: i32,
        stderr: String,
    },

    /// A PowerShell script exceeded its timeout.
    #[error("Script '{script}' timed out after {timeout_secs}s")]
    ScriptTimeout { script: String, timeout_secs: u64 },

    /// `pnputil` / `Install-DriverOnVM.ps1` reported a failure.
    ///
    /// Exit codes from the install script:
    /// 1 = no .inf found, 2 = VM not found, 3 = copy failure,
    /// 4 = certificate failure, 5 = install failure, 6 = timeout.
    #[error("Driver installation failed (install exit code {install_exit_code}): {detail}")]
    DriverInstallFailed {
        install_exit_code: i32,
        detail: String,
    },

    /// Post-install verification detected a version or provider mismatch.
    #[error("Driver verification failed: {reason}")]
    DriverVerificationFailed { reason: String },

    /// Hyper-V snapshot create or revert failed.
    #[error("Snapshot operation failed: {reason}")]
    SnapshotFailed { reason: String },

    /// ETW / trace capture failed.
    #[error("Trace capture failed: {reason}")]
    CaptureFailed { reason: String },

    /// An I/O error from the standard library.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl SmoketestError {
    /// Returns a human-readable remediation hint for this error.
    pub fn action(&self) -> &str {
        match self {
            Self::PackageValidation { .. } => {
                "ACTION: Verify your build output contains a valid .inf and .sys in the expected \
                 package layout."
            }
            Self::VmNotFound { .. } => {
                "ACTION: Ensure the VM name matches an existing Hyper-V VM (Get-VM)."
            }
            Self::VmNotRunning { .. } => {
                "ACTION: Start the VM before running the smoketest (Start-VM)."
            }
            Self::ScriptExecution { .. } => {
                "ACTION: Check stderr output above. Ensure PowerShell scripts are accessible and \
                 the execution policy allows running them."
            }
            Self::ScriptTimeout { .. } => {
                "ACTION: Increase the timeout or investigate why the script is hanging."
            }
            Self::DriverInstallFailed {
                install_exit_code, ..
            } => match install_exit_code {
                1 => "ACTION: No .inf file found in the driver package.",
                2 => "ACTION: VM not found by the install script; verify the VM name.",
                3 => "ACTION: Failed to copy driver files to the VM; check VM connectivity.",
                4 => "ACTION: Certificate installation failed; ensure the test-signing cert is available.",
                5 => "ACTION: pnputil install failed; check driver signing and INF syntax.",
                6 => "ACTION: Install timed out; the VM may be unresponsive.",
                _ => "ACTION: Unexpected install exit code; check Install-DriverOnVM.ps1 output.",
            },
            Self::DriverVerificationFailed { .. } => {
                "ACTION: The installed driver version or provider does not match the expected \
                 values. Rebuild or re-sign the driver."
            }
            Self::SnapshotFailed { .. } => {
                "ACTION: Verify Hyper-V snapshot permissions and available disk space."
            }
            Self::CaptureFailed { .. } => {
                "ACTION: Ensure ETW tracing permissions are available and the trace session is not \
                 already in use."
            }
            Self::Io(_) => {
                "ACTION: An I/O error occurred; check file paths and permissions."
            }
        }
    }

    /// Maps this error to a process exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            // Test-level failures
            Self::PackageValidation { .. }
            | Self::DriverInstallFailed { .. }
            | Self::DriverVerificationFailed { .. } => EXIT_TEST_FAILURE,

            // Infrastructure / environmental errors
            Self::VmNotFound { .. }
            | Self::VmNotRunning { .. }
            | Self::ScriptExecution { .. }
            | Self::ScriptTimeout { .. }
            | Self::SnapshotFailed { .. }
            | Self::CaptureFailed { .. }
            | Self::Io(_) => EXIT_INFRA_ERROR,
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, SmoketestError>;

impl SmoketestError {
    /// Returns a short classification label for structured reporting.
    pub fn classification(&self) -> &str {
        match self {
            Self::PackageValidation { .. } => "PACKAGE_VALIDATION",
            Self::VmNotFound { .. } => "VM_NOT_FOUND",
            Self::VmNotRunning { .. } => "VM_NOT_RUNNING",
            Self::ScriptExecution { .. } => "SCRIPT_EXECUTION",
            Self::ScriptTimeout { .. } => "SCRIPT_TIMEOUT",
            Self::DriverInstallFailed { .. } => "DRIVER_INSTALL_FAILED",
            Self::DriverVerificationFailed { .. } => "DRIVER_VERIFICATION_FAILED",
            Self::SnapshotFailed { .. } => "SNAPSHOT_FAILED",
            Self::CaptureFailed { .. } => "CAPTURE_FAILED",
            Self::Io(_) => "IO_ERROR",
        }
    }
}
