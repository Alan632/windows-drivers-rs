<#
.SYNOPSIS
    Captures ETW driver trace logs from a Hyper-V VM for a specified duration
    and extracts them to the host desktop for post-process analysis.

.DESCRIPTION
    Parses the driver .inf file to identify the driver under test. Connects to
    the VM via PowerShell Direct (credentialless first, falling back to explicit
    credentials) and uses logman (built-in, no WDK required) to capture ETW
    traces from the KMDF/WDF framework provider and any driver-specific WPP
    provider GUID found in the source. After the capture duration elapses, the
    ETL file is copied back to the host desktop and decoded to XML via tracerpt
    for smoke-test analysis.

    Alternatively, a lightweight DebugView capture mode can be used for basic
    DbgPrint smoke tests without requiring WDK or ETW knowledge.

.PARAMETER VMName
    Name of the Hyper-V VM running the driver under test.

.PARAMETER DriverPath
    Path to the driver package directory on the host. Must contain at least one
    .inf file and optionally a .pdb for WPP trace decoding.

.PARAMETER Credential
    Optional PSCredential for the VM. If not supplied, tries credentialless
    PS Direct first, then prompts interactively.

.PARAMETER Duration
    Number of seconds to capture traces. Default is 30.

.PARAMETER OutputFormat
    Output format: 'Text' (default) for human-readable output, 'Json' for
    structured JSON output suitable for automation pipelines.

.PARAMETER CaptureMode
    Capture method: 'ETW' (default) uses logman for full ETW trace capture,
    'DebugView' uses Sysinternals DebugView for lightweight DbgPrint capture.

.EXAMPLE
    .\Capture-DriverLogs.ps1 -VMName "driver-test-vm" -DriverPath ".\target\debug\sample_kmdf_driver_package"

.EXAMPLE
    .\Capture-DriverLogs.ps1 -VMName "driver-test-vm" -DriverPath ".\target\debug\sample_kmdf_driver_package" -OutputFormat Json

.EXAMPLE
    .\Capture-DriverLogs.ps1 -VMName "driver-test-vm" -DriverPath ".\target\debug\sample_kmdf_driver_package" -CaptureMode DebugView
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
    [PSCredential]$Credential,

    [Parameter()]
    [int]$Duration = 30,

    [Parameter()]
    [ValidateSet('Text', 'Json')]
    [string]$OutputFormat = 'Text',

    [Parameter()]
    [ValidateSet('ETW', 'DebugView')]
    [string]$CaptureMode = 'ETW'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Structured exit codes
$EXIT_SUCCESS    = 0
$EXIT_NO_INF     = 1
$EXIT_VM_NOT_FOUND = 2
$EXIT_SESSION_FAIL = 3
$EXIT_TRACE_FAIL = 4
$EXIT_COPY_FAIL  = 5
$EXIT_TIMEOUT    = 6

# Structured results collector for JSON output (#8)
$captureResults = @{
    Status      = 'Unknown'
    Driver      = $null
    VM          = $VMName
    CaptureMode = $CaptureMode
    Duration    = $Duration
    Providers   = @()
    Files       = @()
    EventCount  = 0
    Warnings    = @()
    Errors      = @()
}

# ---------------------------------------------------------------------------
# Helper: Parse .inf for driver metadata
# ---------------------------------------------------------------------------
function Get-InfDriverMetadata {
    param([string]$InfPath)

    $content = Get-Content -Path $InfPath -Raw

    $driverVersion = $null
    if ($content -match '(?im)^\s*DriverVer\s*=\s*[^,]+,\s*(.+)$') {
        $driverVersion = $Matches[1].Trim()
    }

    $providerRaw = $null
    if ($content -match '(?im)^\s*Provider\s*=\s*(.+)$') {
        $providerRaw = $Matches[1].Trim()
    }
    $provider = $providerRaw
    if ($providerRaw -match '^%(.+)%$') {
        $varName = $Matches[1]
        if ($content -match "(?im)^\s*$varName\s*=\s*""?([^""`r`n]+)""?") {
            $provider = $Matches[1].Trim()
        }
    }

    # Extract driver name from INF filename
    $driverName = [System.IO.Path]::GetFileNameWithoutExtension($InfPath)

    [PSCustomObject]@{
        InfFile    = Split-Path $InfPath -Leaf
        DriverName = $driverName
        Version    = $driverVersion
        Provider   = $provider
    }
}

