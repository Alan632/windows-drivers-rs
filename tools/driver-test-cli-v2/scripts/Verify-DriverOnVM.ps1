<#
.SYNOPSIS
    Verifies the correct driver version is installed on a Hyper-V VM, creates
    the device node if missing, and course-corrects if the wrong driver is
    attached.

.DESCRIPTION
    Parses the driver .inf file from the host driver package to extract the
    expected DriverVer (version and date), provider, and hardware ID. Then
    connects to the VM via PowerShell Direct to verify the currently installed
    driver matches. If the device node does not exist, devgen.exe is used to
    create it. If there is a mismatch (wrong version, wrong provider, or driver
    not found), the script re-installs the driver package using
    Install-DriverOnVM.ps1 and ensures the correct driver binds to the device.

    Returns a structured PSCustomObject result and sets a non-zero exit code
    on failure for CI integration.

.PARAMETER VMName
    Name of the Hyper-V VM to verify.

.PARAMETER DriverPath
    Path to the driver package directory on the host. Must contain at least one
    .inf file. Typically the build output directory
    (e.g. target\debug\sample_kmdf_driver_package).

.PARAMETER Credential
    Optional PSCredential for the VM. If omitted, the script tries credentialless
    PS Direct first, then prompts interactively.

.EXAMPLE
    .\Verify-DriverOnVM.ps1 -VMName "driver-test-vm" -DriverPath "C:\drivers\sample_kmdf_driver_package"
#>

# Note: Elevation is handled by the calling tool or user.

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
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# Exit codes for structured error reporting (#10)
$EXIT_SUCCESS           = 0
$EXIT_NO_INF            = 1
$EXIT_VM_NOT_FOUND      = 2
$EXIT_SESSION_FAIL      = 3
$EXIT_DEVGEN_NOT_FOUND  = 4
$EXIT_INSTALL_FAIL      = 5
$EXIT_VERIFY_FAIL       = 6
$EXIT_TIMEOUT           = 7

