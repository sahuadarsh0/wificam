<#
.SYNOPSIS
    Downloads and launches the official Microsoft-signed VB-Audio Virtual Cable
    installer so Windows exposes a real virtual microphone ("CABLE Output") for
    WifiCam Studio to feed into Google Meet, Teams, Zoom, Discord, and OBS.
#>
$ErrorActionPreference = "Stop"

$Url = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip"
$WorkDir = Join-Path $env:TEMP "WifiCam-VBCable"
$ZipPath = Join-Path $WorkDir "VBCABLE_Driver_Pack45.zip"

New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null

Write-Host "Downloading official signed VB-Audio Virtual Cable driver pack..." -ForegroundColor Cyan
Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

Write-Host "Extracting driver archive..." -ForegroundColor Cyan
Expand-Archive -Path $ZipPath -DestinationPath $WorkDir -Force

$SetupExe = Join-Path $WorkDir "VBCABLE_Setup_x64.exe"
if (-not (Test-Path $SetupExe)) {
    throw "Could not find VBCABLE_Setup_x64.exe in $WorkDir"
}

Write-Host "Launching elevated VB-Cable installer (click 'Install Driver' in the window)..." -ForegroundColor Green
Start-Process -FilePath $SetupExe -Verb RunAs -Wait
Write-Host "Done! Restart or click 'Refresh Mic' in WifiCam Studio, then select 'CABLE Output (VB-Audio Virtual Cable)' as your microphone in Meet/Teams/Zoom." -ForegroundColor Green
