param(
    [Parameter(Mandatory = $true)]
    [string]$EvrtckReportDir,
    [Parameter(Mandatory = $true)]
    [string]$HardwareReportDir,
    [double]$Budget60FpsMs = 16.667,
    [double]$Budget120FpsMs = 8.333
)

$ErrorActionPreference = "Stop"

$EvrtckReportDir = (Resolve-Path -LiteralPath $EvrtckReportDir).Path
$HardwareReportDir = (Resolve-Path -LiteralPath $HardwareReportDir).Path

$evrtckSummaryPath = Join-Path $EvrtckReportDir "summary.json"
$hardwareCsvPath = Join-Path $HardwareReportDir "hardware_codec_prod_bench.csv"
$hardwareMetadataPath = Join-Path $HardwareReportDir "metadata.json"
$outJsonPath = Join-Path $HardwareReportDir "evrtck_vs_hardware_summary.json"
$outMdPath = Join-Path $HardwareReportDir "evrtck_vs_hardware_summary.md"

if (!(Test-Path -LiteralPath $evrtckSummaryPath)) {
    throw "EVRTCK summary not found: $evrtckSummaryPath"
}
if (!(Test-Path -LiteralPath $hardwareCsvPath)) {
    throw "Hardware bench CSV not found: $hardwareCsvPath"
}

