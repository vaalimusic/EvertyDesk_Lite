param(
    [Parameter(Mandatory = $true)]
    [string]$ReportDir,
    [double]$Budget60FpsMs = 16.667,
    [double]$Budget120FpsMs = 8.333,
    [double]$PayloadWarnRatio = 0.25
)

$ErrorActionPreference = "Stop"

$ReportDir = (Resolve-Path -LiteralPath $ReportDir).Path
$csvPath = Join-Path $ReportDir "evrtck_prod_bench.csv"
$metadataPath = Join-Path $ReportDir "metadata.json"
$summaryMdPath = Join-Path $ReportDir "summary.md"
$summaryJsonPath = Join-Path $ReportDir "summary.json"

if (!(Test-Path -LiteralPath $csvPath)) {
    throw "EVRTCK bench CSV not found: $csvPath"
}
if (!(Test-Path -LiteralPath $metadataPath)) {
    throw "EVRTCK bench metadata not found: $metadataPath"
}

$rows = Import-Csv -LiteralPath $csvPath
$metadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json

function To-Double($value) {
    return [double]::Parse(
        [string]$value,
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function To-Int64($value) {
    return [int64]::Parse(
        [string]$value,
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function VerdictFor($roundtripRow) {
    if ($null -eq $roundtripRow) {
        return "no_roundtrip"
    }

    $p99Ms = (To-Double $roundtripRow.p99_us) / 1000.0
    $payload = To-Int64 $roundtripRow.payload_bytes
    $raw = [Math]::Max(1, (To-Int64 $roundtripRow.raw_bytes))
    $payloadRatio = [double]$payload / [double]$raw
    $dirtyRatio = To-Double $roundtripRow.dirty_ratio_target
    $entropy = [string]$roundtripRow.dirty_entropy

    if ($p99Ms -le $Budget120FpsMs -and $payloadRatio -lt $PayloadWarnRatio) {
        return "evrtck_120fps_ok"
    }
    if ($p99Ms -le $Budget60FpsMs -and $payloadRatio -lt $PayloadWarnRatio) {
        return "evrtck_60fps_ok"
    }
    if ($payloadRatio -ge $PayloadWarnRatio -or ($entropy -eq "Noise" -and $dirtyRatio -ge 0.5)) {
        return "prefer_hardware_codec"
    }
    return "scheduler_decision_needed"
}

$scenarioNames = $rows |
    Where-Object { $_.operation -ne "encode" -or $_.scenario -ne "keyframe_gradient" } |
    Select-Object -ExpandProperty scenario -Unique

$decisions = @()
foreach ($scenario in $scenarioNames) {
    $encode = $rows | Where-Object { $_.scenario -eq $scenario -and $_.operation -eq "encode" } | Select-Object -First 1
    $decode = $rows | Where-Object { $_.scenario -eq $scenario -and $_.operation -eq "decode" } | Select-Object -First 1
    $roundtrip = $rows | Where-Object { $_.scenario -eq $scenario -and $_.operation -eq "roundtrip" } | Select-Object -First 1
    if ($null -eq $roundtrip) {
        continue
    }

    $payload = To-Int64 $roundtrip.payload_bytes
    $raw = [Math]::Max(1, (To-Int64 $roundtrip.raw_bytes))
    $payloadRatio = [double]$payload / [double]$raw

    $decisions += [pscustomobject]@{
        scenario = $scenario
        dirty_ratio_target = To-Double $roundtrip.dirty_ratio_target
        distribution = $roundtrip.dirty_distribution
        entropy = $roundtrip.dirty_entropy
        payload_bytes = $payload
        raw_bytes = $raw
        payload_ratio = [Math]::Round($payloadRatio, 6)
        dirty_tiles = To-Int64 $roundtrip.dirty_tiles
        total_tiles = To-Int64 $roundtrip.total_tiles
        encode_p99_ms = if ($encode) { [Math]::Round((To-Double $encode.p99_us) / 1000.0, 3) } else { $null }
        decode_p99_ms = if ($decode) { [Math]::Round((To-Double $decode.p99_us) / 1000.0, 3) } else { $null }
        roundtrip_p50_ms = [Math]::Round((To-Double $roundtrip.p50_us) / 1000.0, 3)
        roundtrip_p95_ms = [Math]::Round((To-Double $roundtrip.p95_us) / 1000.0, 3)
        roundtrip_p99_ms = [Math]::Round((To-Double $roundtrip.p99_us) / 1000.0, 3)
        verdict = VerdictFor $roundtrip
    }
}

$keyframe = $rows | Where-Object { $_.scenario -eq "keyframe_gradient" -and $_.operation -eq "encode" } | Select-Object -First 1

$summary = [ordered]@{
    generated_at = (Get-Date).ToString("o")
    source_report_dir = $ReportDir
    metadata = $metadata
    budgets = [ordered]@{
        fps_60_ms = $Budget60FpsMs
        fps_120_ms = $Budget120FpsMs
        payload_warn_ratio = $PayloadWarnRatio
    }
    keyframe = if ($keyframe) {
        [ordered]@{
            payload_bytes = To-Int64 $keyframe.payload_bytes
            p50_ms = [Math]::Round((To-Double $keyframe.p50_us) / 1000.0, 3)
            p95_ms = [Math]::Round((To-Double $keyframe.p95_us) / 1000.0, 3)
            p99_ms = [Math]::Round((To-Double $keyframe.p99_us) / 1000.0, 3)
        }
    } else {
        $null
    }
    decisions = $decisions
}

$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryJsonPath -Encoding utf8

$md = New-Object System.Collections.Generic.List[string]
$md.Add("# EVRTCK production benchmark summary")
$md.Add("")
$md.Add("- Report: ``$ReportDir``")
$md.Add("- Generated: $($summary.generated_at)")
$md.Add("- Commit: ``$($metadata.commit)``")
$md.Add("- Dirty tree: ``$($metadata.working_tree_dirty)``")
$md.Add("- CPU: $($metadata.cpu.name), cores=$($metadata.cpu.cores), threads=$($metadata.cpu.logical_processors), max_clock_mhz=$($metadata.cpu.max_clock_mhz)")
$md.Add("- OS: $($metadata.os.caption) $($metadata.os.version) build $($metadata.os.build_number)")
$md.Add("- Resolution: $($metadata.width)x$($metadata.height)")
$md.Add("- Iterations: $($metadata.iterations), warmup: $($metadata.warmup), quick: $($metadata.quick)")
$md.Add("")
$md.Add("Budgets:")
$md.Add("")
$md.Add("- 60 FPS frame budget: $Budget60FpsMs ms")
$md.Add("- 120 FPS frame budget: $Budget120FpsMs ms")
$md.Add("- Payload warning threshold: $([Math]::Round($PayloadWarnRatio * 100.0, 1))% of raw BGRA")
$md.Add("")
if ($keyframe) {
    $md.Add("## Keyframe")
    $md.Add("")
    $md.Add("| payload bytes | p50 ms | p95 ms | p99 ms |")
    $md.Add("|---:|---:|---:|---:|")
    $md.Add("| $($summary.keyframe.payload_bytes) | $($summary.keyframe.p50_ms) | $($summary.keyframe.p95_ms) | $($summary.keyframe.p99_ms) |")
    $md.Add("")
}
$md.Add("## P-frame decision map")
$md.Add("")
$md.Add("| scenario | payload | payload/raw | encode p99 ms | decode p99 ms | roundtrip p99 ms | verdict |")
$md.Add("|---|---:|---:|---:|---:|---:|---|")
foreach ($decision in $decisions) {
    $payloadPct = [Math]::Round($decision.payload_ratio * 100.0, 2)
    $md.Add("| $($decision.scenario) | $($decision.payload_bytes) | $payloadPct% | $($decision.encode_p99_ms) | $($decision.decode_p99_ms) | $($decision.roundtrip_p99_ms) | $($decision.verdict) |")
}
$md.Add("")
$md.Add("## Interpretation")
$md.Add("")
$md.Add("- ``evrtck_120fps_ok``: codec roundtrip p99 is inside 120 FPS budget and payload is not suspiciously large.")
$md.Add("- ``evrtck_60fps_ok``: codec roundtrip p99 is inside 60 FPS budget but not 120 FPS budget.")
$md.Add("- ``prefer_hardware_codec``: payload or entropy profile says scheduler should prefer H.264/H.265/AV1 silicon for this kind of frame.")
$md.Add("- ``scheduler_decision_needed``: neither path is obvious from this codec-only bench; compare with hardware codec data.")
$md.Add("")
$md.Add("Roundtrip here means EVRTCK P-frame encode plus EVRTCK P-frame decode after base-frame state is established. It excludes capture, network, encryption, presentation, and OS scheduling delay.")

$md | Set-Content -LiteralPath $summaryMdPath -Encoding utf8

[pscustomobject]@{
    SummaryMarkdown = $summaryMdPath
    SummaryJson = $summaryJsonPath
    Decisions = $decisions.Count
}