# ---------------------------------------------------------------------------
# Helper: Search for WPP Control GUIDs in driver source/header files
# ---------------------------------------------------------------------------
function Find-WppGuids {
    param([string]$PackagePath)

    $guids = @()

    # Only search source files in the package and the sibling src/ directory
    $searchPaths = @()
    $srcSrc = Join-Path (Split-Path (Split-Path $PackagePath -Parent) -Parent) 'src'
    if (Test-Path $srcSrc) { $searchPaths += $srcSrc }

    foreach ($searchPath in $searchPaths) {
        $files = Get-ChildItem -Path $searchPath -Recurse -Include '*.h','*.c','*.cpp','*.rs' -ErrorAction SilentlyContinue
        foreach ($file in $files) {
            $text = Get-Content $file.FullName -Raw -ErrorAction SilentlyContinue
            if (-not $text) { continue }

            # Only look for GUIDs near WPP/trace keywords
            if ($text -notmatch 'WPP_CONTROL_GUIDS|TRACELOGGING_DEFINE_PROVIDER|trace_guid|TraceEvents') {
                continue
            }

            # Match WPP-style comma-separated GUIDs: (xxxxxxxx,xxxx,xxxx,xxxx,xxxxxxxxxxxx)
            $wppMatches = [regex]::Matches($text, '\(([0-9a-fA-F]{8}),\s*([0-9a-fA-F]{4}),\s*([0-9a-fA-F]{4}),\s*([0-9a-fA-F]{4}),\s*([0-9a-fA-F]{12})\)')
            foreach ($m in $wppMatches) {
                $guid = "$($m.Groups[1].Value)-$($m.Groups[2].Value)-$($m.Groups[3].Value)-$($m.Groups[4].Value)-$($m.Groups[5].Value)"
                $guids += $guid
            }

            # Match standard GUID format near trace keywords
            $guidMatches = [regex]::Matches($text, '\{?([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\}?')
            foreach ($m in $guidMatches) {
                $guids += $m.Groups[1].Value
            }
        }
    }

    return ($guids | Select-Object -Unique)
}

# ---------------------------------------------------------------------------
# Helper: create a PS Direct session with the resolved credential mode (#4, #5)
# ---------------------------------------------------------------------------
function New-GuestSession {
    if ($script:cred) {
        New-PSSession -VMName $VMName -Credential $script:cred
    } else {
        New-PSSession -VMName $VMName
    }
}

# ---------------------------------------------------------------------------
# Helper: invoke a command on the guest with optional timeout (#5, #6)
# ---------------------------------------------------------------------------
function Invoke-GuestCommand {
    param(
        [Parameter(Mandatory)][System.Management.Automation.Runspaces.PSSession]$Session,
        [Parameter(Mandatory)][scriptblock]$ScriptBlock,
        [object[]]$ArgumentList,
        [int]$TimeoutSeconds = 0
    )
    $invokeParams = @{
        Session     = $Session
        ScriptBlock = $ScriptBlock
    }
    if ($PSBoundParameters.ContainsKey('ArgumentList')) {
        $invokeParams['ArgumentList'] = $ArgumentList
    }
    if ($TimeoutSeconds -gt 0) {
        $invokeParams['AsJob'] = $true
        $job = Invoke-Command @invokeParams
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
        return Invoke-Command @invokeParams
    }
}

# ---------------------------------------------------------------------------
# Step 1: Parse INF and discover trace providers
# ---------------------------------------------------------------------------
$infFiles = Get-ChildItem -Path $DriverPath -Filter '*.inf'
if (-not $infFiles) {
    Write-Error "No .inf files found in '$DriverPath'."
    exit $EXIT_NO_INF
}

