// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! PowerShell script runner for driver test orchestration.
//!
//! Embeds PowerShell scripts at compile time and provides facilities to
//! execute them with optional VM credentials, timeouts, and retries.
//!
//! **Fallback mode**: When the repository-root `.ps1` files are unavailable at
//! compile time, the embedded constants are empty and scripts are loaded at
//! runtime from a `scripts/` directory relative to the executable.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Admin detection
// ---------------------------------------------------------------------------

/// Check if the current process is running with administrator privileges.
fn is_running_as_admin() -> bool {
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Embedded scripts
// ---------------------------------------------------------------------------
// Build requirement: the repository root must contain the PowerShell scripts
// (Install-DriverOnVM.ps1, Verify-DriverOnVM.ps1,
// Capture-DriverLogs.ps1). When the files are not accessible at compile time
// (e.g. content-exclusion policy), these constants are empty and the runner
// falls back to loading from a `scripts/` directory at runtime.

pub const INSTALL_SCRIPT: &str = "";
pub const VERIFY_SCRIPT: &str = "";
pub const CAPTURE_SCRIPT: &str = "";

/// Known script names mapped to their (possibly empty) embedded content.
fn embedded_scripts() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("Install-DriverOnVM.ps1", INSTALL_SCRIPT),
        ("Verify-DriverOnVM.ps1", VERIFY_SCRIPT),
        ("Capture-DriverLogs.ps1", CAPTURE_SCRIPT),
    ])
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PsRunnerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("script not found: {0}")]
    ScriptNotFound(String),

    #[error("script timed out after {0:?}")]
    Timeout(Duration),

    #[error("script failed with exit code {exit_code}: {stderr}")]
    ScriptFailed { exit_code: i32, stderr: String },

    #[error("all {0} retries exhausted")]
    RetriesExhausted(u32),

    #[error("script '{0}' has no content — ensure scripts are properly bundled or available in the scripts/ directory")]
    EmptyScript(String),
}

pub type Result<T> = std::result::Result<T, PsRunnerError>;

// ---------------------------------------------------------------------------
// VmCredential
// ---------------------------------------------------------------------------

/// Optional username + password for PS Direct VM connections.
#[derive(Clone)]
pub struct VmCredential {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for VmCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmCredential")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ScriptResult
// ---------------------------------------------------------------------------

/// Captured output from a PowerShell script run.
#[derive(Debug, Clone)]
pub struct ScriptResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

impl ScriptResult {
    /// Returns `true` when the script exited with code 0.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

// ---------------------------------------------------------------------------
// Runtime script loading (fallback)
// ---------------------------------------------------------------------------

/// Load a script from the `scripts/` directory. Searches multiple locations:
/// 1. Next to the executable (`<exe_dir>/scripts/`)
/// 2. Current working directory (`./scripts/`)
/// 3. Alongside the crate source (for `cargo run` from workspace root)
pub fn load_script_from_disk(name: &str) -> Result<String> {
    let exe_dir = std::env::current_exe()
        .map_err(PsRunnerError::Io)?
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let mut candidates = vec![
        exe_dir.join("scripts").join(name),
        PathBuf::from("scripts").join(name),
    ];

    // When run via `cargo run` from workspace root, the exe is in
    // target/debug/ but scripts live in tools/driver-test-cli-v2/scripts/
    candidates.push(PathBuf::from("tools/driver-test-cli-v2/scripts").join(name));

    // Also check CARGO_MANIFEST_DIR if set (available during `cargo run`)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(&manifest_dir).join("scripts").join(name));
    }

    for path in &candidates {
        if path.exists() {
            return fs::read_to_string(path).map_err(PsRunnerError::Io);
        }
    }

    Err(PsRunnerError::ScriptNotFound(name.to_string()))
}

// ---------------------------------------------------------------------------
// PsRunner
// ---------------------------------------------------------------------------

/// Main PowerShell script runner.
///
/// On construction the runner writes each known script to `scripts_dir` so
/// that `powershell.exe` can be pointed at a concrete file path.
pub struct PsRunner {
    scripts_dir: PathBuf,
}

impl PsRunner {
    /// Create a new runner, writing embedded scripts to `scripts_dir`.
    ///
    /// When an embedded script is empty (fallback mode), the runner attempts to
    /// load it from disk via [`load_script_from_disk`].
    pub fn new(scripts_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&scripts_dir)?;

