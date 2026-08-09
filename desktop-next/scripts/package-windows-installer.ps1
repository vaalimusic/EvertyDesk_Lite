param(
    [string]$Configuration = "release",
    [switch]$StopRunningBuildProcesses
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$dist = Join-Path $root "dist"
$version = $env:EVERTYDESK_RELEASE_VERSION

if ([string]::IsNullOrWhiteSpace($version)) {
    $cargoToml = Get-Content -LiteralPath (Join-Path $root "Cargo.toml") -Raw
    if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
        throw "Cannot resolve package version from Cargo.toml"
    }
    $version = $Matches[1]
}

if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "MSI ProductVersion must be numeric X.Y.Z, got '$version'"
}

$wix = Get-Command wix -ErrorAction SilentlyContinue
if ($null -eq $wix) {
    throw "WiX CLI not found. Install: dotnet tool install --global wix --version 5.0.2"
}

Push-Location $root
try {
    $binDir = Join-Path $root "target\$Configuration"
    $releaseProcessNames = @(
        "evertydesk-launcher",
        "evertydesk-viewer",
        "evertydesk-rdp-viewer"
    )
    $runningBuildProcesses = Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $releaseProcessNames -contains $_.ProcessName -and $_.Path -like "$binDir*" }
    if ($runningBuildProcesses) {
        if (!$StopRunningBuildProcesses) {
            $ids = ($runningBuildProcesses | ForEach-Object { "$($_.ProcessName):$($_.Id)" }) -join ", "
            throw "Release binaries are running and cannot be overwritten: $ids. Re-run with -StopRunningBuildProcesses."
        }
        foreach ($process in $runningBuildProcesses) {
            Stop-Process -Id $process.Id -Force
        }
    }

    cargo build --release --features viewer-core --bins
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    New-Item -ItemType Directory -Force -Path $dist | Out-Null

    $assetsDir = Join-Path $root "assets"
    $wixDir = Join-Path $root "wix"
    $wxs = Join-Path $wixDir "main.wxs"
    $msi = Join-Path $dist "EvertyDeskNext-$version-x64.msi"

    foreach ($required in @(
        (Join-Path $binDir "evertydesk-launcher.exe"),
        (Join-Path $binDir "evertydesk-viewer.exe"),
        (Join-Path $binDir "evertydesk-rdp-viewer.exe"),
        (Join-Path $assetsDir "logo.ico"),
        (Join-Path $wixDir "license.rtf"),
        $wxs
    )) {
        if (!(Test-Path -LiteralPath $required)) {
            throw "Required release input is missing: $required"
        }
    }

    & (Join-Path $root "scripts\validate-windows-installer-layout.ps1") -Configuration $Configuration | Out-Null

    wix build `
        "$wxs" `
        -ext WixToolset.UI.wixext/5.0.2 `
        -d EvertyDeskVersion=$version `
        -d BinDir="$binDir" `
        -d AssetsDir="$assetsDir" `
        -d WixDir="$wixDir" `
        -arch x64 `
        -o "$msi"
    if ($LASTEXITCODE -ne 0) {
        throw "wix build failed with exit code $LASTEXITCODE"
    }

    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $msi).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$msi.sha256" -Value "$hash  $(Split-Path -Leaf $msi)" -Encoding ascii

    [pscustomobject]@{
        Version = $version
        Installer = $msi
        Sha256 = "$msi.sha256"
    }
}
finally {
    Pop-Location
}