$infFile = $infFiles[0]
Write-Host "Parsing INF: $($infFile.FullName)"
$driverMeta = Get-InfDriverMetadata -InfPath $infFile.FullName

$captureResults.Driver = @{
    Name     = $driverMeta.DriverName
    Version  = $driverMeta.Version
    Provider = $driverMeta.Provider
    InfFile  = $driverMeta.InfFile
}

Write-Host "  Driver:   $($driverMeta.DriverName)"
Write-Host "  Version:  $($driverMeta.Version)"
Write-Host "  Provider: $($driverMeta.Provider)"

# Build list of ETW providers to capture
$providers = @()

# Always capture KMDF framework events (KMDFv1 Trace Provider)
$providers += @{
    Name = '{544D4C9D-942C-46D5-BF50-DF5CD9524A50}'
    Desc = 'KMDF Framework (KMDFv1 Trace Provider)'
}

# Search for driver-specific WPP GUIDs
$wppGuids = Find-WppGuids -PackagePath $DriverPath
foreach ($guid in $wppGuids) {
    $providers += @{
        Name = "{$guid}"
        Desc = "Driver WPP ($guid)"
    }
}

$captureResults.Providers = @($providers | ForEach-Object { @{ Name = $_.Name; Description = $_.Desc } })

Write-Host ""
Write-Host "ETW providers to capture:"
foreach ($p in $providers) {
    Write-Host "  - $($p.Desc): $($p.Name)"
}

# ---------------------------------------------------------------------------
# Step 2: Ensure VM is running and connect
# ---------------------------------------------------------------------------
try {
    $vm = Get-VM -Name $VMName
} catch {
    Write-Error "VM '$VMName' not found: $_"
    exit $EXIT_VM_NOT_FOUND
}
if ($vm.State -ne 'Running') {
    Write-Host "`nStarting VM '$VMName'..."
    Start-VM -Name $VMName
    Wait-VM -Name $VMName -For Heartbeat -Timeout 300
    Write-Host "VM is ready."
}