function To-Double($value) {
    if ($null -eq $value -or [string]::IsNullOrWhiteSpace([string]$value)) {
        return 0.0
    }
    return [double]::Parse(
        [string]$value,
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function To-Int64($value) {
    if ($null -eq $value -or [string]::IsNullOrWhiteSpace([string]$value)) {
        return 0
    }
    return [int64]::Parse(
        [string]$value,
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function FpsVerdict($p99Ms, $payloadBytes, $rawBytes) {
    $payloadRatio = if ($rawBytes -gt 0) { [double]$payloadBytes / [double]$rawBytes } else { 0.0 }
    if ($p99Ms -le $Budget120FpsMs) {
        return "120fps_ok"
    }
    if ($p99Ms -le $Budget60FpsMs) {
        return "60fps_ok"
    }
    if ($payloadRatio -ge 0.25) {
        return "large_payload"
    }
    return "over_budget"
}

function ChooseWinner($evrtck, $hardware) {
    if ($null -eq $hardware) {
        return "evrtck_no_hardware_data"
    }
    if ($evrtck.effective_p99_ms -le $Budget120FpsMs -and $evrtck.payload_ratio -lt 0.05) {
        return "evrtck"
    }
    if ($evrtck.effective_p99_ms -le $Budget60FpsMs -and $evrtck.effective_payload_bytes -lt $hardware.payload_bytes) {
        return "evrtck"
    }
    if ($hardware.p99_ms -lt $evrtck.effective_p99_ms -or $hardware.payload_bytes -lt $evrtck.effective_payload_bytes) {
        return "hardware"
    }
    return "evrtck"
}

function HardwareOperationRank($operation) {
    switch ([string]$operation) {
        "roundtrip" { return 0 }
        "decode" { return 1 }
        "encode" { return 2 }
        default { return 99 }
    }
}

$evrtckSummary = Get-Content -LiteralPath $evrtckSummaryPath -Raw | ConvertFrom-Json
$hardwareRows = Import-Csv -LiteralPath $hardwareCsvPath
$hardwareMetadata = if (Test-Path -LiteralPath $hardwareMetadataPath) {
    Get-Content -LiteralPath $hardwareMetadataPath -Raw | ConvertFrom-Json
} else {
    $null
}

$warnings = @()
if ($null -eq $hardwareMetadata) {
    $warnings += "hardware metadata is missing"
} else {
    foreach ($field in @("width", "height", "iterations", "warmup", "quick")) {
        $evrtckValue = $evrtckSummary.metadata.$field
        $hardwareValue = $hardwareMetadata.$field
        if ([string]$evrtckValue -ne [string]$hardwareValue) {
            $warnings += "metadata mismatch: $field EVRTCK=$evrtckValue hardware=$hardwareValue"
        }
    }
}
$publishable = $warnings.Count -eq 0

$decisions = @()
foreach ($decision in $evrtckSummary.decisions) {
    $scenario = [string]$decision.scenario
    $hardwareForScenario = $hardwareRows |
        Where-Object {
            $_.scenario -eq $scenario -and
            $_.operation -in @("roundtrip", "decode", "encode") -and
            $_.available -eq "true"
        } |
        ForEach-Object {
            [pscustomobject]@{
                codec = [string]$_.codec
                operation = [string]$_.operation
                operation_rank = HardwareOperationRank $_.operation
                p99_ms = [Math]::Round((To-Double $_.p99_us) / 1000.0, 3)
                p95_ms = [Math]::Round((To-Double $_.p95_us) / 1000.0, 3)
                payload_bytes = To-Int64 $_.payload_bytes
                packets_emitted = To-Int64 $_.packets_emitted
                backend = [string]$_.backend
            }
        } |
        Sort-Object @{ Expression = "operation_rank"; Ascending = $true }, @{ Expression = "p99_ms"; Ascending = $true }, @{ Expression = "payload_bytes"; Ascending = $true }

    $bestHardware = $hardwareForScenario | Select-Object -First 1
    $evrtckP99 = if ($null -ne $decision.hinted_roundtrip_p99_ms) {
        To-Double $decision.hinted_roundtrip_p99_ms
    } else {
        To-Double $decision.roundtrip_p99_ms
    }
    $evrtckPayload = if ($null -ne $decision.hinted_payload_bytes) {
        To-Int64 $decision.hinted_payload_bytes
    } else {
        To-Int64 $decision.payload_bytes
    }
    $rawBytes = To-Int64 $decision.raw_bytes
    $evrtckData = [pscustomobject]@{
        effective_p99_ms = $evrtckP99
        effective_payload_bytes = $evrtckPayload
        payload_ratio = if ($rawBytes -gt 0) { [double]$evrtckPayload / [double]$rawBytes } else { 1.0 }
    }

    $decisions += [pscustomobject]@{
        scenario = $scenario
        evrtck_p99_ms = $evrtckP99
        evrtck_payload_bytes = $evrtckPayload
        evrtck_verdict = [string]$decision.verdict
        hardware_codec = if ($bestHardware) { $bestHardware.codec } else { $null }
        hardware_operation = if ($bestHardware) { $bestHardware.operation } else { $null }
        hardware_p99_ms = if ($bestHardware) { $bestHardware.p99_ms } else { $null }
        hardware_payload_bytes = if ($bestHardware) { $bestHardware.payload_bytes } else { $null }
        hardware_verdict = if ($bestHardware) { FpsVerdict $bestHardware.p99_ms $bestHardware.payload_bytes $rawBytes } else { "no_hardware_data" }
        winner = ChooseWinner $evrtckData $bestHardware
    }
}

$fallbackHardware = $decisions | Where-Object {
    $null -ne $_.hardware_operation -and $_.hardware_operation -ne "roundtrip"
}
if ($fallbackHardware.Count -gt 0) {
    $warnings += "some hardware comparisons use decode/encode fallback because roundtrip rows were unavailable"
    $publishable = $false
}
$tinyHardwarePayload = $decisions | Where-Object {
    $null -ne $_.hardware_operation -and
    $_.hardware_operation -eq "roundtrip" -and
    $null -ne $_.hardware_payload_bytes -and
    $_.hardware_payload_bytes -lt 64
}
if ($tinyHardwarePayload.Count -gt 0) {
    $warnings += "some hardware roundtrip payloads are under 64 bytes; verify the encoded stream is a meaningful scene update before using scheduler conclusions"
    $publishable = $false
}

$summary = [ordered]@{
    generated_at = (Get-Date).ToString("o")
    evrtck_report_dir = $EvrtckReportDir
    hardware_report_dir = $HardwareReportDir
    budgets = [ordered]@{
        fps_60_ms = $Budget60FpsMs
        fps_120_ms = $Budget120FpsMs
    }
    publishable = $publishable
    warnings = $warnings
    evrtck_metadata = $evrtckSummary.metadata
    hardware_metadata = $hardwareMetadata
    decisions = $decisions
}

$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $outJsonPath -Encoding utf8

$md = New-Object System.Collections.Generic.List[string]
$md.Add("# EVRTCK vs hardware codec benchmark summary")
$md.Add("")
$md.Add("- EVRTCK report: ``$EvrtckReportDir``")
$md.Add("- Hardware report: ``$HardwareReportDir``")
$md.Add("- Generated: $($summary.generated_at)")
$md.Add("- 60 FPS budget: $Budget60FpsMs ms")
$md.Add("- 120 FPS budget: $Budget120FpsMs ms")
$md.Add("- Publishable: ``$publishable``")
if ($warnings.Count -gt 0) {
    $md.Add("")
    $md.Add("Warnings:")
    $md.Add("")
    foreach ($warning in $warnings) {
        $md.Add("- $warning")
    }
}
$md.Add("")
$md.Add("| scenario | EVRTCK p99 ms | EVRTCK payload | EVRTCK verdict | HW codec | HW op | HW p99 ms | HW payload | HW verdict | winner |")
$md.Add("|---|---:|---:|---|---|---|---:|---:|---|---|")
foreach ($decision in $decisions) {
    $md.Add("| $($decision.scenario) | $($decision.evrtck_p99_ms) | $($decision.evrtck_payload_bytes) | $($decision.evrtck_verdict) | $($decision.hardware_codec) | $($decision.hardware_operation) | $($decision.hardware_p99_ms) | $($decision.hardware_payload_bytes) | $($decision.hardware_verdict) | $($decision.winner) |")
}
$md.Add("")
$md.Add("Notes:")
$md.Add("")
$md.Add("- EVRTCK values use hinted roundtrip when available: encode + decode after base-frame state is established.")
$md.Add("- Hardware values prefer roundtrip rows. If roundtrip is unavailable, the report falls back to decode/encode rows and marks the comparison non-publishable.")
$md.Add("- A hardware winner here means the scheduler should prefer silicon for this scene class, not that transport/presentation latency is already solved.")

$md | Set-Content -LiteralPath $outMdPath -Encoding utf8

[pscustomobject]@{
    SummaryMarkdown = $outMdPath
    SummaryJson = $outJsonPath
    Decisions = $decisions.Count
}
