$ErrorActionPreference = "Stop"

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Dist = Join-Path $Root "dist"
$CargoToml = Join-Path $Root "Cargo.toml"

$versionLine = Select-String -LiteralPath $CargoToml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) {
    throw "Cannot read package version from Cargo.toml"
}

$Version = $versionLine.Matches[0].Groups[1].Value
$PackageName = "evertydesk-lite-astra-source-$Version"
$Stage = Join-Path $Dist $PackageName
$ZipPath = Join-Path $Dist "$PackageName.zip"
$TarGzPath = Join-Path $Dist "$PackageName.tar.gz"

function Assert-PathUnder {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Parent
    )

    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $fullParent = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if (-not $fullPath.StartsWith($fullParent, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to operate outside $fullParent`: $fullPath"
    }
}

function Copy-SourcePath {
    param([Parameter(Mandatory = $true)][string]$RelativePath)

    $source = Join-Path $Root $RelativePath
    if (-not (Test-Path -LiteralPath $source)) {
        return
    }

    $destination = Join-Path $Stage $RelativePath
    $destinationParent = Split-Path -Parent $destination
    New-Item -ItemType Directory -Force -Path $destinationParent | Out-Null
    Copy-Item -LiteralPath $source -Destination $destination -Recurse -Force
}

New-Item -ItemType Directory -Force -Path $Dist | Out-Null
Assert-PathUnder -Path $Stage -Parent $Dist

if (Test-Path -LiteralPath $Stage) {
    Remove-Item -LiteralPath $Stage -Recurse -Force
}
if (Test-Path -LiteralPath $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
}
if (Test-Path -LiteralPath $TarGzPath) {
    Remove-Item -LiteralPath $TarGzPath -Force
}

New-Item -ItemType Directory -Force -Path $Stage | Out-Null

$paths = @(
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    ".cargo",
    "src",
    "scripts",
    "docs",
    "README.md",
    "BUILD_LINUX.md",
    "EVRT_ROADMAP.md",
    "ROADMAP.md",
    "WINDOWS_DEV_GUIDE.md",
    "edesk_lite_logo.png"
)

foreach ($path in $paths) {
    Copy-SourcePath -RelativePath $path
}

$readme = @"
# EvertyDesk Lite Astra source package

This archive contains source code only. It intentionally does not include:

- Windows/Android build artifacts
- target/
- local keystores or secrets
- NVIDIA Video Codec SDK
- local toolchains from engine/
- external RustDesk/EvertyGame working copies

## Build on Astra Linux

Install Rust if it is not installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "\$HOME/.cargo/env"
```

Install build dependencies:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config cmake nasm \
    libx11-dev libxcb1-dev libxkbcommon-dev \
    libgl1-mesa-dev libegl1-mesa-dev \
    libasound2-dev libxtst-dev xdotool libvpx-dev
```

Build the Astra-friendly profile:

```bash
chmod +x scripts/*.sh
./scripts/build-linux.sh astra
```

Run:

```bash
./target/release/evertydesk-lite
```

If GLX/OpenGL is unstable in the VM, use the CPU UI path:

```bash
EVERTYDESK_RENDERER=software ./target/release/evertydesk-lite
```

On Astra, auto startup prefers the CPU framebuffer UI first. Override only for
diagnostics:

```bash
# Force normal GL auto attempts before CPU fallback.
EVERTYDESK_LINUX_GL_AUTO=1 ./target/release/evertydesk-lite

# Allow WGPU as an extra auto candidate.
EVERTYDESK_LINUX_AUTO_WGPU=1 ./target/release/evertydesk-lite
```

Install for the current user:

```bash
./scripts/install-linux-user.sh
```

Runtime diagnostics:

```bash
./scripts/run-linux-safe.sh --check
```

## Codec profile

For Astra use `./scripts/build-linux.sh astra`, which builds with:

```bash
cargo build --release --no-default-features --features desktop-gui,live-h264,live-vpx-system
```

That uses eframe/egui for the desktop UI, OpenH264, and the system `libvpx-dev` package instead of trying to build libvpx from source on the Astra toolchain.
"@

Set-Content -LiteralPath (Join-Path $Stage "ASTRA_README.md") -Value $readme -Encoding UTF8

$manifest = @(
    "Package: $PackageName",
    "Version: $Version",
    "Kind: source-only Astra Linux build package",
    "Created: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')",
    "",
    "Included paths:",
    ($paths | ForEach-Object { "  - $_" }),
    "  - ASTRA_README.md",
    "",
    "Excluded intentionally:",
    "  - target/",
    "  - android/",
    "  - diagnostics/",
    "  - dist/",
    "  - engine/",
    "  - screenshots/",
    "  - Video_Codec_SDK_*",
    "  - rustdesk-master/",
    "  - EvertyGame-main/"
)
Set-Content -LiteralPath (Join-Path $Stage "MANIFEST.txt") -Value $manifest -Encoding UTF8

Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $Stage,
    $ZipPath,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $true
)

$tar = Get-Command tar -ErrorAction SilentlyContinue
if ($tar) {
    Push-Location $Dist
    try {
        & $tar.Source -czf "$PackageName.tar.gz" "$PackageName"
        if ($LASTEXITCODE -ne 0) {
            throw "tar failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

foreach ($artifact in @($ZipPath, $TarGzPath)) {
    if (Test-Path -LiteralPath $artifact) {
        $hash = Get-FileHash -LiteralPath $artifact -Algorithm SHA256
        Set-Content -LiteralPath "$artifact.sha256" -Value "$($hash.Hash.ToLowerInvariant())  $([System.IO.Path]::GetFileName($artifact))" -Encoding ASCII
    }
}

Get-ChildItem -LiteralPath $Dist -File |
    Where-Object { $_.Name -like "$PackageName*" } |
    Select-Object FullName, Length, LastWriteTime
