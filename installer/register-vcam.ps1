<#
.SYNOPSIS
    Developer helper to install & register WifiCam's Virtual Camera COM DLLs
    into C:\Program Files\WifiCam\vcam so Windows 11 FrameServer can load them.
.DESCRIPTION
    Run from an elevated (Administrator) PowerShell prompt.
    Pass -Unregister to cleanly unregister and remove the COM DLLs.
#>
param(
    [switch]$Unregister
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$InstallDir = Join-Path $env:ProgramFiles "WifiCam\vcam"

$MfSrc = Join-Path $RepoRoot "pc-vcam-mf\build\Release\pc-vcam-mf.dll"
$DsSrc = Join-Path $RepoRoot "pc-vcam-ds\build\Release\pc-vcam-ds.dll"
$MfDst = Join-Path $InstallDir "pc-vcam-mf.dll"
$DsDst = Join-Path $InstallDir "pc-vcam-ds.dll"

if ($Unregister) {
    if (Test-Path $MfDst) { & regsvr32.exe /s /u "$MfDst" }
    if (Test-Path $DsDst) { & regsvr32.exe /s /u "$DsDst" }
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    Write-Host "WifiCam Virtual Camera DLLs unregistered cleanly." -ForegroundColor Green
    exit 0
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

if (Test-Path $MfSrc) {
    Copy-Item -Force $MfSrc $MfDst
    & regsvr32.exe /s "$MfDst"
    Write-Host "Registered Windows 11 Media Foundation Virtual Camera: $MfDst" -ForegroundColor Cyan
} else {
    Write-Warning "Not built yet: $MfSrc (run cmake --build pc-vcam-mf/build --config Release)"
}

if (Test-Path $DsSrc) {
    Copy-Item -Force $DsSrc $DsDst
    & regsvr32.exe /s "$DsDst"
    Write-Host "Registered Windows 10 DirectShow Virtual Camera: $DsDst" -ForegroundColor Cyan
} else {
    Write-Warning "Not built yet: $DsSrc (run cmake --build pc-vcam-ds/build --config Release)"
}
