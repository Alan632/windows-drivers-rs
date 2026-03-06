// Copyright (c) Microsoft Corporation
// License: MIT OR Apache-2.0

//! Start and stop DebugView + logman ETW trace capture on a Hyper-V VM.
//!
//! The three public entry-points mirror the lifecycle of a capture session:
//!
//! 1. [`start_capture`] — configures the guest, launches DebugView in
//!    log-to-file mode, and starts a `logman` ETW trace session.
//! 2. [`wait_capture`]  — sleeps for the configured duration with periodic
//!    progress output.
//! 3. [`stop_and_extract`] — stops both capture methods, decodes the ETL
//!    file to XML via `tracerpt`, and copies all artefacts back to the host.
//!
//! Each function writes a small temporary `.ps1` script into the
//! [`PsRunner`]'s scripts directory and executes it via
//! [`PsRunner::run_script`], reusing the existing credential and timeout
//! handling.

use std::path::PathBuf;
use std::time::Duration;

use tracing::info;

use crate::ps_runner::{PsRunner, PsRunnerError};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a capture session.
pub struct CaptureConfig {
    /// Hyper-V VM name.
    pub vm_name: String,
    /// Seconds to wait after verification before stopping capture.
    pub capture_duration: u32,
    /// Host directory where extracted log files will be placed.
    pub output_dir: PathBuf,
}

// ---------------------------------------------------------------------------
// Session handle
// ---------------------------------------------------------------------------

