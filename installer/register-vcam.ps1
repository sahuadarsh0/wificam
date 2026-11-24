<#
.SYNOPSIS
    Installs and registers WifiCam's Windows 11 Media Foundation Virtual Camera COM DLL
    into C:\Program Files\WifiCam\vcam and removes any legacy DirectShow filter.
.DESCRIPTION
    Run from an elevated (Administrator) PowerShell prompt.
    Pass -Unregister to cleanly unregister and remove the COM DLL.
#>
param(
    [switch]$Unregister
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$InstallDir = Join-Path $env:ProgramFiles "WifiCam\vcam"

$MfSrc = Join-Path $RepoRoot "pc-vcam-mf\build\Release\pc-vcam-mf.dll"
$MfDst = Join-Path $InstallDir "pc-vcam-mf.dll"
$LegacyDsDst = Join-Path $InstallDir "pc-vcam-ds.dll"

# Always clean up any legacy DirectShow ghost camera left over from v0.1.0-v0.1.4.
if (Test-Path $LegacyDsDst) {
    & regsvr32.exe /s /u "$LegacyDsDst"
    Remove-Item -Force $LegacyDsDst -ErrorAction SilentlyContinue
}
Remove-Item -Path "HKLM:\SOFTWARE\Classes\CLSID\{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance\{C8F5A0B2-7D3E-4A29-8F4B-2E5D6C7F8091}" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path "HKLM:\SOFTWARE\Classes\CLSID\{C8F5A0B2-7D3E-4A29-8F4B-2E5D6C7F8091}" -Recurse -Force -ErrorAction SilentlyContinue

if ($Unregister) {
    if (Test-Path $MfDst) { & regsvr32.exe /s /u "$MfDst" }
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    Write-Host "WifiCam Virtual Camera DLL unregistered cleanly." -ForegroundColor Green
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