# ---------------------------------------------------------------------------
# Helper: Parse a .inf file using section-aware heuristics (#11)
# ---------------------------------------------------------------------------
function Get-InfDriverMetadata {
    param([string]$InfPath)

    $content = Get-Content -Path $InfPath -Raw
    $lines = $content -split "`r?`n"

    # Parse DriverVer line: DriverVer = <date>,<version>
    $driverVersion = $null
    $driverDate = $null
    if ($content -match '(?im)^\s*DriverVer\s*=\s*([^,]+),\s*(.+)$') {
        $driverDate = $Matches[1].Trim()
        $driverVersion = $Matches[2].Trim()
    }

    # Parse Provider (may be a string substitution variable)
    $providerRaw = $null
    if ($content -match '(?im)^\s*Provider\s*=\s*(.+)$') {
        $providerRaw = $Matches[1].Trim()
    }

    # If provider is a %Variable%, resolve it from [Strings] section
    $provider = $providerRaw
    if ($providerRaw -match '^%(.+)%$') {
        $varName = $Matches[1]
        if ($content -match "(?im)^\s*$varName\s*=\s*""?([^""`r`n]+)""?") {
            $provider = $Matches[1].Trim()
        }
    }

    # Section-aware hardware ID parsing (#11): locate [Manufacturer] to find
    # model section names, then extract hardware IDs only from those sections.
    $hardwareIds = @()
    $modelSections = @()
    $inManufacturer = $false
    foreach ($line in $lines) {
        if ($line -match '^\s*\[Manufacturer\]') {
            $inManufacturer = $true
            continue
        }
        if ($inManufacturer) {
            if ($line -match '^\s*\[') { break }
            if ($line -match '^\s*;') { continue }
            # Lines like: %MfgName%=Standard,NTamd64
            if ($line -match '=\s*(\w+(?:\s*,\s*\w+)*)') {
                $parts = $Matches[1] -split '\s*,\s*'
                $modelBase = $parts[0].Trim()
                $modelSections += $modelBase
                for ($i = 1; $i -lt $parts.Count; $i++) {
                    $modelSections += "$modelBase.$($parts[$i].Trim())"
                }
            }
        }
    }

    # Parse hardware IDs from identified model sections
    if ($modelSections.Count -gt 0) {
        $inModelSection = $false
        foreach ($line in $lines) {
            if ($line -match '^\s*\[([^\]]+)\]') {
                $inModelSection = $Matches[1].Trim() -in $modelSections
                continue
            }
            if ($inModelSection -and $line -notmatch '^\s*;' -and $line -match '=\s*\w+\s*,\s*([\w\\&]+)') {
                $hwid = $Matches[1].Trim()
                if ($hwid -match '\\' -and $hwid -notin $hardwareIds) {
                    $hardwareIds += $hwid
                }
            }
        }
    }

    # Fallback: broad regex if section-aware parsing found nothing
    if ($hardwareIds.Count -eq 0) {
        $hwMatches = [regex]::Matches($content, '(?im)^[^;]*=\s*\w+,\s*([\w\\&]+)')
        foreach ($m in $hwMatches) {
            $hwid = $m.Groups[1].Value
            if ($hwid -match '\\' -and $hwid -notin $hardwareIds) {
                $hardwareIds += $hwid
            }
        }
    }

    # Parse device description from [Strings] section
    $deviceDesc = $null
    if ($content -match '(?im)^\s*DeviceDesc\s*=\s*"([^"]+)"') {
        $deviceDesc = $Matches[1].Trim()
    } elseif ($content -match '(?im)^\s*DeviceDesc\s*=\s*(.+)$') {
        $deviceDesc = $Matches[1].Trim().Trim('"')
    }

    # Parse Class
    $class = $null
    if ($content -match '(?im)^\s*Class\s*=\s*(\S+)') {
        $class = $Matches[1].Trim()
    }

    [PSCustomObject]@{
        InfFile      = Split-Path $InfPath -Leaf
        Version      = $driverVersion
        Date         = $driverDate
        Provider     = $provider
        Class        = $class
        HardwareIds  = $hardwareIds
        DeviceDesc   = $deviceDesc
    }
}

# ---------------------------------------------------------------------------
# Credential setup
# ---------------------------------------------------------------------------
$cred = $null
if ($Credential) {
    $cred = $Credential
} else {
    $ss = New-Object System.Security.SecureString
    '1234'.ToCharArray() | ForEach-Object { $ss.AppendChar($_) }
    $cred = New-Object System.Management.Automation.PSCredential('DriverTestAdmin', $ss)
}

# ---------------------------------------------------------------------------
# Helper: create a PS Direct session with the resolved credential mode (#3)
# ---------------------------------------------------------------------------
function New-GuestSession {
    if ($cred) {
        New-PSSession -VMName $VMName -Credential $cred
    } else {
        New-PSSession -VMName $VMName
    }
}

# ---------------------------------------------------------------------------
# Helper: invoke a command on the guest with optional timeout (#3, #4)
# ---------------------------------------------------------------------------
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

