param(
    [int]$Iterations = 300,
    [int]$Warmup = 30,
    [int]$Width = 1920,
    [int]$Height = 1080,
    [int]$Fps = 60,
    [int]$Bitrate = 8000000,
    [string]$Codec = "H264,H265",
    [switch]$Quick,
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $root "reports\hardware-codec-prod\$timestamp"
}
$OutDir = (New-Item -ItemType Directory -Force -Path $OutDir).FullName

Push-Location $root
try {
    $commit = (git rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($commit)) {
        $commit = "unknown"
    }
    $branch = (git rev-parse --abbrev-ref HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($branch)) {
        $branch = "unknown"
    }
    $dirty = (git status --porcelain 2>$null)
    $isDirty = -not [string]::IsNullOrWhiteSpace(($dirty -join ""))

    $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
    $os = Get-CimInstance Win32_OperatingSystem | Select-Object -First 1
    $rustc = (rustc -Vv) -join "`n"
    if ($LASTEXITCODE -ne 0) {
        throw "rustc -Vv failed with exit code $LASTEXITCODE"
    }
    $cargo = (cargo -V) -join "`n"
    if ($LASTEXITCODE -ne 0) {
        throw "cargo -V failed with exit code $LASTEXITCODE"
    }

    $metadata = [ordered]@{
        generated_at = (Get-Date).ToString("o")
        profile = "release"
        command = "cargo run --release --features live-vp9-mf --bin hardware_codec_prod_bench"
        commit = $commit.Trim()
        branch = $branch.Trim()
        working_tree_dirty = $isDirty
        width = $Width
        height = $Height
        fps = $Fps
        bitrate = $Bitrate
        codec = $Codec
        iterations = $Iterations
        warmup = $Warmup
        quick = [bool]$Quick
        cpu = [ordered]@{
            name = $cpu.Name
            manufacturer = $cpu.Manufacturer
            logical_processors = $cpu.NumberOfLogicalProcessors
            cores = $cpu.NumberOfCores
            max_clock_mhz = $cpu.MaxClockSpeed
        }
        os = [ordered]@{
            caption = $os.Caption
            version = $os.Version
            build_number = $os.BuildNumber
            architecture = $os.OSArchitecture
        }
        rustc = $rustc
        cargo = $cargo
        outputs = [ordered]@{
            csv = (Join-Path $OutDir "hardware_codec_prod_bench.csv")
            jsonl = (Join-Path $OutDir "hardware_codec_prod_bench.jsonl")
            metadata = (Join-Path $OutDir "metadata.json")
        }
    }

    $metadataPath = Join-Path $OutDir "metadata.json"
    $metadata | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $metadataPath -Encoding utf8

    $args = @(
        "run",
        "--release",
        "--features",
        "live-vp9-mf",
        "--bin",
        "hardware_codec_prod_bench",
        "--",
        "--width",
        $Width,
        "--height",
        $Height,
        "--fps",
        $Fps,
        "--bitrate",
        $Bitrate,
        "--codec",
        $Codec,
        "--iterations",
        $Iterations,
        "--warmup",
        $Warmup,
        "--out",
        $OutDir
    )
    if ($Quick) {
        $args += "--quick"
    }

    cargo @args
    if ($LASTEXITCODE -ne 0) {
        throw "Hardware codec production bench failed with exit code $LASTEXITCODE"
    }

    [pscustomobject]@{
        OutputDirectory = $OutDir
        Metadata = $metadataPath
        Csv = Join-Path $OutDir "hardware_codec_prod_bench.csv"
        Jsonl = Join-Path $OutDir "hardware_codec_prod_bench.jsonl"
    }
}
finally {
    Pop-Location
}