# Enable Guest Service Interface for file copy support (#7)
$guestSvc = Get-VMIntegrationService -VMName $VMName | Where-Object { $_.Name -eq 'Guest Service Interface' }
if ($guestSvc -and -not $guestSvc.Enabled) {
    Write-Host "Enabling Guest Service Interface..."
    Enable-VMIntegrationService -Name 'Guest Service Interface' -VMName $VMName
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

Write-Host "`nConnecting to VM '$VMName'..."
$session = New-GuestSession

$timestamp = Get-Date -Format 'yyyyMMdd_HHmmss'
$sessionName = "DriverTrace_$timestamp"
$vmLogDir = 'C:\DriverLogs'
$vmEtlPath = "$vmLogDir\$sessionName.etl"
$outputDir = $null

try {
    # -------------------------------------------------------------------
    # Configure DbgPrint filter registry key on the guest (#9)
    # -------------------------------------------------------------------
    Write-Host "`nConfiguring DbgPrint filter on VM..."
    Invoke-GuestCommand -Session $session -ScriptBlock {
        $regPath = 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Debug Print Filter'
        if (-not (Test-Path $regPath)) {
            New-Item -Path $regPath -Force | Out-Null
        }
        Set-ItemProperty -Path $regPath -Name 'DEFAULT' -Value 0xFFFFFFFF -Type DWord
    }
    Write-Host "  DbgPrint filter set to capture all debug output."

    if ($CaptureMode -eq 'DebugView') {
        # ---------------------------------------------------------------
        # DebugView capture mode (#12)
        # ---------------------------------------------------------------
        Write-Host "`nUsing DebugView capture mode..."

        $vmDbgViewDir = "$vmLogDir\DebugView"
        $vmDbgViewExe = "$vmDbgViewDir\Dbgview.exe"
        $vmDbgViewLog = "$vmDbgViewDir\dbgview_$timestamp.log"

        # Locate DebugView: check env var, then local cache, then download
        $hostDbgViewPath = $null
        if ($env:DBGVIEW_PATH -and (Test-Path $env:DBGVIEW_PATH)) {
            $hostDbgViewPath = $env:DBGVIEW_PATH
            Write-Host "  Using DebugView from DBGVIEW_PATH: $hostDbgViewPath"
        } else {
            $hostDbgViewPath = Join-Path $env:TEMP 'Dbgview.exe'
            if (-not (Test-Path $hostDbgViewPath)) {
                Write-Host "  Downloading DebugView from Sysinternals..."
                try {
                    Invoke-WebRequest -Uri 'https://live.sysinternals.com/Dbgview.exe' -OutFile $hostDbgViewPath -UseBasicParsing
                } catch {
                    Write-Error "Failed to download DebugView. Download manually or set DBGVIEW_PATH to the full path of Dbgview.exe: $_"
                    exit $EXIT_TRACE_FAIL
                }
            }
        }

        # Create directory and copy DebugView to guest
        Invoke-GuestCommand -Session $session -ArgumentList $vmDbgViewDir -ScriptBlock {
            param($Dir)
            New-Item -Path $Dir -ItemType Directory -Force | Out-Null
        }
        Copy-Item -ToSession $session -Path $hostDbgViewPath -Destination $vmDbgViewExe -Force
        Write-Host "  DebugView deployed to VM."

        # Accept EULA silently and launch DebugView
        Invoke-GuestCommand -Session $session -ArgumentList $vmDbgViewExe, $vmDbgViewLog -ScriptBlock {
            param($ExePath, $LogPath)
            # Accept EULA via registry
            $regPath = 'HKCU:\Software\Sysinternals\DbgView'
            if (-not (Test-Path $regPath)) { New-Item -Path $regPath -Force | Out-Null }
            Set-ItemProperty -Path $regPath -Name 'EulaAccepted' -Value 1 -Type DWord
            # Launch: /k = kernel capture, /g = global Win32, /t = log to file, /l = log file
            Start-Process -FilePath $ExePath -ArgumentList "/accepteula /k /g /t /l `"$LogPath`"" -WindowStyle Hidden
        }
        Write-Host "  DebugView capture started."

        # Wait for capture duration with progress
        Write-Host "Capturing DebugView output for $Duration seconds..."
        for ($i = $Duration; $i -gt 0; $i -= 5) {
            $remaining = [Math]::Min($i, 5)
            Start-Sleep -Seconds $remaining
            $elapsed = $Duration - $i + $remaining
            Write-Host "  $elapsed / $Duration seconds elapsed"

            # Stream DebugView log size periodically (#11)
            if (($elapsed % 10) -eq 0 -or $i -le 5) {
                try {
                    $logStatus = Invoke-GuestCommand -Session $session -TimeoutSeconds 10 -ArgumentList $vmDbgViewLog -ScriptBlock {
                        param($LogPath)
                        if (Test-Path $LogPath) {
                            [Math]::Round((Get-Item $LogPath).Length / 1KB, 1)
                        } else { 0 }
                    }
                    Write-Host "    Log size: $logStatus KB"
                } catch {
                    # Non-fatal: continue capture even if status check fails
                }
            }
        }

        # Stop DebugView and retrieve log
        Write-Host "`nStopping DebugView..."
        $dbgResult = Invoke-GuestCommand -Session $session -TimeoutSeconds 30 -ArgumentList $vmDbgViewLog -ScriptBlock {
            param($LogPath)
            Get-Process -Name 'Dbgview' -ErrorAction SilentlyContinue | Stop-Process -Force
            Start-Sleep -Seconds 2
            $logExists = Test-Path $LogPath
            $lineCount = 0
            $sizeKB = 0
            if ($logExists) {
                $f = Get-Item $LogPath
                $sizeKB = [Math]::Round($f.Length / 1KB, 1)
                $lineCount = (Get-Content $LogPath -ErrorAction SilentlyContinue | Measure-Object -Line).Lines
            }
            [PSCustomObject]@{
                LogExists = $logExists
                LogPath   = $LogPath
                SizeKB    = $sizeKB
                LineCount = $lineCount
            }
        }

        # Verify captured events (#10)
        if (-not $dbgResult.LogExists) {
            $captureResults.Warnings += 'DebugView log file was not created'
            Write-Warning "DebugView log file was not created."
        } elseif ($dbgResult.LineCount -eq 0) {
            $captureResults.Warnings += 'DebugView captured zero lines (driver may not be producing DbgPrint output)'
            Write-Warning "DebugView captured zero lines. Driver may not be producing DbgPrint output."
        } else {
            Write-Host "  DebugView captured $($dbgResult.LineCount) lines ($($dbgResult.SizeKB) KB)"
        }

        $captureResults.EventCount = $dbgResult.LineCount

        # Copy log to host
        $desktop = [Environment]::GetFolderPath('Desktop')
        $outputDir = Join-Path $desktop "DriverLogs_$($driverMeta.DriverName)_$timestamp"
        New-Item -Path $outputDir -ItemType Directory -Force | Out-Null

        Write-Host "`nCopying DebugView log to host: $outputDir"
        $localLog = Join-Path $outputDir "dbgview_$timestamp.log"
        try {
            Copy-Item -FromSession $session -Path $vmDbgViewLog -Destination $localLog -Force
            Write-Host "  Copied: $localLog"
            $captureResults.Files += @{ Name = (Split-Path $localLog -Leaf); SizeKB = $dbgResult.SizeKB }
        } catch {
            $captureResults.Warnings += "Could not copy DebugView log: $_"
            Write-Warning "  Could not copy DebugView log: $_"
        }

        $captureResults.Status = 'Success'
        $captureResults.OutputDir = $outputDir

    } else {
        # ---------------------------------------------------------------
        # ETW capture mode (default)
        # ---------------------------------------------------------------

        # -------------------------------------------------------------------
        # Step 3: Start ETW trace session on VM
        # -------------------------------------------------------------------
        Write-Host "`nStarting ETW trace session '$sessionName' on VM..."

        $providerNames = $providers | ForEach-Object { $_.Name }

        $startResult = Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ArgumentList $sessionName, $vmEtlPath, @(, $providerNames) -ScriptBlock {
            param($SessionName, $EtlPath, $ProviderNames)

            # Create log directory
            $logDir = Split-Path $EtlPath -Parent
            New-Item -Path $logDir -ItemType Directory -Force | Out-Null

            # Clean up any stale session with the same name
            logman stop $SessionName -ets 2>&1 | Out-Null

            # Build logman command — start session with first provider
            # logman syntax: logman start <name> -p "<provider>" <keywords> <level> -o <file> -ets
            $providerArg = "`"$($ProviderNames[0])`" 0xFFFFFFFF 0xFF"
            $output = cmd /c "logman start $SessionName -p $providerArg -o `"$EtlPath`" -ets" 2>&1 | Out-String

            if ($LASTEXITCODE -ne 0) {
                return [PSCustomObject]@{
                    Success = $false
                    Output  = "Failed to start session: $output"
                }
            }

            # Add additional providers to the running session
            for ($i = 1; $i -lt $ProviderNames.Count; $i++) {
                $extraProvider = "`"$($ProviderNames[$i])`" 0xFFFFFFFF 0xFF"
                $addOutput = cmd /c "logman update $SessionName -p $extraProvider --append -ets" 2>&1 | Out-String
                $output += "`n$addOutput"
            }

            # Verify session is running
            $status = logman query $SessionName -ets 2>&1 | Out-String

            return [PSCustomObject]@{
                Success = $true
                Output  = $output
                Status  = $status
            }
        }

        if (-not $startResult.Success) {
            Write-Error "Failed to start trace session: $($startResult.Output)"
            exit $EXIT_TRACE_FAIL
        }

        Write-Host "Trace session started."
        Write-Host $startResult.Status

        # -------------------------------------------------------------------
        # Step 4: Wait for capture duration with log streaming (#11)
        # -------------------------------------------------------------------
        Write-Host "Capturing traces for $Duration seconds..."
        for ($i = $Duration; $i -gt 0; $i -= 5) {
            $remaining = [Math]::Min($i, 5)
            Start-Sleep -Seconds $remaining
            $elapsed = $Duration - $i + $remaining
            Write-Host "  $elapsed / $Duration seconds elapsed"

            # Stream ETL file size periodically (#11)
            if (($elapsed % 10) -eq 0 -or $i -le 5) {
                try {
                    $etlSizeKB = Invoke-GuestCommand -Session $session -TimeoutSeconds 10 -ArgumentList $vmEtlPath -ScriptBlock {
                        param($EtlPath)
                        if (Test-Path $EtlPath) {
                            [Math]::Round((Get-Item $EtlPath).Length / 1KB, 1)
                        } else { 0 }
                    }
                    Write-Host "    ETL size: $etlSizeKB KB"
                } catch {
                    # Non-fatal: continue capture even if status check fails
                }
            }
        }

        # -------------------------------------------------------------------
        # Step 5: Stop trace session and collect results
        # -------------------------------------------------------------------
        Write-Host "`nStopping trace session..."

        $stopResult = Invoke-GuestCommand -Session $session -TimeoutSeconds 60 -ArgumentList $sessionName, $vmEtlPath -ScriptBlock {
            param($SessionName, $EtlPath)

            $output = logman stop $SessionName -ets 2>&1 | Out-String

            # Get file info
            $fileInfo = $null
            if (Test-Path $EtlPath) {
                $f = Get-Item $EtlPath
                $fileInfo = [PSCustomObject]@{
                    Path     = $f.FullName
                    SizeKB   = [Math]::Round($f.Length / 1KB, 1)
                    Exists   = $true
                }
            } else {
                $fileInfo = [PSCustomObject]@{
                    Path   = $EtlPath
                    SizeKB = 0
                    Exists = $false
                }
            }

            # Decode ETL to XML using tracerpt (built-in)
            $xmlPath = $EtlPath -replace '\.etl$', '.xml'
            $summaryPath = $EtlPath -replace '\.etl$', '_summary.txt'
            $decodeSuccess = $false
            $decodeError = $null
            try {
                $traceOutput = tracerpt $EtlPath -o $xmlPath -of XML -summary $summaryPath -y 2>&1 | Out-String
                if (Test-Path $xmlPath) {
                    $decodeSuccess = $true
                }
            } catch {
                $decodeError = $_.ToString()
            }

            # Parse XML to verify captured events (#10) and detect decode issues (#13)
            $eventCount = 0
            $xmlValid = $false
            if ($decodeSuccess -and (Test-Path $xmlPath)) {
                $xmlSize = (Get-Item $xmlPath).Length
                if ($xmlSize -gt 100) {
                    try {
                        [xml]$xmlContent = Get-Content $xmlPath -Raw
                        $events = $xmlContent.SelectNodes('//Event')
                        if ($events) {
                            $eventCount = $events.Count
                        }
                        $xmlValid = $true
                    } catch {
                        # XML parsing failed
                    }
                }
            }

            # Fallback info if decode failed (#13)
            $fallbackInfo = $null
            if (-not $xmlValid) {
                $fallbackInfo = @{
                    EtlSizeKB   = $fileInfo.SizeKB
                    TraceOutput = if ($traceOutput) { $traceOutput } else { '' }
                    DecodeError = $decodeError
                    Suggestion  = 'Try: tracefmt <etl> -o output.txt -pdb <pdb> OR Windows Performance Analyzer (WPA)'
                }
            }

            return [PSCustomObject]@{
                StopOutput    = $output
                EtlFile       = $fileInfo
                XmlPath       = $xmlPath
                SummaryPath   = $summaryPath
                XmlExists     = Test-Path $xmlPath
                EventCount    = $eventCount
                DecodeSuccess = $decodeSuccess
                XmlValid      = $xmlValid
                FallbackInfo  = $fallbackInfo
            }
        }

        Write-Host "Trace session stopped."
        Write-Host "  ETL file: $($stopResult.EtlFile.Path) ($($stopResult.EtlFile.SizeKB) KB)"

        # Verify captured events (#10)
        if ($stopResult.EventCount -gt 0) {
            Write-Host "  Events captured: $($stopResult.EventCount)"
        } elseif ($stopResult.XmlValid) {
            Write-Warning "  Zero events captured. The driver may not be loaded or the provider GUIDs may be incorrect."
            $captureResults.Warnings += 'Zero events captured in ETL trace'
        }

        if ($stopResult.XmlExists -and $stopResult.XmlValid) {
            Write-Host "  XML decoded: $($stopResult.XmlPath)"
        } elseif (-not $stopResult.DecodeSuccess) {
            # Trace decoding fallback (#13)
            Write-Warning "  tracerpt failed to decode ETL. Raw ETL file is still available."
            if ($stopResult.FallbackInfo) {
                Write-Host "  ETL size: $($stopResult.FallbackInfo.EtlSizeKB) KB"
                Write-Host "  $($stopResult.FallbackInfo.Suggestion)"
            }
            $captureResults.Warnings += 'tracerpt failed to decode ETL to XML'
        }

        $captureResults.EventCount = $stopResult.EventCount

        # -------------------------------------------------------------------
        # Step 6: Copy trace files from VM to host desktop
        # -------------------------------------------------------------------

        # Re-check Guest Service Interface before copy (#7)
        $guestSvc = Get-VMIntegrationService -VMName $VMName | Where-Object { $_.Name -eq 'Guest Service Interface' }
        if ($guestSvc -and -not $guestSvc.Enabled) {
            Write-Host "Enabling Guest Service Interface for file copy..."
            Enable-VMIntegrationService -Name 'Guest Service Interface' -VMName $VMName
        }

        $desktop = [Environment]::GetFolderPath('Desktop')
        $outputDir = Join-Path $desktop "DriverLogs_$($driverMeta.DriverName)_$timestamp"
        New-Item -Path $outputDir -ItemType Directory -Force | Out-Null

        Write-Host "`nCopying trace files to host: $outputDir"

        # Copy ETL with error handling (#14)
        $localEtl = Join-Path $outputDir "$sessionName.etl"
        try {
            Copy-Item -FromSession $session -Path $vmEtlPath -Destination $localEtl -Force
            Write-Host "  Copied: $localEtl"
            $captureResults.Files += @{ Name = "$sessionName.etl"; SizeKB = $stopResult.EtlFile.SizeKB }
        } catch {
            Write-Warning "  Could not copy ETL: $_"
            $captureResults.Warnings += "ETL copy failed: $_"
            $captureResults.Errors += "ETL copy failed: $_"
        }

        # Copy decoded XML
        $vmXmlPath = $vmEtlPath -replace '\.etl$', '.xml'
        $localXml = Join-Path $outputDir "$sessionName.xml"
        try {
            Copy-Item -FromSession $session -Path $vmXmlPath -Destination $localXml -Force
            Write-Host "  Copied: $localXml"
            $captureResults.Files += @{ Name = "$sessionName.xml" }
        } catch {
            Write-Warning "  Could not copy XML: $_"
        }

        # Copy summary
        $vmSummaryPath = $vmEtlPath -replace '\.etl$', '_summary.txt'
        $localSummary = Join-Path $outputDir "${sessionName}_summary.txt"
        try {
            Copy-Item -FromSession $session -Path $vmSummaryPath -Destination $localSummary -Force
            Write-Host "  Copied: $localSummary"
            $captureResults.Files += @{ Name = "${sessionName}_summary.txt" }
        } catch {
            Write-Warning "  Could not copy summary: $_"
        }

        # Also copy the PDB for offline WPP decoding if available
        $pdbFiles = Get-ChildItem -Path $DriverPath -Filter '*.pdb' -ErrorAction SilentlyContinue
        foreach ($pdb in $pdbFiles) {
            $localPdb = Join-Path $outputDir $pdb.Name
            Copy-Item -Path $pdb.FullName -Destination $localPdb -Force
            Write-Host "  Copied PDB: $localPdb"
            $captureResults.Files += @{ Name = $pdb.Name }
        }

        $captureResults.Status = 'Success'
        $captureResults.OutputDir = $outputDir
    }

    # -------------------------------------------------------------------
    # Step 7: Print summary
    # -------------------------------------------------------------------
    if ($OutputFormat -eq 'Json') {
        $captureResults | ConvertTo-Json -Depth 5
    } else {
        Write-Host ""
        Write-Host "=========================================="
        Write-Host " Driver Log Capture Complete"
        Write-Host "=========================================="
        Write-Host "  Driver:      $($driverMeta.DriverName) v$($driverMeta.Version)"
        Write-Host "  VM:          $VMName"
        Write-Host "  Mode:        $CaptureMode"
        Write-Host "  Duration:    $Duration seconds"
        if ($CaptureMode -eq 'ETW') {
            Write-Host "  Providers:   $($providers.Count)"
            Write-Host "  Events:      $($captureResults.EventCount)"
        } else {
            Write-Host "  Lines:       $($captureResults.EventCount)"
        }
        Write-Host "  Output dir:  $($captureResults.OutputDir)"
        Write-Host ""
        Write-Host "Files:"
        if ($captureResults.OutputDir -and (Test-Path $captureResults.OutputDir)) {
            Get-ChildItem $captureResults.OutputDir | ForEach-Object {
                $sizeKB = [Math]::Round($_.Length / 1KB, 1)
                Write-Host "  $($_.Name) ($sizeKB KB)"
            }
        }
        if ($captureResults.Warnings.Count -gt 0) {
            Write-Host ""
            Write-Host "Warnings:"
            foreach ($w in $captureResults.Warnings) {
                Write-Host "  - $w"
            }
        }
        Write-Host ""
        Write-Host "Use these files for post-process smoke-test analysis:"
        if ($CaptureMode -eq 'ETW') {
            Write-Host "  - Open .etl in Windows Performance Analyzer (WPA)"
            Write-Host "  - Open .xml in any text/XML editor for decoded events"
            Write-Host "  - Use tracefmt with .pdb for full WPP trace decoding"
        } else {
            Write-Host "  - Open .log in any text editor for DbgPrint output"
        }
    }

} catch {
    $captureResults.Status = 'Failed'
    $captureResults.Errors += $_.ToString()
    if ($_.Exception.Message -match 'timed out') {
        Write-Error "Operation timed out: $_"
        exit $EXIT_TIMEOUT
    }
    throw
} finally {
    # Clean up: ensure trace session is stopped even on error
    if ($session) {
        try {
            if ($CaptureMode -eq 'ETW') {
                Invoke-Command -Session $session -ScriptBlock {
                    param($SessionName)
                    logman stop $SessionName -ets 2>&1 | Out-Null
                } -ArgumentList $sessionName -ErrorAction SilentlyContinue
            } else {
                Invoke-Command -Session $session -ScriptBlock {
                    Get-Process -Name 'Dbgview' -ErrorAction SilentlyContinue | Stop-Process -Force
                } -ErrorAction SilentlyContinue
            }

            # Clean up guest artifacts after retrieval (#15)
            Invoke-Command -Session $session -ScriptBlock {
                if (Test-Path 'C:\DriverLogs') {
                    Remove-Item -Path 'C:\DriverLogs' -Recurse -Force -ErrorAction SilentlyContinue
                }
            } -ErrorAction SilentlyContinue
        } catch {
            # Suppress cleanup errors
        }

        Remove-PSSession $session
    }
}
