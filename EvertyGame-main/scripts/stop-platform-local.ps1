param(
    [string]$OutputRoot = "artifacts/platform"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$runDir = Join-Path (Join-Path $root $OutputRoot) "run"
$stopped = @()

foreach ($name in @("relay-node", "control-plane")) {
    $pidPath = Join-Path $runDir "$name.pid"
    if (-not (Test-Path $pidPath)) {
        continue
    }

    $pidValue = (Get-Content $pidPath -Raw).Trim()
    if ($pidValue) {
        $process = Get-Process -Id ([int]$pidValue) -ErrorAction SilentlyContinue
        if ($process) {
            Stop-Process -Id $process.Id -Force
            $stopped += [pscustomobject]@{
                name = $name
                pid = $process.Id
            }
        }
    }

    Remove-Item -LiteralPath $pidPath -Force
}

[pscustomobject]@{
    runDir = $runDir
    stopped = $stopped
} | ConvertTo-Json -Compress
