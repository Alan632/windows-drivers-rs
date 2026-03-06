<#
.SYNOPSIS
    Captures DbgPrint/DbgPrintEx kernel debug output from a Hyper-V VM using
    DebugView (Dbgview.exe -l).

.DESCRIPTION
    Connects to a Hyper-V VM via PowerShell Direct, ensures DebugView is
    available inside the guest, configures the Debug Print Filter registry key
    to capture all components, and runs Dbgview.exe in log-to-file mode for a
    specified duration. The resulting log file is copied back to the host.

.PARAMETER VMName
    Name of the Hyper-V VM to capture debug output from.

.PARAMETER OutputPath
    Path on the host where the captured log file will be saved.
    Defaults to DebugView_<VMName>_<timestamp>.log on the user's Desktop.

.PARAMETER Duration
    Number of seconds to capture debug output. Default is 60.

.PARAMETER Credential
    Optional PSCredential for the VM. If omitted, the script uses a default
    test credential (DriverTestAdmin / 1234).

.PARAMETER DbgViewPath
    Optional path to Dbgview.exe on the host. If omitted, the script downloads
    it from live.sysinternals.com into the guest directly.

.PARAMETER FilterMask
    Debug Print Filter mask value (DWORD). Default is 0xFFFFFFFF (capture all
    components at all levels). Applied to
    HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print Filter.

.EXAMPLE
    .\Capture-DbgPrintToFile.ps1 -VMName "TestVM"

.EXAMPLE
    .\Capture-DbgPrintToFile.ps1 -VMName "TestVM" -Duration 120 -OutputPath "C:\logs\dbg.log"

.EXAMPLE
    .\Capture-DbgPrintToFile.ps1 -VMName "TestVM" -DbgViewPath "C:\Tools\Dbgview.exe"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [Parameter()]
    [string]$OutputPath,

    [Parameter()]
    [ValidateRange(1, 3600)]
    [int]$Duration = 60,

    [Parameter()]
    [PSCredential]$Credential,

    [Parameter()]
    [string]$DbgViewPath,

    [Parameter()]
    [int]$FilterMask = -1
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$EXIT_SUCCESS       = 0
$EXIT_VM_NOT_FOUND  = 1
$EXIT_SESSION_FAIL  = 2
$EXIT_SETUP_FAIL    = 3
$EXIT_CAPTURE_FAIL  = 4
$EXIT_COPY_FAIL     = 5

# Default output path
if (-not $OutputPath) {
    $timestamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    $OutputPath = Join-Path ([Environment]::GetFolderPath('Desktop')) "DebugView_${VMName}_${timestamp}.log"
}

# Ensure output directory exists on host
$outputDir = Split-Path -Path $OutputPath -Parent
if ($outputDir -and -not (Test-Path $outputDir)) {
    New-Item -Path $outputDir -ItemType Directory -Force | Out-Null
}

# ── VM state check ──────────────────────────────────────────────────────────

Write-Host "Checking VM '$VMName'..."
try {
    $vm = Get-VM -Name $VMName -ErrorAction Stop
    if ($vm.State -ne 'Running') {
        Write-Host "Starting VM '$VMName'..."
        Start-VM -Name $VMName
        Wait-VM -Name $VMName -For Heartbeat -Timeout 300
        Write-Host "VM is ready."
    }
} catch [Microsoft.HyperV.PowerShell.Commands.GetVM] {
    Write-Error "VM '$VMName' not found."
    exit $EXIT_VM_NOT_FOUND
} catch {
    Write-Host "Note: Cannot query VM state (Hyper-V admin may be required). Assuming VM is running."
}

# ── Credential setup ────────────────────────────────────────────────────────

$cred = $null
if ($Credential) {
    $cred = $Credential
} else {
    $ss = New-Object System.Security.SecureString
    '1234'.ToCharArray() | ForEach-Object { $ss.AppendChar($_) }
    $cred = New-Object System.Management.Automation.PSCredential('DriverTestAdmin', $ss)
}

function New-GuestSession {
    if ($cred) {
        New-PSSession -VMName $VMName -Credential $cred
    } else {
        New-PSSession -VMName $VMName
    }
}

# ── Connect to VM ───────────────────────────────────────────────────────────

Write-Host "Connecting to VM '$VMName' via PS Direct..."
try {
    $session = New-GuestSession
} catch {
    Write-Error "Failed to create PS Direct session to '$VMName': $_"
    exit $EXIT_SESSION_FAIL
}

$GuestWorkDir = "C:\DebugViewCapture-$([System.IO.Path]::GetRandomFileName().Split('.')[0])"
$GuestLogFile = "$GuestWorkDir\dbgprint.log"
$GuestDbgView = "$GuestWorkDir\Dbgview.exe"