/// Handles for a running capture session (returned by [`start_capture`]).
pub struct CaptureSession {
    /// Hyper-V VM name.
    pub vm_name: String,
    /// logman ETW session name (e.g. `"DriverTrace_20260305_120000"`).
    pub etw_session_name: String,
    /// Guest directory that holds all log artefacts.
    pub guest_log_dir: String,
    /// Guest path to the DebugView log file.
    pub guest_dbgview_log: String,
    /// Guest path to the ETL file.
    pub guest_etl_path: String,
    /// Host output directory (copied from [`CaptureConfig`]).
    pub output_dir: PathBuf,
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Files extracted from the VM after capture.
pub struct CaptureResult {
    /// Host path to the copied DebugView log (if successfully copied).
    pub dbgview_log: Option<PathBuf>,
    /// Host path to the copied ETL file (if successfully copied).
    pub etl_file: Option<PathBuf>,
    /// Host path to the decoded XML file (if successfully copied).
    pub xml_file: Option<PathBuf>,
    /// Host path to the tracerpt summary file (if successfully copied).
    pub summary_file: Option<PathBuf>,
    /// Host output directory.
    pub output_dir: PathBuf,
}

// ---------------------------------------------------------------------------
// KMDF framework ETW provider GUID
// ---------------------------------------------------------------------------

/// KMDFv1 Trace Provider — always captured.
const KMDF_PROVIDER_GUID: &str = "{544D4C9D-942C-46D5-BF50-DF5CD9524A50}";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start both DebugView and logman ETW capture on the VM.
///
/// Writes a temporary `_start_capture.ps1` into the runner's scripts
/// directory and executes it.  Returns a [`CaptureSession`] that must be
/// passed to [`stop_and_extract`] later.
pub fn start_capture(
    runner: &PsRunner,
    config: &CaptureConfig,
) -> Result<CaptureSession, PsRunnerError> {
    let timestamp = chrono_timestamp();
    let session_name = format!("DriverTrace_{timestamp}");
    let guest_log_dir = r"C:\DriverLogs".to_string();
    let guest_dbgview_log = format!(r"C:\DriverLogs\dbgview_{timestamp}.log");
    let guest_etl_path = format!(r"C:\DriverLogs\{session_name}.etl");

    info!(
        vm = %config.vm_name,
        session = %session_name,
        "starting capture (DebugView + ETW)"
    );

    let script = format!(
        r#"param(
    [Parameter(Mandatory)][string]$VMName,
    [Parameter(Mandatory)][string]$LogDir,
    [Parameter(Mandatory)][string]$DbgViewLog,
    [Parameter(Mandatory)][string]$EtlPath,
    [Parameter(Mandatory)][string]$SessionName,
    [Parameter(Mandatory)][string]$ProviderGuid
)

$ErrorActionPreference = 'Stop'

# --- credential setup (mirrors existing scripts) ---
$ss = New-Object System.Security.SecureString
'1234'.ToCharArray() | ForEach-Object {{ $ss.AppendChar($_) }}
$cred = New-Object System.Management.Automation.PSCredential('DriverTestAdmin', $ss)
$session = New-PSSession -VMName $VMName -Credential $cred

try {{
    # 1. Create log directory on the guest
    Invoke-Command -Session $session -ArgumentList $LogDir -ScriptBlock {{
        param($Dir)
        New-Item -Path $Dir -ItemType Directory -Force | Out-Null
    }}

    # 2. Set DbgPrint filter to capture all components
    Invoke-Command -Session $session -ScriptBlock {{
        $regPath = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print Filter'
        if (-not (Test-Path $regPath)) {{ New-Item -Path $regPath -Force | Out-Null }}
        Set-ItemProperty -Path $regPath -Name 'DEFAULT' -Value ([int]-1) -Type DWord
    }}
    Write-Host 'DbgPrint filter configured.'

    # 3. Provision DebugView in the guest
    $hostDbgView = $null
    if ($env:DBGVIEW_PATH -and (Test-Path $env:DBGVIEW_PATH)) {{
        $hostDbgView = $env:DBGVIEW_PATH
    }} else {{
        $hostDbgView = Join-Path $env:TEMP 'Dbgview.exe'
        if (-not (Test-Path $hostDbgView)) {{
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri 'https://live.sysinternals.com/Dbgview.exe' `
                -OutFile $hostDbgView -UseBasicParsing
        }}
    }}
    $vmDbgView = "$LogDir\Dbgview.exe"
    Copy-Item -ToSession $session -Path $hostDbgView -Destination $vmDbgView -Force

    # 4. Accept DebugView EULA and start it
    Invoke-Command -Session $session -ArgumentList $vmDbgView, $DbgViewLog -ScriptBlock {{
        param($ExePath, $LogPath)
        $eulaKey = 'HKCU:\Software\Sysinternals\DbgView'
        if (-not (Test-Path $eulaKey)) {{ New-Item -Path $eulaKey -Force | Out-Null }}
        Set-ItemProperty -Path $eulaKey -Name 'EulaAccepted' -Value 1 -Type DWord
        Start-Process -FilePath $ExePath `
            -ArgumentList "/accepteula /k /g /t /l `"$LogPath`"" `
            -WindowStyle Hidden
    }}
    Write-Host 'DebugView started.'

    # 5. Start logman ETW session
    Invoke-Command -Session $session -ArgumentList $SessionName, $EtlPath, $ProviderGuid -ScriptBlock {{
        param($SName, $EPath, $PGuid)
        logman stop $SName -ets 2>&1 | Out-Null
        $out = cmd /c "logman start $SName -p `"$PGuid`" 0xFFFFFFFF 0xFF -o `"$EPath`" -ets" 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) {{ throw "logman start failed: $out" }}
    }}
    Write-Host 'ETW trace session started.'

    Write-Host 'Capture started successfully.'
}} finally {{
    Remove-PSSession $session -ErrorAction SilentlyContinue
}}
"#
    );

    runner.write_script("_start_capture.ps1", &script)?;

    let args = vec![
        "-VMName".into(),
        config.vm_name.clone(),
        "-LogDir".into(),
        guest_log_dir.clone(),
        "-DbgViewLog".into(),
        guest_dbgview_log.clone(),
        "-EtlPath".into(),
        guest_etl_path.clone(),
        "-SessionName".into(),
        session_name.clone(),
        "-ProviderGuid".into(),
        KMDF_PROVIDER_GUID.to_string(),
    ];

    let timeout = Duration::from_secs(180);
    let result = runner.run_script("_start_capture.ps1", &args, timeout, None, false)?;

    if !result.success() {
        return Err(PsRunnerError::ScriptFailed {
            exit_code: result.exit_code,
            stderr: result.stderr,
        });
    }

    Ok(CaptureSession {
        vm_name: config.vm_name.clone(),
        etw_session_name: session_name,
        guest_log_dir,
        guest_dbgview_log,
        guest_etl_path,
        output_dir: config.output_dir.clone(),
    })
}

/// Wait for the capture duration, logging progress every five seconds.
pub fn wait_capture(duration_secs: u32) {
    info!(duration_secs, "waiting for capture period");
    let step = 5u32;
    let mut elapsed = 0u32;
    while elapsed < duration_secs {
        let sleep = step.min(duration_secs - elapsed);
        std::thread::sleep(Duration::from_secs(u64::from(sleep)));
        elapsed += sleep;
        info!(elapsed, total = duration_secs, "capture progress");
    }
}

/// Stop both capture methods, decode the ETL, and copy artefacts to the host.
///
/// Writes `_stop_capture.ps1` and `_extract_capture.ps1` into the runner's
/// scripts directory and executes them in sequence.
pub fn stop_and_extract(
    runner: &PsRunner,
    session: &CaptureSession,
) -> Result<CaptureResult, PsRunnerError> {
    info!(vm = %session.vm_name, "stopping capture and extracting files");

    // -- Stop script -------------------------------------------------------
    let stop_script = format!(
        r#"param(
    [Parameter(Mandatory)][string]$VMName,
    [Parameter(Mandatory)][string]$SessionName,
    [Parameter(Mandatory)][string]$EtlPath
)

$ErrorActionPreference = 'Stop'

$ss = New-Object System.Security.SecureString
'1234'.ToCharArray() | ForEach-Object {{ $ss.AppendChar($_) }}
$cred = New-Object System.Management.Automation.PSCredential('DriverTestAdmin', $ss)
$session = New-PSSession -VMName $VMName -Credential $cred

try {{
    # Stop DebugView
    Invoke-Command -Session $session -ScriptBlock {{
        Get-Process -Name 'Dbgview' -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Seconds 2
    }}
    Write-Host 'DebugView stopped.'

    # Stop logman
    Invoke-Command -Session $session -ArgumentList $SessionName -ScriptBlock {{
        param($SName)
        logman stop $SName -ets 2>&1 | Out-Null
    }}
    Write-Host 'ETW session stopped.'

    # Decode ETL to XML via tracerpt
    Invoke-Command -Session $session -ArgumentList $EtlPath -ScriptBlock {{
        param($EPath)
        $xmlPath  = $EPath -replace '\.etl$', '.xml'
        $summPath = $EPath -replace '\.etl$', '_summary.txt'
        tracerpt $EPath -o $xmlPath -of XML -summary $summPath -y 2>&1 | Out-Null
    }}
    Write-Host 'ETL decoded.'

    Write-Host 'Capture stopped successfully.'
}} finally {{
    Remove-PSSession $session -ErrorAction SilentlyContinue
}}
"#
    );

    runner.write_script("_stop_capture.ps1", &stop_script)?;

    let stop_args = vec![
        "-VMName".into(),
        session.vm_name.clone(),
        "-SessionName".into(),
        session.etw_session_name.clone(),
        "-EtlPath".into(),
        session.guest_etl_path.clone(),
    ];

    let timeout = Duration::from_secs(120);
    let stop_result = runner.run_script("_stop_capture.ps1", &stop_args, timeout, None, false)?;

    if !stop_result.success() {
        return Err(PsRunnerError::ScriptFailed {
            exit_code: stop_result.exit_code,
            stderr: stop_result.stderr,
        });
    }

    // -- Extract script ----------------------------------------------------
    let guest_xml_path = session.guest_etl_path.replace(".etl", ".xml");
    let guest_summary_path = session.guest_etl_path.replace(".etl", "_summary.txt");

    let extract_script = format!(
        r#"param(
    [Parameter(Mandatory)][string]$VMName,
    [Parameter(Mandatory)][string]$DbgViewLog,
    [Parameter(Mandatory)][string]$EtlPath,
    [Parameter(Mandatory)][string]$XmlPath,
    [Parameter(Mandatory)][string]$SummaryPath,
    [Parameter(Mandatory)][string]$OutputDir
)

$ErrorActionPreference = 'Stop'

$ss = New-Object System.Security.SecureString
'1234'.ToCharArray() | ForEach-Object {{ $ss.AppendChar($_) }}
$cred = New-Object System.Management.Automation.PSCredential('DriverTestAdmin', $ss)
$session = New-PSSession -VMName $VMName -Credential $cred

New-Item -Path $OutputDir -ItemType Directory -Force | Out-Null

try {{
    # DebugView log
    try {{
        Copy-Item -FromSession $session -Path $DbgViewLog `
            -Destination (Join-Path $OutputDir (Split-Path $DbgViewLog -Leaf)) -Force
        Write-Host "Copied: dbgview log"
    }} catch {{ Write-Warning "Could not copy dbgview log: $_" }}

    # ETL
    try {{
        Copy-Item -FromSession $session -Path $EtlPath `
            -Destination (Join-Path $OutputDir (Split-Path $EtlPath -Leaf)) -Force
        Write-Host "Copied: ETL"
    }} catch {{ Write-Warning "Could not copy ETL: $_" }}

    # XML
    try {{
        Copy-Item -FromSession $session -Path $XmlPath `
            -Destination (Join-Path $OutputDir (Split-Path $XmlPath -Leaf)) -Force
        Write-Host "Copied: XML"
    }} catch {{ Write-Warning "Could not copy XML: $_" }}

    # Summary
    try {{
        Copy-Item -FromSession $session -Path $SummaryPath `
            -Destination (Join-Path $OutputDir (Split-Path $SummaryPath -Leaf)) -Force
        Write-Host "Copied: summary"
    }} catch {{ Write-Warning "Could not copy summary: $_" }}

    Write-Host "Extraction complete."
}} finally {{
    Remove-PSSession $session -ErrorAction SilentlyContinue
}}
"#
    );

