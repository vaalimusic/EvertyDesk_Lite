param(
    [string]$BaseUrl = "http://127.0.0.1:5180"
)

$ErrorActionPreference = "Stop"

$statePath = Join-Path $env:TEMP "everty-control-plane-ready-smoke-$([Guid]::NewGuid().ToString('N')).json"
$process = $null

try {
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath
    $out = Join-Path $env:TEMP "everty-control-plane-ready.out"
    $err = Join-Path $env:TEMP "everty-control-plane-ready.err"
    Remove-Item -LiteralPath $out,$err -ErrorAction SilentlyContinue
    $process = Start-Process -FilePath dotnet `
        -ArgumentList @("run", "--project", "control-plane/Everty.ControlPlane.csproj", "--urls", $BaseUrl) `
        -WorkingDirectory (Get-Location) `
        -RedirectStandardOutput $out `
        -RedirectStandardError $err `
        -PassThru

    $ready = $null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 500
        try {
            $ready = Invoke-RestMethod -Method "GET" -Uri "$BaseUrl/api/ready" -TimeoutSec 2
            break
        } catch {
        }
    }

    if ($null -eq $ready) {
        throw "ready endpoint did not respond."
    }

    if (-not $ready.ready -or -not $ready.persistenceWritable) {
        throw "control-plane is not ready: $($ready | ConvertTo-Json -Compress)"
    }

    [pscustomobject]@{
        ready = $ready.ready
        persistenceWritable = $ready.persistenceWritable
        persistencePath = $ready.persistencePath
    } | ConvertTo-Json -Compress
}
finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    Remove-Item -LiteralPath $statePath,"$statePath.tmp","$statePath.ready" -ErrorAction SilentlyContinue
}