try {
    # ── Create work directory in guest ──────────────────────────────────────

    Invoke-Command -Session $session -ScriptBlock {
        param($Dir)
        if (Test-Path $Dir) { Remove-Item -Path $Dir -Recurse -Force }
        New-Item -Path $Dir -ItemType Directory -Force | Out-Null
    } -ArgumentList $GuestWorkDir

    # ── Provision DebugView in guest ────────────────────────────────────────

    if ($DbgViewPath) {
        # Copy from host
        if (-not (Test-Path $DbgViewPath -PathType Leaf)) {
            Write-Error "DbgViewPath '$DbgViewPath' not found on host."
            exit $EXIT_SETUP_FAIL
        }
        Write-Host "Copying DebugView from host to VM..."
        Copy-Item -ToSession $session -Path $DbgViewPath -Destination $GuestDbgView -Force
    } else {
        # Download directly inside the guest
        Write-Host "Downloading DebugView inside VM..."
        Invoke-Command -Session $session -ScriptBlock {
            param($Dest)
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri 'https://live.sysinternals.com/Dbgview.exe' `
                -OutFile $Dest -UseBasicParsing
            if (-not (Test-Path $Dest)) {
                throw "Failed to download Dbgview.exe"
            }
        } -ArgumentList $GuestDbgView
    }

    # Accept the Sysinternals EULA silently
    Invoke-Command -Session $session -ScriptBlock {
        $eulaKey = 'HKCU:\Software\Sysinternals\DebugView'
        if (-not (Test-Path $eulaKey)) {
            New-Item -Path $eulaKey -Force | Out-Null
        }
        Set-ItemProperty -Path $eulaKey -Name 'EulaAccepted' -Value 1 -Type DWord
    }

    # ── Configure Debug Print Filter ────────────────────────────────────────

    $maskHex = '{0:X8}' -f ([BitConverter]::ToUInt32([BitConverter]::GetBytes($FilterMask), 0))
    Write-Host "Configuring Debug Print Filter (mask = 0x$maskHex)..."
    Invoke-Command -Session $session -ScriptBlock {
        param($Mask)
        $filterKey = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print Filter'
        if (-not (Test-Path $filterKey)) {
            New-Item -Path $filterKey -Force | Out-Null
        }
        # Write as unsigned DWORD — [int]-1 becomes 0xFFFFFFFF
        Set-ItemProperty -Path $filterKey -Name 'DEFAULT' -Value $Mask -Type DWord
    } -ArgumentList $FilterMask

    # ── Run DebugView capture ───────────────────────────────────────────────

    Write-Host "Starting DebugView capture for ${Duration}s..."
    Write-Host "  Guest log file: $GuestLogFile"

    Invoke-Command -Session $session -ScriptBlock {
        param($DbgExe, $LogFile, $Seconds)

        # Start Dbgview.exe in log mode:
        #   /accepteula  - accept EULA silently
        #   /l <file>    - log output to file
        #   /k           - capture kernel output (DbgPrint/DbgPrintEx)
        $proc = Start-Process -FilePath $DbgExe `
            -ArgumentList '/accepteula', '/l', $LogFile, '/k' `
            -PassThru -WindowStyle Hidden

        if (-not $proc -or $proc.HasExited) {
            throw "Dbgview.exe failed to start."
        }

        Write-Host "DebugView started (PID: $($proc.Id))"

        # Let it capture for the requested duration
        Start-Sleep -Seconds $Seconds

        # Stop DebugView gracefully
        try {
            $proc | Stop-Process -Force -ErrorAction SilentlyContinue
        } catch { }

        # Brief pause to let the file be flushed
        Start-Sleep -Seconds 2

        # Report results
        if (Test-Path $LogFile) {
            $info = Get-Item $LogFile
            Write-Host "Log captured: $($info.Length) bytes"
        } else {
            Write-Host "WARNING: Log file was not created."
        }
    } -ArgumentList $GuestDbgView, $GuestLogFile, $Duration

    # ── Copy log back to host ───────────────────────────────────────────────

    Write-Host "Copying log to host: $OutputPath"
    try {
        Copy-Item -FromSession $session -Path $GuestLogFile -Destination $OutputPath -Force
        $hostFile = Get-Item $OutputPath
        Write-Host "Log saved: $OutputPath ($($hostFile.Length) bytes)"
    } catch {
        Write-Warning "Could not copy log from VM: $_"
        Write-Host "The log remains in the guest at: $GuestLogFile"
        exit $EXIT_COPY_FAIL
    }

    # ── Cleanup guest ───────────────────────────────────────────────────────

    Write-Host "Cleaning up guest work directory..."
    Invoke-Command -Session $session -ScriptBlock {
        param($Dir)
        if (Test-Path $Dir) { Remove-Item -Path $Dir -Recurse -Force -ErrorAction SilentlyContinue }
    } -ArgumentList $GuestWorkDir
    Write-Host " done."

    Write-Host ""
    Write-Host "Done. Captured ${Duration}s of DbgPrint output from '$VMName'."
    Write-Host "  Output: $OutputPath"
    exit $EXIT_SUCCESS

} catch {
    Write-Error "Capture failed: $_"
    exit $EXIT_CAPTURE_FAIL
} finally {
    if ($session) {
        Remove-PSSession -Session $session -ErrorAction SilentlyContinue
    }
}