        for (name, content) in embedded_scripts() {
            let dest = scripts_dir.join(name);
            let script_content = if content.is_empty() {
                match load_script_from_disk(name) {
                    Ok(loaded) => {
                        info!(script = name, "loaded script from disk (fallback)");
                        loaded
                    }
                    Err(_e) => {
                        return Err(PsRunnerError::EmptyScript(name.to_string()));
                    }
                }
            } else {
                content.to_string()
            };

            if script_content.is_empty() {
                return Err(PsRunnerError::EmptyScript(name.to_string()));
            }

            let mut file = fs::File::create(&dest)?;
            file.write_all(script_content.as_bytes())?;
            debug!(script = name, path = %dest.display(), "wrote script to temp dir");
        }

        Ok(Self { scripts_dir })
    }

    /// Write an ad-hoc script to the scripts directory so it can be run via
    /// [`run_script`].
    pub fn write_script(&self, name: &str, content: &str) -> Result<()> {
        let dest = self.scripts_dir.join(name);
        let mut file = fs::File::create(&dest)?;
        file.write_all(content.as_bytes())?;
        debug!(script = name, path = %dest.display(), "wrote ad-hoc script");
        Ok(())
    }

    /// Run a PowerShell script by name.
    ///
    /// # Arguments
    /// * `name`       – Script filename (e.g. `"Install-DriverOnVM.ps1"`)
    /// * `args`       – Arguments forwarded to the script
    /// * `timeout`    – Maximum wall-clock time before the process is killed
    /// * `credential` – Optional [`VmCredential`] for PS Direct sessions
    /// * `elevated`   – If `true`, run the script in an elevated (admin) PowerShell session
    pub fn run_script(
        &self,
        name: &str,
        args: &[String],
        timeout: Duration,
        credential: Option<&VmCredential>,
        elevated: bool,
    ) -> Result<ScriptResult> {
        let script_path = self.scripts_dir.join(name);
        if !script_path.exists() {
            return Err(PsRunnerError::ScriptNotFound(name.to_string()));
        }

        info!(
            script = name,
            ?timeout,
            has_credential = credential.is_some(),
            elevated,
            "running script"
        );

        let start = Instant::now();

        // Determine if we actually need elevation.
        // If already running as admin, skip the Start-Process -Verb RunAs wrapper.
        let needs_elevation = elevated && !is_running_as_admin();

        // Credentials are hardcoded in the scripts, so always use spawn_direct.
        let mut child = if needs_elevation {
            self.spawn_elevated(&script_path, args, credential)?
        } else {
            self.spawn_direct(&script_path, args)?
        };

        // Separate pipes before sharing the child so readers don't need
        // the mutex and the watchdog retains kill access at all times.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        // ── Timeout watchdog ──────────────────────────────────────────
        let child_handle = Arc::new(Mutex::new(child));
        let child_for_watchdog = child_handle.clone();
        let timed_out = Arc::new(AtomicBool::new(false));
        let flag = timed_out.clone();

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = cancelled.clone();

        // Poll cancelled flag in small intervals so the watchdog exits
        // promptly when the script finishes before the timeout.
        let watchdog = std::thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if cancelled_clone.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if !cancelled_clone.load(Ordering::SeqCst) {
                flag.store(true, Ordering::SeqCst);
                if let Ok(mut guard) = child_for_watchdog.lock() {
                    let _ = guard.kill();
                }
            }
        });

        // Read stdout/stderr in dedicated threads (pipes close when
        // the child exits or is killed, unblocking the reads).
        let stdout_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });
        let stderr_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        // Wait for exit through the mutex — child stays killable.
        let exit_status = {
            let mut guard = child_handle.lock().unwrap();
            guard.wait()?
        };

        // Signal and join watchdog (returns quickly due to polling).
        cancelled.store(true, Ordering::SeqCst);
        let _ = watchdog.join();

        let duration = start.elapsed();

        let stdout_bytes = stdout_thread.join().unwrap_or_default();
        let stderr_bytes = stderr_thread.join().unwrap_or_default();

        if timed_out.load(Ordering::SeqCst) {
            return Err(PsRunnerError::Timeout(timeout));
        }

        let result = ScriptResult {
            exit_code: exit_status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
            duration,
        };

        debug!(
            script = name,
            exit_code = result.exit_code,
            duration_ms = result.duration.as_millis() as u64,
            "script completed"
        );

        Ok(result)
    }

    /// Run a script with automatic retries and exponential backoff (1 s, 2 s,
    /// 4 s, …).
    pub fn run_with_retry(
        &self,
        name: &str,
        args: &[String],
        timeout: Duration,
        credential: Option<&VmCredential>,
        max_retries: u32,
    ) -> Result<ScriptResult> {
        let mut last_result: Option<ScriptResult> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let backoff = Duration::from_secs((1u64 << (attempt - 1)).min(30));
                warn!(
                    script = name,
                    attempt,
                    max_retries,
                    backoff_secs = backoff.as_secs(),
                    "retrying after backoff"
                );
                std::thread::sleep(backoff);
            }

            match self.run_script(name, args, timeout, credential, false) {
                Ok(result) if result.success() => return Ok(result),
                Ok(result) => {
                    warn!(
                        script = name,
                        attempt,
                        exit_code = result.exit_code,
                        "script returned non-zero exit code"
                    );
                    last_result = Some(result);
                }
                Err(PsRunnerError::Timeout(_)) if attempt < max_retries => {
                    warn!(script = name, attempt, "script timed out, will retry");
                }
                Err(e) => return Err(e),
            }
        }

        match last_result {
            Some(ref result) if !result.success() => Err(PsRunnerError::ScriptFailed {
                exit_code: result.exit_code,
                stderr: result.stderr.clone(),
            }),
            Some(result) => Ok(result),
            None => Err(PsRunnerError::RetriesExhausted(max_retries)),
        }
    }

    /// Remove the temporary script directory.
    pub fn cleanup(&self) -> Result<()> {
        if self.scripts_dir.exists() {
            fs::remove_dir_all(&self.scripts_dir)?;
            info!(path = %self.scripts_dir.display(), "cleaned up scripts directory");
        }
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn spawn_direct(
        &self,
        script_path: &Path,
        args: &[String],
    ) -> std::io::Result<std::process::Child> {
        Command::new("powershell.exe")
            .args(["-ExecutionPolicy", "Bypass", "-File", &script_path.to_string_lossy()])
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }

    /// Spawn PowerShell with credentials passed via environment variables.
    /// The script reads `DRIVER_TEST_VM_USER` and `DRIVER_TEST_VM_PASSWORD`
    /// env vars and constructs a PSCredential internally.
    // fn spawn_with_credential(
    //     &self,
    //     script_path: &Path,
    //     args: &[String],
    //     credential: &VmCredential,
    // ) -> std::io::Result<std::process::Child> {
    //     Command::new("powershell.exe")
    //         .args(["-ExecutionPolicy", "Bypass", "-File", &script_path.to_string_lossy()])
    //         .args(args)
    //         .env("DRIVER_TEST_VM_USER", &credential.username)
    //         .env("DRIVER_TEST_VM_PASSWORD", &credential.password)
    //         .stdout(std::process::Stdio::piped())
    //         .stderr(std::process::Stdio::piped())
    //         .spawn()
    // }

    /// Spawn an elevated PowerShell session using an encoded command that
    /// invokes `Start-Process -Verb RunAs`. Output is captured via temp files
    /// because `-Verb RunAs` does not support `-RedirectStandard*` parameters.
    /// The inner elevated process writes its own output to files, and the outer
    /// (non-elevated) process reads them back into the captured pipes.
    fn spawn_elevated(
        &self,
        script_path: &Path,
        args: &[String],
        credential: Option<&VmCredential>,
    ) -> std::io::Result<std::process::Child> {
        let stdout_file = self.scripts_dir.join("elevated_stdout.txt");
        let stderr_file = self.scripts_dir.join("elevated_stderr.txt");

        // Build the inner script arguments
        let script_args = args
            .iter()
            .map(|a| {
                if a.starts_with('-') {
                    a.clone()
                } else {
                    format!("'{}'", a.replace('\'', "''"))
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        // Build the inner command that the elevated process will run.
        // The inner command redirects its own output to temp files.
        let script_call = if let Some(cred) = credential {
            format!(
                "$secPass = ConvertTo-SecureString ''{}'' -AsPlainText -Force; \
                 $cred = New-Object System.Management.Automation.PSCredential(''{}'',$secPass); \
                 & ''{}'' {} -Credential $cred",
                cred.password.replace('\'', "''''"),
                cred.username.replace('\'', "''''"),
                script_path.to_string_lossy().replace('\'', "''''"),
                script_args.replace('\'', "''''"),
            )
        } else {
            format!(
                "& ''{}'' {}",
                script_path.to_string_lossy().replace('\'', "''''"),
                script_args.replace('\'', "''''"),
            )
        };

        // The elevated inner script wraps the call and redirects output to files
        let inner_script = format!(
            "$ErrorActionPreference = ''Stop''; \
             try {{ {call} *> ''{stdout}'' 2> ''{stderr}'' }} \
             catch {{ $_ | Out-File ''{stderr}'' -Append; exit 1 }}",
            call = script_call,
            stdout = stdout_file.to_string_lossy().replace('\'', "''''"),
            stderr = stderr_file.to_string_lossy().replace('\'', "''''"),
        );

        // The outer command: encode the inner script, launch elevated, wait,
        // then read the temp files back to stdout/stderr for the Rust process.
        let ps_command = format!(
            "$ErrorActionPreference = 'Stop'; \
             $innerBytes = [System.Text.Encoding]::Unicode.GetBytes('{inner}'); \
             $innerCmd = [Convert]::ToBase64String($innerBytes); \
             $proc = Start-Process powershell.exe \
               -Verb RunAs \
               -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand',$innerCmd \
               -Wait -PassThru -WindowStyle Hidden; \
             if (Test-Path '{stdout}') {{ Get-Content '{stdout}' -Raw }}; \
             if (Test-Path '{stderr}') {{ Get-Content '{stderr}' -Raw | Write-Error }}; \
             exit $proc.ExitCode",
            inner = inner_script.replace('\'', "''"),
            stdout = stdout_file.to_string_lossy().replace('\'', "''"),
            stderr = stderr_file.to_string_lossy().replace('\'', "''"),
        );

        let encoded = encode_powershell_command(&ps_command);

        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &encoded,
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    }
}

impl Drop for PsRunner {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            warn!(error = %e, "failed to clean up scripts directory on drop");
        }
    }
}

// ---------------------------------------------------------------------------
// UTF-16LE base64 encoding for -EncodedCommand
// ---------------------------------------------------------------------------

/// Encode a PowerShell command as a UTF-16LE base64 string for use with
/// `powershell.exe -EncodedCommand`.
fn encode_powershell_command(command: &str) -> String {
    let utf16_bytes: Vec<u8> = command
        .encode_utf16()
        .flat_map(|c| [(c & 0xFF) as u8, (c >> 8) as u8])
        .collect();

    base64_encode(&utf16_bytes)
}

/// Minimal base64 encoder (avoids pulling in an external crate).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encoded_command_is_nonempty_base64() {
        let encoded = encode_powershell_command("Write-Host 'hi'");
        assert!(!encoded.is_empty());
        assert!(encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn script_result_success_predicate() {
        let ok = ScriptResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            duration: Duration::from_millis(100),
        };
        assert!(ok.success());

        let fail = ScriptResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "boom".into(),
            duration: Duration::from_millis(200),
        };
        assert!(!fail.success());
    }

    #[test]
    fn embedded_scripts_map_has_all_entries() {
        let map = embedded_scripts();
        assert_eq!(map.len(), 3);
        assert!(map.contains_key("Install-DriverOnVM.ps1"));
        assert!(map.contains_key("Verify-DriverOnVM.ps1"));
        assert!(map.contains_key("Capture-DriverLogs.ps1"));
    }
}