    runner.write_script("_extract_capture.ps1", &extract_script)?;

    let extract_args = vec![
        "-VMName".into(),
        session.vm_name.clone(),
        "-DbgViewLog".into(),
        session.guest_dbgview_log.clone(),
        "-EtlPath".into(),
        session.guest_etl_path.clone(),
        "-XmlPath".into(),
        guest_xml_path.clone(),
        "-SummaryPath".into(),
        guest_summary_path.clone(),
        "-OutputDir".into(),
        session.output_dir.display().to_string(),
    ];

    let extract_result =
        runner.run_script("_extract_capture.ps1", &extract_args, timeout, None, false)?;

    if !extract_result.success() {
        return Err(PsRunnerError::ScriptFailed {
            exit_code: extract_result.exit_code,
            stderr: extract_result.stderr,
        });
    }

    // Build result with paths that should now exist on the host.
    let dbgview_leaf = PathBuf::from(&session.guest_dbgview_log)
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    let etl_leaf = PathBuf::from(&session.guest_etl_path)
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    let xml_leaf = PathBuf::from(&guest_xml_path)
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    let summary_leaf = PathBuf::from(&guest_summary_path)
        .file_name()
        .unwrap_or_default()
        .to_os_string();

    let maybe = |leaf: std::ffi::OsString| -> Option<PathBuf> {
        let p = session.output_dir.join(&leaf);
        if p.exists() { Some(p) } else { None }
    };

