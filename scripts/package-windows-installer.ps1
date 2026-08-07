# Builds evertydesk-launcher.exe / evertydesk-viewer.exe in release mode and
# packages them into an unsigned MSI via WiX v5.
#
# Prerequisites (not installed by this script):
#   dotnet tool install --global wix
#   wix extension add WixToolset.UI.wixext
#
# Usage:
#   powershell -File scripts\package-windows-installer.ps1
#
# Output: dist\EvertyDeskLite-<version>-x64.msi, plus a .sha256 file next to
# it (paste that hash straight into your update manifest's "sha256" field).
#
# Unsigned: Windows SmartScreen will warn on first run until this is signed
# with a real Authenticode certificate. See desktop-next\RELEASE.md.

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$DesktopNext = Join-Path $RepoRoot "desktop-next"
$DistDir = Join-Path $RepoRoot "dist"

Write-Host "==> Building release binaries"
Push-Location $DesktopNext
try {
    cargo build --release --bin evertydesk-launcher --bin evertydesk-viewer --features viewer-core
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

$BinDir = Join-Path $DesktopNext "target\release"
$AssetsDir = Join-Path $DesktopNext "assets"
$WixDir = Join-Path $DesktopNext "wix"

$CargoToml = Get-Content (Join-Path $DesktopNext "Cargo.toml") -Raw
if ($CargoToml -notmatch 'version\s*=\s*"([^"]+)"') {
    throw "could not find version in desktop-next\Cargo.toml"
}
$Version = $Matches[1]
Write-Host "==> Packaging version $Version"

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
$OutputMsi = Join-Path $DistDir "EvertyDeskLite-$Version-x64.msi"

wix build (Join-Path $WixDir "main.wxs") `
    -d "EvertyDeskVersion=$Version" `
    -d "BinDir=$BinDir" `
    -d "AssetsDir=$AssetsDir" `
    -ext WixToolset.UI.wixext `
    -arch x64 `
    -o $OutputMsi

if ($LASTEXITCODE -ne 0) { throw "wix build failed" }

$Hash = (Get-FileHash -Path $OutputMsi -Algorithm SHA256).Hash.ToLower()
Set-Content -Path "$OutputMsi.sha256" -Value $Hash -Encoding utf8 -NoNewline

Write-Host "==> Built $OutputMsi"
Write-Host "==> SHA256: $Hash"
