param(
    [string]$BaseUrl = "http://127.0.0.1:5182"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http

$statePath = Join-Path $env:TEMP "everty-control-plane-admin-dashboard-smoke-$([Guid]::NewGuid().ToString('N')).json"
$out = Join-Path $env:TEMP "everty-control-plane-admin-dashboard.out"
$err = Join-Path $env:TEMP "everty-control-plane-admin-dashboard.err"
$process = $null
$previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
$previousOperatorKey = $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY

try {
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath
    $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY = "dashboard-smoke-key"

    Remove-Item -LiteralPath $out,$err -ErrorAction SilentlyContinue
    $process = Start-Process -FilePath dotnet `
        -ArgumentList @("run", "--no-build", "--project", "control-plane/Everty.ControlPlane.csproj", "--urls", $BaseUrl) `
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

    if ($null -eq $ready -or -not $ready.ready) {
        throw "control-plane did not become ready."
    }

    $httpClient = [System.Net.Http.HttpClient]::new()
    $httpClient.Timeout = [TimeSpan]::FromSeconds(5)
    $response = $httpClient.GetAsync("$BaseUrl/admin").GetAwaiter().GetResult()
    $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()

    if (-not $response.IsSuccessStatusCode) {
        throw "dashboard returned $([int]$response.StatusCode)."
    }

    if ($response.Content.Headers.ContentType.MediaType -ne "text/html") {
        throw "dashboard content type was $($response.Content.Headers.ContentType)."
    }

    foreach ($required in @("Everty Operator Console", "/api/admin/summary", "/api/admin/sessions", "/api/admin/billing/summary", "/api/admin/billing/reconciliation", '/api/admin/hosts/${hostId}/offer', "operator_console_disable_host", "operator_console_settle", "operator_console_retry")) {
        if (-not $body.Contains($required)) {
            throw "dashboard did not contain expected marker: $required"
        }
    }

    [pscustomobject]@{
        dashboard = "ok"
        contentType = $response.Content.Headers.ContentType.MediaType
        length = $body.Length
    } | ConvertTo-Json -Compress
}
finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }

    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $previousStatePath
    $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY = $previousOperatorKey

    Remove-Item -LiteralPath $statePath,"$statePath.tmp","$statePath.ready" -ErrorAction SilentlyContinue
}