# ---------------------------------------------------------------------------
# Helper: Find a device on the VM using layered PnP queries (#5, #6)
# ---------------------------------------------------------------------------
function Find-DeviceOnVM {
    param(
        [System.Management.Automation.Runspaces.PSSession]$Session,
        [string[]]$HardwareIds,
        [string]$DeviceDesc,
        [string]$ExpectedVersion,
        [string]$ExpectedProvider,
        [int]$TimeoutSeconds = 120
    )

    # Capture parameters for $using: in remote scriptblock
    $findHwIds    = $HardwareIds
    $findDesc     = $DeviceDesc
    $findVersion  = $ExpectedVersion
    $findProvider = $ExpectedProvider

    Invoke-GuestCommand -Session $Session -TimeoutSeconds $TimeoutSeconds -ScriptBlock {
        $HardwareIds       = $using:findHwIds
        $DeviceDesc        = $using:findDesc
        $ExpectedVersion   = $using:findVersion
        $ExpectedProvider  = $using:findProvider

        $result = [PSCustomObject]@{
            DeviceFound     = $false
            InstanceId      = $null
            FriendlyName    = $null
            ActualVersion   = $null
            ActualProvider  = $null
            ActualInfPath   = $null
            VersionMatch    = $false
            ProviderMatch   = $false
            FullMatch       = $false
            QueryMethod     = 'none'
        }

        # --- Layer 1: pnputil /enum-drivers /format xml (#5) ---
        $driverMatch = $null
        try {
            $xmlRaw = pnputil.exe /enum-drivers /format xml 2>&1 | Out-String
            if ($LASTEXITCODE -eq 0 -and $xmlRaw -match '<') {
                $xml = [xml]$xmlRaw
                foreach ($drv in $xml.SelectNodes('//Driver')) {
                    $drvVer  = $drv.DriverVersion
                    $drvProv = $drv.ProviderName
                    if ($drvVer -eq $ExpectedVersion -and $drvProv -eq $ExpectedProvider) {
                        $driverMatch = [PSCustomObject]@{
                            PublishedName = $drv.PublishedName
                            Version       = $drvVer
                            Provider      = $drvProv
                            Class         = $drv.ClassName
                        }
                        break
                    }
                }
                if ($result.QueryMethod -eq 'none') { $result.QueryMethod = 'pnputil-xml' }
            }
        } catch {}

        # --- Layer 2: Get-CimInstance Win32_PnPSignedDriver fallback (#6) ---
        if (-not $driverMatch) {
            try {
                $cimDrivers = Get-CimInstance Win32_PnPSignedDriver -ErrorAction Stop
                foreach ($drv in $cimDrivers) {
                    if ($drv.DriverVersion -eq $ExpectedVersion -and $drv.DriverProviderName -eq $ExpectedProvider) {
                        $driverMatch = [PSCustomObject]@{
                            PublishedName = $drv.InfName
                            Version       = $drv.DriverVersion
                            Provider      = $drv.DriverProviderName
                            Class         = $drv.DeviceClass
                        }
                        $result.QueryMethod = 'cim-fallback'
                        break
                    }
                }
                if ($result.QueryMethod -eq 'none') { $result.QueryMethod = 'cim-fallback' }
            } catch {}
        }

        # --- Find the actual device by hardware ID or description ---
        $device = $null

        # Strategy 1: Find by hardware ID
        foreach ($hwid in $HardwareIds) {
            try {
                $candidates = Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
                    Where-Object {
                        try {
                            $ids = (Get-PnpDeviceProperty -InstanceId $_.InstanceId `
                                -KeyName 'DEVPKEY_Device_HardwareIds' -ErrorAction SilentlyContinue).Data
                            $ids -contains $hwid
                        } catch { $false }
                    }
                if ($candidates) {
                    $device = $candidates | Select-Object -First 1
                    if ($result.QueryMethod -eq 'none') { $result.QueryMethod = 'pnpdevice' }
                    break
                }
            } catch {}
        }

        # Strategy 2: Fall back to searching by device description
        if (-not $device -and $DeviceDesc) {
            try {
                $device = Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
                    Where-Object { $_.FriendlyName -like "*$DeviceDesc*" } |
                    Select-Object -First 1
            } catch {}
        }

        # Strategy 3: pnputil /enum-devices fallback (#6)
        if (-not $device -and $HardwareIds.Count -gt 0) {
            try {
                $devEnum = pnputil.exe /enum-devices /ids 2>&1 | Out-String
                foreach ($hwid in $HardwareIds) {
                    if ($devEnum -match [regex]::Escape($hwid)) {
                        $blocks = $devEnum -split '(?=Instance ID:)'
                        foreach ($block in $blocks) {
                            if ($block -match [regex]::Escape($hwid) -and $block -match 'Instance ID:\s*(.+)') {
                                $instId = $Matches[1].Trim()
                                try { $device = Get-PnpDevice -InstanceId $instId -ErrorAction SilentlyContinue } catch {}
                                if ($device) { break }
                            }
                        }
                        if ($device) { break }
                    }
                }
            } catch {}
        }

        if (-not $device) {
            return $result
        }

        $result.DeviceFound  = $true
        $result.InstanceId   = $device.InstanceId
        $result.FriendlyName = $device.FriendlyName

        # Query driver properties
        try {
            $result.ActualVersion = (Get-PnpDeviceProperty -InstanceId $device.InstanceId `
                -KeyName 'DEVPKEY_Device_DriverVersion' -ErrorAction SilentlyContinue).Data
        } catch {}
        try {
            $result.ActualProvider = (Get-PnpDeviceProperty -InstanceId $device.InstanceId `
                -KeyName 'DEVPKEY_Device_DriverProvider' -ErrorAction SilentlyContinue).Data
        } catch {}
        try {
            $result.ActualInfPath = (Get-PnpDeviceProperty -InstanceId $device.InstanceId `
                -KeyName 'DEVPKEY_Device_DriverInfPath' -ErrorAction SilentlyContinue).Data
        } catch {}

        $result.VersionMatch  = $result.ActualVersion -eq $ExpectedVersion
        $result.ProviderMatch = $result.ActualProvider -eq $ExpectedProvider
        $result.FullMatch     = $result.VersionMatch -and $result.ProviderMatch

        return $result
    }
}

# ---------------------------------------------------------------------------
# Step 1: Parse the INF file from the host driver package
# ---------------------------------------------------------------------------
$infFiles = Get-ChildItem -Path $DriverPath -Filter '*.inf'
if (-not $infFiles) {
    Write-Error "No .inf files found in '$DriverPath'."
    exit $EXIT_NO_INF
}

$infFile = $infFiles[0]
Write-Host "Parsing INF: $($infFile.FullName)"
$expected = Get-InfDriverMetadata -InfPath $infFile.FullName

if (-not $expected.Version) {
    Write-Error "Could not parse DriverVer from '$($infFile.Name)'."
    exit $EXIT_NO_INF
}

Write-Host ""
Write-Host "Expected driver metadata:"
Write-Host "  INF:          $($expected.InfFile)"
Write-Host "  Version:      $($expected.Version)"
Write-Host "  Date:         $($expected.Date)"
Write-Host "  Provider:     $($expected.Provider)"
Write-Host "  Class:        $($expected.Class)"
Write-Host "  Hardware IDs: $($expected.HardwareIds -join ', ')"
Write-Host "  Device Desc:  $($expected.DeviceDesc)"
Write-Host ""

# ---------------------------------------------------------------------------
# Step 2: Ensure VM is running
# ---------------------------------------------------------------------------
try {
    $vm = Get-VM -Name $VMName
} catch {
    Write-Error "VM '$VMName' not found: $_"
    exit $EXIT_VM_NOT_FOUND
}
if ($vm.State -ne 'Running') {
    Write-Host "Starting VM '$VMName'..."
    Start-VM -Name $VMName
    Wait-VM -Name $VMName -For Heartbeat -Timeout 300
    Write-Host "VM is ready."
}

# ---------------------------------------------------------------------------
# Step 3: Connect via PowerShell Direct
# ---------------------------------------------------------------------------
Write-Host "Connecting to VM '$VMName'..."
$session = New-GuestSession

# Build the result object for structured output (#9)
$verifyResult = [PSCustomObject]@{
    VMName           = $VMName
    DriverPath       = $DriverPath
    ExpectedVersion  = $expected.Version
    ExpectedProvider = $expected.Provider
    Result           = 'UNKNOWN'
    ActualVersion    = $null
    ActualProvider   = $null
    InstanceId       = $null
    QueryMethod      = $null
    Actions          = [System.Collections.ArrayList]@()
}

try {
    # -------------------------------------------------------------------
    # Step 4: Ensure devgen.exe is available on the VM
    # -------------------------------------------------------------------
    Write-Host "Checking for devgen.exe on VM..."

    $devgenAvailable = Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ScriptBlock {
        $devgen = Get-Command devgen.exe -ErrorAction SilentlyContinue
        if ($devgen) { return $devgen.Source }
        if (Test-Path 'C:\DriverTools\devgen.exe') { return 'C:\DriverTools\devgen.exe' }
        $wdkPaths = Get-ChildItem 'C:\Program Files\Windows Kits\10\Tools' -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending
        foreach ($ver in $wdkPaths) {
            $candidate = Join-Path $ver.FullName 'x64\devgen.exe'
            if (Test-Path $candidate) { return $candidate }
        }
        $wdkPaths86 = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\Tools' -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending
        foreach ($ver in $wdkPaths86) {
            $candidate = Join-Path $ver.FullName 'x64\devgen.exe'
            if (Test-Path $candidate) { return $candidate }
        }
        return $null
    }

    if (-not $devgenAvailable) {
        Write-Host "devgen.exe not found on VM. Searching host for devgen.exe to copy..."

        $hostDevgen = $null

        # Check DEVGEN_PATH environment variable first
        if ($env:DEVGEN_PATH -and (Test-Path $env:DEVGEN_PATH)) {
            $hostDevgen = $env:DEVGEN_PATH
            Write-Host "Found devgen.exe via DEVGEN_PATH env var: $hostDevgen"
        }

        # Query registry for WDK installation root
        if (-not $hostDevgen) {
            $wdkRoot = $null
            $regPaths = @(
                'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots',
                'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots'
            )
            foreach ($regPath in $regPaths) {
                if (Test-Path $regPath) {
                    $root = Get-ItemProperty -Path $regPath -Name 'KitsRoot10' -ErrorAction SilentlyContinue
                    if ($root) { $wdkRoot = $root.KitsRoot10; break }
                    $root = Get-ItemProperty -Path $regPath -Name 'KitsRoot' -ErrorAction SilentlyContinue
                    if ($root) { $wdkRoot = $root.KitsRoot; break }
                }
            }

            if ($wdkRoot) {
                $toolsDir = Join-Path $wdkRoot 'Tools'
                if (Test-Path $toolsDir) {
                    $versions = Get-ChildItem $toolsDir -Directory | Sort-Object Name -Descending
                    foreach ($ver in $versions) {
                        $candidate = Join-Path $ver.FullName 'x64\devgen.exe'
                        if (Test-Path $candidate) {
                            $hostDevgen = $candidate
                            Write-Host "Found devgen.exe via registry WDK root: $hostDevgen"
                            break
                        }
                    }
                }
            }
        }

        # Fallback: hardcoded well-known WDK paths
        if (-not $hostDevgen) {
            $wdkToolsRoots = @(
                'C:\Program Files\Windows Kits\10\Tools',
                'C:\Program Files (x86)\Windows Kits\10\Tools'
            )
            foreach ($root in $wdkToolsRoots) {
                if (Test-Path $root) {
                    $versions = Get-ChildItem $root -Directory | Sort-Object Name -Descending
                    foreach ($ver in $versions) {
                        $candidate = Join-Path $ver.FullName 'x64\devgen.exe'
                        if (Test-Path $candidate) {
                            $hostDevgen = $candidate
                            break
                        }
                    }
                }
                if ($hostDevgen) { break }
            }
        }

        if (-not $hostDevgen) {
            Write-Error "devgen.exe not found on host or VM. Install the Windows Driver Kit, or set DEVGEN_PATH to the full path of devgen.exe."
            exit $EXIT_DEVGEN_NOT_FOUND
        }

        Write-Host "Found devgen.exe at: $hostDevgen"
        Write-Host "Copying devgen.exe to VM..."

        Invoke-GuestCommand -Session $session -TimeoutSeconds 30 -ScriptBlock {
            New-Item -Path 'C:\DriverTools' -ItemType Directory -Force | Out-Null
        }
        Copy-Item -ToSession $session -Path $hostDevgen -Destination 'C:\DriverTools\devgen.exe' -Force
        $devgenAvailable = 'C:\DriverTools\devgen.exe'
        Write-Host "devgen.exe copied to VM at C:\DriverTools\devgen.exe"
    } else {
        Write-Host "devgen.exe found on VM at: $devgenAvailable"
    }

    # -------------------------------------------------------------------
    # Step 5: Check if the device exists and query its current driver
    # -------------------------------------------------------------------
    Write-Host ""
    Write-Host "Querying driver state on VM..."

    $hwIds = $expected.HardwareIds
    $deviceDesc = $expected.DeviceDesc

    $vmState = Find-DeviceOnVM -Session $session -HardwareIds $hwIds `
        -DeviceDesc $deviceDesc -ExpectedVersion $expected.Version `
        -ExpectedProvider $expected.Provider

    $verifyResult.QueryMethod = $vmState.QueryMethod

    # -------------------------------------------------------------------
    # Step 6: Report findings and determine action
    # -------------------------------------------------------------------
    Write-Host ""
    $needsInstall = $false
    $needsDeviceCreation = $false

    if (-not $vmState.DeviceFound) {
        Write-Host "Device not found on VM."
        $needsInstall = $true
        $needsDeviceCreation = $true
        $verifyResult.Actions.Add('device-not-found') | Out-Null
    } else {
        Write-Host "Device found on VM:"
        Write-Host "  Instance ID:    $($vmState.InstanceId)"
        Write-Host "  Friendly Name:  $($vmState.FriendlyName)"
        Write-Host "  Actual Version: $($vmState.ActualVersion)"
        Write-Host "  Actual Provider:$($vmState.ActualProvider)"
        Write-Host "  Actual INF:     $($vmState.ActualInfPath)"
        Write-Host "  Query Method:   $($vmState.QueryMethod)"
        Write-Host ""

        if ($vmState.FullMatch) {
            Write-Host "`u{2705} Driver verification PASSED. Correct driver is installed."
            Write-Host "  Version:  $($vmState.ActualVersion) (matches expected $($expected.Version))"
            Write-Host "  Provider: $($vmState.ActualProvider) (matches expected $($expected.Provider))"

            $verifyResult.Result         = 'PASS'
            $verifyResult.ActualVersion  = $vmState.ActualVersion
            $verifyResult.ActualProvider = $vmState.ActualProvider
            $verifyResult.InstanceId     = $vmState.InstanceId
            Write-Host ""
            Write-Host ($verifyResult | ConvertTo-Json -Compress)
            exit $EXIT_SUCCESS
        }

        Write-Host "`u{26A0}`u{FE0F}  Driver verification FAILED:"
        if (-not $vmState.VersionMatch) {
            Write-Host "  Version mismatch:  expected '$($expected.Version)' but found '$($vmState.ActualVersion)'"
        }
        if (-not $vmState.ProviderMatch) {
            Write-Host "  Provider mismatch: expected '$($expected.Provider)' but found '$($vmState.ActualProvider)'"
        }
        $needsInstall = $true
        $verifyResult.Actions.Add('version-mismatch') | Out-Null
    }

    # -------------------------------------------------------------------
    # Step 7: Course-correct — remove wrong driver if present
    # -------------------------------------------------------------------
    Write-Host ""
    Write-Host "Beginning course correction..."

    if ($vmState.DeviceFound -and $vmState.ActualInfPath) {
        Write-Host "Removing incorrect driver ($($vmState.ActualInfPath)) from VM..."
        $infToRemove = $vmState.ActualInfPath
        Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ScriptBlock {
            $inf = $using:infToRemove
            pnputil.exe /delete-driver $inf /uninstall /force 2>&1 | Out-Null
        }
        Write-Host "Old driver removed."
        $verifyResult.Actions.Add('removed-old-driver') | Out-Null
    }

    # -------------------------------------------------------------------
    # Step 8: Install correct driver via Install-DriverOnVM.ps1 (#8)
    # Keep session open — pass credential via splatting
    # -------------------------------------------------------------------
    $installScript = Join-Path $ScriptDir 'Install-DriverOnVM.ps1'
    if (-not (Test-Path $installScript)) {
        Write-Error "Install-DriverOnVM.ps1 not found at '$installScript'. Place this script alongside Install-DriverOnVM.ps1."
        exit $EXIT_INSTALL_FAIL
    }

    Write-Host "Running Install-DriverOnVM.ps1 (in subprocess)..."
    # Run Install in a subprocess so its exit statements don't kill this session.
    # The Install script has hardcoded credentials, so just use -File directly.
    $installOutput = powershell.exe -ExecutionPolicy Bypass -File $installScript -VMName $VMName -DriverPath $DriverPath 2>&1
    $installExitCode = $LASTEXITCODE
    Write-Host $installOutput
    if ($installExitCode -ne 0) {
        Write-Warning "Install script exited with code $installExitCode"
    }
    $verifyResult.Actions.Add('reinstalled-driver') | Out-Null

    # Re-establish session (Install-DriverOnVM.ps1 may have rebooted the VM)
    if ($session) {
        Remove-PSSession $session -ErrorAction SilentlyContinue
        $session = $null
    }
    $session = New-GuestSession

    # -------------------------------------------------------------------
    # Step 9: Create device node with devgen if needed
    # -------------------------------------------------------------------
    if ($needsDeviceCreation) {
        $primaryHwId = $hwIds[0]
        Write-Host ""
        Write-Host "Creating device node for hardware ID '$primaryHwId' using devgen..."

        $devgenPath = $devgenAvailable
        $devgenResult = Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ScriptBlock {
            $DevgenPath = $using:devgenPath
            $HwId = $using:primaryHwId

            # Check if device already exists
            $existingCheck = pnputil.exe /enum-devices /ids 2>&1 | Out-String
            if ($existingCheck -match [regex]::Escape($HwId)) {
                return [PSCustomObject]@{
                    AlreadyExists = $true
                    Success       = $true
                    Output        = "Device already exists with hardware ID $HwId"
                }
            }

            $output = & $DevgenPath /add /hardwareid $HwId 2>&1 | Out-String
            [PSCustomObject]@{
                AlreadyExists = $false
                Success       = $LASTEXITCODE -eq 0
                Output        = $output
            }
        }

        if ($devgenResult.AlreadyExists) {
            Write-Host "Device already exists (likely created by PnP during driver install)."
        } elseif ($devgenResult.Success) {
            Write-Host "Device node created successfully."
            Write-Host $devgenResult.Output
            $verifyResult.Actions.Add('created-device-node') | Out-Null
        } else {
            Write-Warning "devgen failed to create device node:"
            Write-Warning $devgenResult.Output
        }

        Start-Sleep -Seconds 3
    }

    # -------------------------------------------------------------------
    # Step 10: Final verification — ensure correct driver is on device
    # -------------------------------------------------------------------
    Write-Host ""
    Write-Host "Performing final driver verification..."

    $postInstall = Find-DeviceOnVM -Session $session -HardwareIds $hwIds `
        -DeviceDesc $deviceDesc -ExpectedVersion $expected.Version `
        -ExpectedProvider $expected.Provider

    Write-Host ""
    if (-not $postInstall.DeviceFound) {
        Write-Warning "Device still not found after devgen + driver install."
        Write-Warning "Attempting reboot to allow PnP enumeration..."
        $verifyResult.Actions.Add('rebooted-vm') | Out-Null

        Remove-PSSession $session -ErrorAction SilentlyContinue
        $session = $null

        Restart-VM -Name $VMName -Force
        Wait-VM -Name $VMName -For Heartbeat -Timeout 300
        $session = New-GuestSession

        Start-Sleep -Seconds 5

        $postInstall = Find-DeviceOnVM -Session $session -HardwareIds $hwIds `
            -DeviceDesc $deviceDesc -ExpectedVersion $expected.Version `
            -ExpectedProvider $expected.Provider
    }

    if ($postInstall.FullMatch) {
        Write-Host "`u{2705} Verification PASSED. Correct driver is installed and bound to device."
        Write-Host "  Instance ID: $($postInstall.InstanceId)"
        Write-Host "  Version:     $($postInstall.ActualVersion)"
        Write-Host "  Provider:    $($postInstall.ActualProvider)"

        $verifyResult.Result         = 'PASS'
        $verifyResult.ActualVersion  = $postInstall.ActualVersion
        $verifyResult.ActualProvider = $postInstall.ActualProvider
        $verifyResult.InstanceId     = $postInstall.InstanceId
        Write-Host ""
        Write-Host ($verifyResult | ConvertTo-Json -Compress)
        exit $EXIT_SUCCESS

    } elseif ($postInstall.DeviceFound -and -not $postInstall.FullMatch) {
        Write-Host "`u{26A0}`u{FE0F}  Device found but wrong driver attached. Attempting force rebind..."
        Write-Host "  Current:  $($postInstall.ActualProvider) v$($postInstall.ActualVersion)"
        Write-Host "  Expected: $($expected.Provider) v$($expected.Version)"
        $verifyResult.Actions.Add('force-rebind') | Out-Null

        if ($postInstall.ActualInfPath) {
            $infToRemove2 = $postInstall.ActualInfPath
            Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ScriptBlock {
                $inf = $using:infToRemove2
                pnputil.exe /delete-driver $inf /uninstall /force 2>&1 | Out-Null
            }
        }

        $instId = $postInstall.InstanceId
        Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ScriptBlock {
            $id = $using:instId
            Disable-PnpDevice -InstanceId $id -Confirm:$false -ErrorAction SilentlyContinue
            Start-Sleep -Seconds 1
            Enable-PnpDevice -InstanceId $id -Confirm:$false -ErrorAction SilentlyContinue
        }

        Start-Sleep -Seconds 3

        $finalState = Find-DeviceOnVM -Session $session -HardwareIds $hwIds `
            -DeviceDesc $deviceDesc -ExpectedVersion $expected.Version `
            -ExpectedProvider $expected.Provider

        if ($finalState.FullMatch) {
            Write-Host "`u{2705} Force rebind SUCCESSFUL."
            Write-Host "  Instance ID: $($finalState.InstanceId)"
            Write-Host "  Version:     $($finalState.ActualVersion)"
            Write-Host "  Provider:    $($finalState.ActualProvider)"

            $verifyResult.Result         = 'PASS'
            $verifyResult.ActualVersion  = $finalState.ActualVersion
            $verifyResult.ActualProvider = $finalState.ActualProvider
            $verifyResult.InstanceId     = $finalState.InstanceId
            Write-Host ""
            Write-Host ($verifyResult | ConvertTo-Json -Compress)
            exit $EXIT_SUCCESS
        } else {
            Write-Warning "Could not bind correct driver after force rebind."
            if ($finalState.DeviceFound) {
                Write-Warning "  Current version:  $($finalState.ActualVersion)"
                Write-Warning "  Current provider: $($finalState.ActualProvider)"
            }
            Write-Warning "Manual intervention may be required."

            $verifyResult.Result         = 'FAIL'
            $verifyResult.ActualVersion  = $finalState.ActualVersion
            $verifyResult.ActualProvider = $finalState.ActualProvider
            $verifyResult.InstanceId     = $finalState.InstanceId
            Write-Host ""
            Write-Host ($verifyResult | ConvertTo-Json -Compress)
            exit $EXIT_VERIFY_FAIL
        }
    } else {
        Write-Warning "Device not found after installation and reboot."
        Write-Warning "Verify that hardware ID '$($hwIds[0])' is correct and devgen.exe succeeded."

        $verifyResult.Result = 'FAIL'
        Write-Host ""
        Write-Host ($verifyResult | ConvertTo-Json -Compress)
        exit $EXIT_VERIFY_FAIL
    }

} catch {
    if ($_.Exception.Message -match 'timed out') {
        Write-Error "Operation timed out: $_"
        $verifyResult.Result = 'TIMEOUT'
        Write-Host ($verifyResult | ConvertTo-Json -Compress)
        exit $EXIT_TIMEOUT
    }
    throw
} finally {
    if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
}
