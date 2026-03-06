[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Param1,
    [Parameter(Mandatory)]
    [string]$Param2
)
Write-Host "Param1: '$Param1'"
Write-Host "Param2: '$Param2'"
