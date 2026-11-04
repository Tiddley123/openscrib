# Builds the Windows release: one folder that runs from anywhere with no
# installer or admin rights, a zip of it, and a per-user setup.exe that packs
# the same exes (scripts\openscrib-setup.nsi, needs NSIS).
#
# Copyright (c) 2026 Rovamp Services Pty Ltd. The exes carry this in their
# Windows file properties (tauri.conf.json and the build.rs files).
#
# Release flow with code signing:
#   1. powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 -NoZip
#   2. Sign the three .exe files in dist\OpenSCRIB-<version>-windows-x64\ (e.g. with SignGUI)
#   3. powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 -ZipOnly
#   4. Sign dist\OpenSCRIB-<version>-setup.exe
#
# Without signing, run it with no switches to do it all in one go.
#
# Cisco's OpenH264 and the AI models are NOT included: OpenH264 must be
# fetched from Cisco by the user (the app does it on first run), and models
# are downloaded on demand from Settings -> Models.

param(
    # Folder with the MSVC redistributable DLLs to ship next to the exes.
    [string]$Redist = "C:\Program Files (x86)\Microsoft Visual Studio\2019\BuildTools\VC\Redist\MSVC\14.29.30133\x64",
    [switch]$SkipBuild,
    # Stop before packing, so the exes can be signed first.
    [switch]$NoZip,
    # Only pack the existing (signed) folder; don't build or copy anything.
    [switch]$ZipOnly,
    # The NSIS compiler that builds the setup.exe.
    [string]$MakeNsis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$name = "OpenSCRIB-$version-windows-x64"
$out = Join-Path $root "dist\$name"
# Built from the code point: Windows PowerShell reads this file as ANSI.
$copyright = "Copyright $([char]0xA9) 2026 Rovamp Services Pty Ltd"

if (-not $ZipOnly) {
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

    # Never clear this folder first: it may be the copy someone is testing, and
    # portable mode keeps their recordings, models and OpenH264 in it. The
    # copies below overwrite what we ship and leave the rest alone.
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
$copyright

Run OpenSCRIB.exe. Everything stays in this folder:
  projects\   your recordings and exports
  models\     AI models you download from Settings -> Models

First run: click "Get OpenH264" on the home screen to enable screen video.
It downloads Cisco's free encoder (under 1 MB) next to the app.

Command line: openscrib-cli.exe --help
"@ | Set-Content -Encoding utf8 "$out\README.txt"
    "Downloaded models are stored here." | Set-Content -Encoding utf8 "$out\models\README.txt"

    # Confirm the copyright made it into every exe before anyone signs them.
    foreach ($exe in Get-ChildItem "$out\*.exe") {
        $info = $exe.VersionInfo
        if ($info.LegalCopyright -ne $copyright -or $info.CompanyName -ne "Rovamp Services Pty Ltd") {
            throw "$($exe.Name) is missing the Rovamp copyright in its file properties"
        }
    }
    Write-Host "Built $out"
}

if (-not (Test-Path $out)) { throw "$out doesn't exist; build it first" }
if ($NoZip) {
    Write-Host "Not zipped. Sign the .exe files in $out, then run again with -ZipOnly."
    exit 0
}

$unsigned = Get-ChildItem "$out\*.exe" | Where-Object { (Get-AuthenticodeSignature $_.FullName).Status -ne "Valid" }
if ($unsigned) {
    Write-Warning ("Not signed: " + ($unsigned.Name -join ", "))
}

$zip = "$out.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
# Only what we ship goes in the zip, not anything from testing this folder.
$stage = Join-Path $root "dist\.zip-stage"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
$top = New-Item -ItemType Directory -Force "$stage\$name\models"
Get-ChildItem $out -File | Where-Object { $_.Name -notlike "openh264*" } | Copy-Item -Destination "$stage\$name"
Copy-Item "$out\models\README.txt" $top.FullName
Compress-Archive -Path "$stage\$name" -DestinationPath $zip
Remove-Item -Recurse -Force $stage
$size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Zipped $zip ($size MB)"

if (-not (Test-Path $MakeNsis)) { throw "NSIS not found at $MakeNsis; pass -MakeNsis" }
$setup = Join-Path $root "dist\OpenSCRIB-$version-setup.exe"
& $MakeNsis /V2 "/DVERSION=$version" "/DSOURCE=$out" "/DOUTFILE=$setup" "$root\scripts\openscrib-setup.nsi"
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
$size = [math]::Round((Get-Item $setup).Length / 1MB, 1)
Write-Host "Built $setup ($size MB). It needs signing too."
