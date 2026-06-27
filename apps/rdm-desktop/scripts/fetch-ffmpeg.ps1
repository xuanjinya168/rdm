<#
.SYNOPSIS
    Download the ffmpeg.exe that is bundled with the app (incl. NVENC/QSV/AMF
    hardware encoders).

.DESCRIPTION
    Places a static Windows ffmpeg.exe at src-tauri/binaries/ffmpeg.exe so that
    tauri.conf.json's bundle.resources can package it. The binary is NOT checked
    into git; run this once before `npm run tauri build` (or in CI).

    Defaults to BtbN's GPL build (includes the hardware encoders). Pass -Url to
    pin a specific version.

.EXAMPLE
    pwsh apps/rdm-desktop/scripts/fetch-ffmpeg.ps1
#>
param(
    [string]$Url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$binDir = [System.IO.Path]::GetFullPath((Join-Path $scriptDir "..\src-tauri\binaries"))
$target = Join-Path $binDir "ffmpeg.exe"

New-Item -ItemType Directory -Force -Path $binDir | Out-Null

if (Test-Path $target) {
    Write-Host "ffmpeg.exe already present: $target (delete it and rerun to update)"
    & $target -hide_banner -version | Select-Object -First 1
    exit 0
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ffmpeg-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$zip = Join-Path $tmp "ffmpeg.zip"

Write-Host "Downloading ffmpeg: $Url"
Invoke-WebRequest -Uri $Url -OutFile $zip

Write-Host "Extracting..."
Expand-Archive -Path $zip -DestinationPath $tmp -Force

$exe = Get-ChildItem -Path $tmp -Recurse -Filter "ffmpeg.exe" | Select-Object -First 1
if ($null -eq $exe) {
    throw "ffmpeg.exe not found inside the archive"
}

Copy-Item -Path $exe.FullName -Destination $target -Force
Remove-Item -Recurse -Force $tmp

Write-Host "Wrote: $target"
& $target -hide_banner -version | Select-Object -First 1
