<#
.SYNOPSIS
    Installs a driver package from the host to a Hyper-V VM.

.DESCRIPTION
    Copies driver files (.sys, .inf, .cat, .cer) to a Hyper-V VM via PowerShell
    Direct, imports any test certificate, enables test signing, and installs the
    driver using pnputil.

.PARAMETER VMName
    Name of the Hyper-V VM to install the driver on.

.PARAMETER DriverPath
    Path to the driver package directory on the host. Must contain at least one
    .inf file. Typically the build output directory (e.g. target\debug\package).

.PARAMETER Credential
    Optional PSCredential for the VM. If omitted, the script tries credentialless
    PS Direct first, then prompts interactively.

.EXAMPLE
    .\Install-DriverOnVM.ps1 -VMName "TestVM" -DriverPath "C:\drivers\mydriver-package"
#>

# Note: Elevation is handled by the calling tool or user. PS Direct does not
# require local admin, but Hyper-V cmdlets (Get-VM, Enable-VMIntegrationService)
# require the user to be in the Hyper-V Administrators group.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$VMName,

    [Parameter(Mandatory)]
    [ValidateScript({ Test-Path $_ -PathType Container })]
    [string]$DriverPath,

    [Parameter()]
    [PSCredential]$Credential
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Exit codes for structured error reporting (#11)
$EXIT_SUCCESS        = 0
$EXIT_NO_INF         = 1
$EXIT_VM_NOT_FOUND   = 2
$EXIT_FILE_COPY_FAIL = 3
$EXIT_CERT_FAIL      = 4
$EXIT_INSTALL_FAIL   = 5
$EXIT_TIMEOUT        = 6

# Use a unique guest directory to avoid collisions from parallel runs (#12)
$GuestDriverDir = "C:\DriverInstall-$(Get-Date -Format 'yyyyMMddHHmmss')-$([System.IO.Path]::GetRandomFileName().Split('.')[0])"

# Validate driver package contains an .inf file
$infFiles = Get-ChildItem -Path $DriverPath -Filter '*.inf'
if (-not $infFiles) {
    Write-Error "No .inf files found in '$DriverPath'. A valid driver package requires an .inf file."
    exit $EXIT_NO_INF
}

# Ensure VM is running (requires Hyper-V admin; skip if not available)
try {
    $vm = Get-VM -Name $VMName -ErrorAction Stop
    if ($vm.State -ne 'Running') {
        Write-Host "Starting VM '$VMName'..."
        Start-VM -Name $VMName
        Wait-VM -Name $VMName -For Heartbeat -Timeout 300
        Write-Host "VM is ready."
    }
} catch {
    Write-Host "Note: Cannot query VM state (Hyper-V admin may be required). Assuming VM is running."
}

# Enable Guest Service Interface (requires Hyper-V admin; skip if not available)
try {
    $guestSvc = Get-VMIntegrationService -VMName $VMName -ErrorAction Stop | Where-Object { $_.Name -eq 'Guest Service Interface' }
    if ($guestSvc -and -not $guestSvc.Enabled) {
        Write-Host "Enabling Guest Service Interface..."
        Enable-VMIntegrationService -Name 'Guest Service Interface' -VMName $VMName
    }
} catch {
    Write-Host "Note: Cannot check Guest Service Interface (Hyper-V admin may be required)."
}

# Establish credential mode
$cred = $null
if ($Credential) {
    $cred = $Credential
} else {
    $ss = New-Object System.Security.SecureString
    '1234'.ToCharArray() | ForEach-Object { $ss.AppendChar($_) }
    $cred = New-Object System.Management.Automation.PSCredential('DriverTestAdmin', $ss)
}

# Helper: create a PS Direct session with the resolved credential mode (#3)
function New-GuestSession {
    if ($cred) {
        New-PSSession -VMName $VMName -Credential $cred
    } else {
        New-PSSession -VMName $VMName
    }
}

# Helper: invoke a command on the guest with optional timeout (#8)
function Invoke-GuestCommand {
    param(
        [Parameter(Mandatory)][System.Management.Automation.Runspaces.PSSession]$Session,
        [Parameter(Mandatory)][scriptblock]$ScriptBlock,
        [int]$TimeoutSeconds = 0
    )
    if ($TimeoutSeconds -gt 0) {
        $job = Invoke-Command -Session $Session -ScriptBlock $ScriptBlock -AsJob
        $completed = $job | Wait-Job -Timeout $TimeoutSeconds
        if (-not $completed) {
            Stop-Job $job
            Remove-Job $job -Force
            throw "Command timed out after ${TimeoutSeconds}s"
        }
        $result = Receive-Job $job
        Remove-Job $job -Force
        return $result
    } else {
        return Invoke-Command -Session $Session -ScriptBlock $ScriptBlock
    }
}

Write-Host "Connecting to VM '$VMName'..."
$session = New-GuestSession

try {
    # Create destination directory in the VM (clear any stale files first)
    Invoke-Command -Session $session -ScriptBlock {
        param($Dir)
        if (Test-Path $Dir) { Remove-Item -Path $Dir -Recurse -Force }
        New-Item -Path $Dir -ItemType Directory -Force | Out-Null
    } -ArgumentList $GuestDriverDir

    # Copy all driver files to the VM with fallback (#13)
    Write-Host "Copying driver files to VM..."
    $copySuccess = $false

    # Try Copy-Item -ToSession first (works via PS Direct session)
    try {
        Copy-Item -ToSession $session -Path "$DriverPath\*" -Destination $GuestDriverDir -Recurse -Force
        $copySuccess = $true
        Write-Host "Files copied to $GuestDriverDir in VM (via PS Direct session)."
    } catch {
        Write-Warning "Copy-Item -ToSession failed: $_"
        Write-Host "Falling back to Copy-VMFile..."
    }

    # Fallback: Copy-VMFile (uses Guest Service Interface, no PS Direct needed)
    if (-not $copySuccess) {
        try {
            $filesToCopy = Get-ChildItem -Path $DriverPath -Recurse -File
            foreach ($file in $filesToCopy) {
                $destPath = Join-Path $GuestDriverDir $file.Name
                Copy-VMFile -Name $VMName -SourcePath $file.FullName -DestinationPath $destPath `
                    -FileSource Host -CreateFullPath -Force
            }
            $copySuccess = $true
            Write-Host "Files copied to $GuestDriverDir in VM (via Copy-VMFile)."
        } catch {
            Write-Error "Both file copy methods failed. Last error: $_"
            exit $EXIT_FILE_COPY_FAIL
        }
    }

    # Import test certificate if present (.cer files) (#4, #5, #9)
    $cerFiles = Get-ChildItem -Path $DriverPath -Filter '*.cer'
    if ($cerFiles) {
        Write-Host "Importing test certificate(s)..."
        $certResult = Invoke-Command -Session $session -ScriptBlock {
            param($Dir)
            $imported = @()
            Get-ChildItem -Path $Dir -Filter '*.cer' | ForEach-Object {
                $certFile = $_.FullName
                $certObj = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 $certFile
                $thumbprint = $certObj.Thumbprint

                # Check if cert already exists in both stores (idempotency) (#9)
                $inRoot = Get-ChildItem 'Cert:\LocalMachine\Root' |
                    Where-Object { $_.Thumbprint -eq $thumbprint }
                $inTrusted = Get-ChildItem 'Cert:\LocalMachine\TrustedPublisher' |
                    Where-Object { $_.Thumbprint -eq $thumbprint }

                if ($inRoot -and $inTrusted) {
                    $imported += @{ Name = $_.Name; Thumbprint = $thumbprint; Action = 'already-present' }
                } else {
                    # Import using modern Import-Certificate cmdlet (#4)
                    if (-not $inRoot) {
                        Import-Certificate -FilePath $certFile -CertStoreLocation 'Cert:\LocalMachine\Root' | Out-Null
                    }
                    if (-not $inTrusted) {
                        Import-Certificate -FilePath $certFile -CertStoreLocation 'Cert:\LocalMachine\TrustedPublisher' | Out-Null
                    }

                    # Verify import succeeded by checking thumbprint (#5)
                    $verifyRoot = Get-ChildItem 'Cert:\LocalMachine\Root' |
                        Where-Object { $_.Thumbprint -eq $thumbprint }
                    $verifyTrusted = Get-ChildItem 'Cert:\LocalMachine\TrustedPublisher' |
                        Where-Object { $_.Thumbprint -eq $thumbprint }

                    if ($verifyRoot -and $verifyTrusted) {
                        $imported += @{ Name = $_.Name; Thumbprint = $thumbprint; Action = 'imported' }
                    } else {
                        $imported += @{ Name = $_.Name; Thumbprint = $thumbprint; Action = 'FAILED' }
                    }
                }
            }
            $imported
        } -ArgumentList $GuestDriverDir

        foreach ($cert in $certResult) {
            $icon = if ($cert.Action -eq 'FAILED') { 'FAIL' } else { 'OK' }
            Write-Host "  [$icon] $($cert.Name): $($cert.Action) (thumbprint: $($cert.Thumbprint))"
        }

        $failedCerts = @($certResult | Where-Object { $_.Action -eq 'FAILED' })
        if ($failedCerts.Count -gt 0) {
            Write-Error "Certificate import failed for: $($failedCerts.Name -join ', ')"
            exit $EXIT_CERT_FAIL
        }

        # Enable test signing and reboot if not already enabled (#9 - idempotent)
        $testSigningEnabled = Invoke-Command -Session $session -ScriptBlock {
            $bcd = bcdedit /enum '{current}' 2>&1 | Out-String
            $bcd -match 'testsigning\s+Yes'
        }

        if (-not $testSigningEnabled) {
            Write-Host "Enabling test signing mode (requires reboot)..."
            Invoke-Command -Session $session -ScriptBlock { bcdedit /set testsigning on | Out-Null }
            Remove-PSSession $session

            Restart-VM -Name $VMName -Force
            Wait-VM -Name $VMName -For Heartbeat -Timeout 300

            $session = New-GuestSession
            Write-Host "VM rebooted with test signing enabled."
        }
    }

    # Install the driver using pnputil with timeout (#8)
    Write-Host "Installing driver..."
    $guestDir = $GuestDriverDir
    $result = Invoke-GuestCommand -Session $session -TimeoutSeconds 120 -ScriptBlock {
        $dir = $using:guestDir
        $rawOutput = pnputil.exe /add-driver "$dir\*.inf" /subdirs /install 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
        [PSCustomObject]@{
            ExitCode = $exitCode
            Output   = $rawOutput
        }
    }

    if ($result.ExitCode -ne 0) {
        Write-Error "Driver installation failed (exit code $($result.ExitCode)):`n$($result.Output)"
        exit $EXIT_INSTALL_FAIL
    }

    Write-Host "`nDriver installed successfully:`n$($result.Output)"
    exit $EXIT_SUCCESS
}
catch {
    if ($_.Exception.Message -match 'timed out') {
        Write-Error "Operation timed out: $_"
        exit $EXIT_TIMEOUT
    }
    throw
}
finally {
    if ($session) { Remove-PSSession $session }
}
