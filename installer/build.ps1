# Builds the release exe and the Inno Setup installer -> dist\Mutatio-Setup-<version>.exe
# Usage (from anywhere):  powershell -ExecutionPolicy Bypass -File installer\build.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 1. Release binary (icon + version info are embedded by build.rs).
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# 2. Wizard artwork generated from Mutatio.jpg. Inno wants BMPs; the square logo is letterboxed
#    onto the dark background so the tall side panel isn't stretched.
Add-Type -AssemblyName System.Drawing
$buildDir = Join-Path $PSScriptRoot 'build'
New-Item -ItemType Directory -Force -Path $buildDir | Out-Null
$logo = [System.Drawing.Image]::FromFile((Join-Path $root 'Mutatio.jpg'))

function New-WizardBitmap([int]$w, [int]$h, [int]$pad, [string]$path) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.Clear([System.Drawing.Color]::FromArgb(25, 25, 28))
    $size = [Math]::Min($w, $h) - 2 * $pad
    $g.DrawImage($logo, [int](($w - $size) / 2), [int](($h - $size) / 2), $size, $size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $g.Dispose(); $bmp.Dispose()
}
# 2x the 100%-DPI sizes (164x314 / 55x55) so the art stays sharp on scaled displays.
New-WizardBitmap 328 628 16 (Join-Path $buildDir 'wizard.bmp')
New-WizardBitmap 110 110 0 (Join-Path $buildDir 'wizard-small.bmp')
$logo.Dispose()

# 3. Compile the installer.
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it: winget install JRSoftware.InnoSetup" }

& $iscc /Q (Join-Path $PSScriptRoot 'mutatio.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

Get-ChildItem (Join-Path $root 'dist') -Filter 'Mutatio-Setup-*.exe' | Select-Object Name, Length, LastWriteTime
