<#
.SYNOPSIS
    Creates or reverts Hyper-V VM snapshots.

.DESCRIPTION
    Manages Hyper-V VM checkpoints (snapshots). Can create a new snapshot or
    revert the VM to an existing one. When reverting, the VM is restarted if
    it was running before the revert. Attempts to run unelevated first and
    auto-elevates only if needed.

.PARAMETER VMName
    Name of the Hyper-V VM.

.PARAMETER Action
    Either 'Create' to take a new snapshot or 'Revert' to restore an existing one.

.PARAMETER SnapshotName
    Optional name for the snapshot. Defaults to "<VMName>-<yyyyMMdd-HHmmss>".

.EXAMPLE
    .\Manage-VMSnapshot.ps1 -VMName "driver-test-vm" -Action Create
.EXAMPLE
    .\Manage-VMSnapshot.ps1 -VMName "driver-test-vm" -Action Revert -SnapshotName "driver-test-vm-20260305-120000"
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [Parameter(Mandatory)]
    [ValidateSet('Create', 'Revert', 'Delete')]
    [string]$Action,

    [Parameter()]
    [string]$SnapshotName,

    # Internal flag -- set when the script re-launches itself elevated
    [switch]$AlreadyElevated
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ---------------------------------------------------------------------------
# Auto-elevation: try unelevated first, re-launch elevated if access denied
# ---------------------------------------------------------------------------
function Test-IsAdmin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]$identity
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# If we haven't already elevated, try the operation unelevated first
if (-not $AlreadyElevated -and -not (Test-IsAdmin)) {
    try {
        # Quick probe -- if this succeeds, we don't need elevation
        Get-VM -Name $VMName -ErrorAction Stop | Out-Null
        Write-Host "Running without elevation (Hyper-V access OK)."
    } catch {
        $msg = $_.Exception.Message
        $isDenied = ($msg -match 'permission|access|denied|authorization') -or ($_.CategoryInfo.Category -eq 'PermissionDenied')
        if ($isDenied) {
            Write-Host "Elevation required -- re-launching as administrator..."

            # Rebuild argument list for the elevated invocation
            $argList = @(
                '-NoProfile', '-ExecutionPolicy', 'Bypass',
                '-File', $MyInvocation.MyCommand.Path,
                '-VMName', $VMName,
                '-Action', $Action,
                '-AlreadyElevated'
            )
            if ($SnapshotName) { $argList += '-SnapshotName'; $argList += $SnapshotName }

            $proc = Start-Process powershell -Verb RunAs -ArgumentList $argList -Wait -PassThru
            exit $proc.ExitCode
        }
        # Not a permission error -- let it fall through to the normal error path
        throw
    }
}

# Default snapshot name: VMName-timestamp
if (-not $SnapshotName) {
    $SnapshotName = "$VMName-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
}

# Validate VM exists
try {
    $vm = Get-VM -Name $VMName
} catch {
    Write-Host "VM '$VMName' not found: $_" -ForegroundColor Red
    exit 1
}

switch ($Action) {
    'Create' {
        Write-Host "Creating snapshot '$SnapshotName' for VM '$VMName'..."
        Checkpoint-VM -Name $VMName -SnapshotName $SnapshotName
        $snap = Get-VMSnapshot -VMName $VMName -Name $SnapshotName
        Write-Host "Snapshot created at $($snap.CreationTime)"
    }

    'Revert' {
        $snap = Get-VMSnapshot -VMName $VMName -Name $SnapshotName -ErrorAction SilentlyContinue
        if (-not $snap) {
            Write-Host "Snapshot '$SnapshotName' not found on VM '$VMName'." -ForegroundColor Red
            Write-Host "Available snapshots:"
            Get-VMSnapshot -VMName $VMName | ForEach-Object {
                Write-Host "  - $($_.Name) (created $($_.CreationTime))"
            }
            exit 1
        }

        $wasRunning = $vm.State -eq 'Running'
        Write-Host "Reverting VM '$VMName' to snapshot '$SnapshotName'..."
        Restore-VMSnapshot -VMSnapshot $snap -Confirm:$false

        Start-Sleep -Seconds 2
        $postRevertState = (Get-VM -Name $VMName).State
        if ($wasRunning -and $postRevertState -ne 'Running') {
            Write-Host "Restarting VM..."
            Start-VM -Name $VMName
            Wait-VM -Name $VMName -For Heartbeat -Timeout 300
        }

        Write-Host "VM reverted to '$SnapshotName'."
    }

    'Delete' {
        if (-not $PSBoundParameters.ContainsKey('SnapshotName')) {
            Write-Host "A -SnapshotName must be provided for the Delete action." -ForegroundColor Red
            exit 1
        }

        $snap = Get-VMSnapshot -VMName $VMName -Name $SnapshotName -ErrorAction SilentlyContinue
        if (-not $snap) {
            Write-Host "Snapshot '$SnapshotName' not found on VM '$VMName'. Nothing to delete." -ForegroundColor Yellow
            Write-Host "Available snapshots:"
            Get-VMSnapshot -VMName $VMName | ForEach-Object {
                Write-Host "  - $($_.Name) (created $($_.CreationTime))"
            }
            exit 1
        }

        Write-Host "Deleting snapshot '$SnapshotName' from VM '$VMName'..."
        Remove-VMSnapshot -VMName $VMName -Name $SnapshotName -Confirm:$false
        Write-Host "Snapshot deleted."
    }
}
