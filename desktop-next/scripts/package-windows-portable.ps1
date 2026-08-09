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
    throw "Portable package version must be numeric X.Y.Z, got '$version'"
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

    $packageName = "EvertyDeskNext-$version-windows-x64-portable"
    $staging = Join-Path $dist $packageName
    $zip = Join-Path $dist "$packageName.zip"

    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
    if (Test-Path -LiteralPath $zip) {
        Remove-Item -LiteralPath $zip -Force
    }
    if (Test-Path -LiteralPath "$zip.sha256") {
        Remove-Item -LiteralPath "$zip.sha256" -Force
    }

    New-Item -ItemType Directory -Force -Path $staging | Out-Null

    foreach ($file in @(
        "evertydesk-launcher.exe",
        "evertydesk-viewer.exe",
        "evertydesk-rdp-viewer.exe"
    )) {
        $source = Join-Path $binDir $file
        if (!(Test-Path -LiteralPath $source)) {
            throw "Required release binary is missing: $source"
        }
        Copy-Item -LiteralPath $source -Destination (Join-Path $staging $file)
    }

    $readme = @"
EvertyDesk Next $version portable package

Run:
  evertydesk-launcher.exe

Keep these files in the same directory:
  evertydesk-launcher.exe
  evertydesk-viewer.exe
  evertydesk-rdp-viewer.exe

Passwords are not stored in this folder. User data is stored under the normal
EvertyDesk application data directory.
"@
    Set-Content -LiteralPath (Join-Path $staging "README.txt") -Value $readme -Encoding utf8

    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $zip -CompressionLevel Optimal
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$zip.sha256" -Value "$hash  $(Split-Path -Leaf $zip)" -Encoding ascii

    & (Join-Path $root "scripts\validate-release-artifacts.ps1") `
        -Version $version `
        -DistDir $dist `
        -SkipManifest | Out-Null

    [pscustomobject]@{
        Version = $version
        PortableZip = $zip
        Sha256 = "$zip.sha256"
    }
}
finally {
    Pop-Location
}
