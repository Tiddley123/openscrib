# Builds the portable Windows release: one folder (and a zip of it) that runs
# from anywhere, including a USB stick, with no installer or admin rights.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
#
# Cisco's OpenH264 and the AI models are NOT included: OpenH264 must be
# fetched from Cisco by the user (the app does it on first run), and models
# are downloaded on demand from Settings -> Models.

param(
    # Folder with the MSVC redistributable DLLs to ship next to the exes.
    [string]$Redist = "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Redist\MSVC\14.29.30133\x64",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$name = "OpenSCRIB-$version-windows-x64"
$out = Join-Path $root "dist\$name"

if (-not $SkipBuild) {
    Push-Location $root
    cargo build --release -p openscrib-cli -p openscrib-vision
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    Push-Location app
    npx tauri build --no-bundle
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed" }
    Pop-Location
    Pop-Location
}

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force "$out\models" | Out-Null

$release = Join-Path $root "target\release"
Copy-Item "$release\openscrib-app.exe" "$out\OpenSCRIB.exe"
# Not "openscrib.exe": Windows names are case-insensitive, so it would
# overwrite OpenSCRIB.exe.
Copy-Item "$release\openscrib.exe" "$out\openscrib-cli.exe"
Copy-Item "$release\openscrib-vision.exe" "$out\openscrib-vision.exe"

# The Visual C++ runtime, deployed app-locally so nothing has to be installed.
$crt = @(
    "$Redist\Microsoft.VC142.CRT\msvcp140.dll",
    "$Redist\Microsoft.VC142.CRT\vcruntime140.dll",
    "$Redist\Microsoft.VC142.CRT\vcruntime140_1.dll",
    "$Redist\Microsoft.VC142.OpenMP\vcomp140.dll"
)
foreach ($dll in $crt) {
    if (-not (Test-Path $dll)) { throw "Missing $dll; pass -Redist with your MSVC redist folder" }
    Copy-Item $dll $out
}

Copy-Item "$root\LICENSE-MIT" "$out\LICENSE-MIT.txt"
@"
OpenSCRIB $version - portable

Run OpenSCRIB.exe. Everything stays in this folder:
  projects\   your recordings and exports
  models\     AI models you download from Settings -> Models

First run: click "Get OpenH264" on the home screen to enable screen video.
It downloads Cisco's free encoder (under 1 MB) next to the app.

Command line: openscrib-cli.exe --help
"@ | Set-Content -Encoding utf8 "$out\README.txt"
"Downloaded models are stored here." | Set-Content -Encoding utf8 "$out\models\README.txt"

$zip = "$out.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $out -DestinationPath $zip
$size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Built $out"
Write-Host "Zipped $zip ($size MB)"