    Ok(CaptureResult {
        dbgview_log: maybe(dbgview_leaf),
        etl_file: maybe(etl_leaf),
        xml_file: maybe(xml_leaf),
        summary_file: maybe(summary_leaf),
        output_dir: session.output_dir.clone(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Produce a `YYYYMMDD_HHMMSS` timestamp without pulling in the `chrono`
/// crate.  Falls back to epoch-based seconds if `SystemTime` misbehaves.
fn chrono_timestamp() -> String {
    // Reuse the same format the PowerShell scripts use.
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Convert epoch seconds to a rough UTC date-time (good enough for a
    // unique-ish suffix; the guest clock determines real file timestamps).
    let secs_per_min = 60u64;
    let secs_per_hour = 3600u64;
    let secs_per_day = 86400u64;

    let total_days = now / secs_per_day;
    let time_of_day = now % secs_per_day;
    let hour = time_of_day / secs_per_hour;
    let minute = (time_of_day % secs_per_hour) / secs_per_min;
    let second = time_of_day % secs_per_min;

    // Days since 1970-01-01 → (year, month, day) via a civil-calendar
    // algorithm (Howard Hinnant's).
    let (year, month, day) = civil_from_days(total_days as i64);

    format!("{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}")
}

/// Convert a day count since 1970-01-01 to (year, month, day).
/// Algorithm from Howard Hinnant (public domain).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_format() {
        let ts = chrono_timestamp();
        // Expected format: YYYYMMDD_HHMMSS (15 chars).
        assert_eq!(ts.len(), 15, "timestamp should be 15 chars: {ts}");
        assert_eq!(&ts[8..9], "_", "separator should be underscore: {ts}");
    }

    #[test]
    fn civil_from_days_epoch() {
        let (y, m, d) = civil_from_days(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_known_date() {
        // 2025-01-01 is day 20089
        let (y, m, d) = civil_from_days(20089);
        assert_eq!((y, m, d), (2025, 1, 1));
    }

    #[test]
    fn capture_config_fields() {
        let cfg = CaptureConfig {
            vm_name: "TestVM".into(),
            capture_duration: 30,
            output_dir: PathBuf::from(r"C:\out"),
        };
        assert_eq!(cfg.vm_name, "TestVM");
        assert_eq!(cfg.capture_duration, 30);
    }
}
