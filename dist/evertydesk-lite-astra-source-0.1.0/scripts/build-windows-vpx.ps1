param(
    [string]$W64DevkitVersion = "2.8.0"
)

$ErrorActionPreference = "Stop"

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ToolsDir = Join-Path $Root "engine\tools"
$DownloadsDir = Join-Path $Root "engine\downloads"
$W64Dir = Join-Path $ToolsDir "w64devkit"
$Archive = Join-Path $DownloadsDir "w64devkit-x64-$W64DevkitVersion.7z.exe"
$Url = "https://github.com/skeeto/w64devkit/releases/download/v$W64DevkitVersion/w64devkit-x64-$W64DevkitVersion.7z.exe"

New-Item -ItemType Directory -Force $ToolsDir, $DownloadsDir | Out-Null

if (!(Test-Path $W64Dir)) {
    if (!(Test-Path $Archive)) {
        Write-Host "Downloading w64devkit $W64DevkitVersion..."
        curl.exe -L -o $Archive $Url
    }

    Write-Host "Extracting w64devkit..."
    Start-Process -FilePath $Archive -ArgumentList @("-y", "-o$ToolsDir") -NoNewWindow -Wait
}

$Bin = Join-Path $W64Dir "bin"
$GccLibDir = Join-Path $W64Dir "lib\gcc\x86_64-w64-mingw32\16.1.0"
$LibGcc = Join-Path $GccLibDir "libgcc.a"
$LibGccEh = Join-Path $GccLibDir "libgcc_eh.a"

if ((Test-Path $LibGcc) -and !(Test-Path $LibGccEh)) {
    Copy-Item -Force $LibGcc $LibGccEh
}

$env:PATH = "$Bin;$env:PATH"

rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu --features live-vpx

Write-Host ""
Write-Host "Built:"
Write-Host (Join-Path $Root "target\x86_64-pc-windows-gnu\release\evertydesk-lite.exe")
