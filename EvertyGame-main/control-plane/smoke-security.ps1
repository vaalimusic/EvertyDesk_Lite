param(
    [string]$BaseUrl = "http://127.0.0.1:5180"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http

$statePath = Join-Path $env:TEMP "everty-control-plane-security-smoke-$([Guid]::NewGuid().ToString('N')).json"
$out = Join-Path $env:TEMP "everty-control-plane-security.out"
$err = Join-Path $env:TEMP "everty-control-plane-security.err"
$process = $null

$previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
$previousAccessHours = $env:EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS
$previousRefreshDays = $env:EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS
$previousMaxRequestBytes = $env:EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES

try {
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath
    $env:EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS = "2"
    $env:EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS = "7"
    $env:EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES = "32768"

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

    if ($null -eq $ready -or -not $ready.ready) {
        throw "control-plane did not become ready."
    }

    $config = Invoke-RestMethod -Method "GET" -Uri "$BaseUrl/api/config/runtime" -TimeoutSec 5
    if ($config.accessTokenHours -ne 2 -or $config.refreshTokenDays -ne 7 -or $config.maxRequestBodyBytes -ne 32768) {
        throw "runtime config did not reflect security env values: $($config | ConvertTo-Json -Compress)"
    }

    $httpClient = [System.Net.Http.HttpClient]::new()
    $httpClient.Timeout = [TimeSpan]::FromSeconds(5)
    $healthResponse = $httpClient.GetAsync("$BaseUrl/api/health").GetAwaiter().GetResult()
    if (-not $healthResponse.IsSuccessStatusCode) {
        throw "health endpoint returned $([int]$healthResponse.StatusCode)."
    }

    $headers = @{}
    foreach ($header in $healthResponse.Headers) {
        $headers[$header.Key] = ($header.Value -join ",")
    }
    foreach ($header in $healthResponse.Content.Headers) {
        $headers[$header.Key] = ($header.Value -join ",")
    }

    if ($headers["X-Content-Type-Options"] -ne "nosniff" -or
        $headers["X-Frame-Options"] -ne "DENY" -or
        $headers["Cache-Control"] -ne "no-store") {
        throw "security headers were missing or invalid: $($headers | ConvertTo-Json -Compress)"
    }

    $largeBody = @{
        deviceLabel = ("x" * 40000)
        platform = "powershell"
    } | ConvertTo-Json

    $tooLargeContent = [System.Net.Http.StringContent]::new($largeBody, [System.Text.Encoding]::UTF8, "application/json")
    $tooLargeResponse = $httpClient.PostAsync("$BaseUrl/api/auth/device-login", $tooLargeContent).GetAwaiter().GetResult()
    $tooLargeStatus = [int]$tooLargeResponse.StatusCode

    if ($tooLargeStatus -ne 413) {
        throw "large request was not rejected with 413; status=$tooLargeStatus"
    }

    [pscustomobject]@{
        runtimeConfig = "ok"
        securityHeaders = "ok"
        largeRequestStatus = $tooLargeStatus
    } | ConvertTo-Json -Compress
}
finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }

    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $previousStatePath
    $env:EVERTY_CONTROL_PLANE_ACCESS_TOKEN_HOURS = $previousAccessHours
    $env:EVERTY_CONTROL_PLANE_REFRESH_TOKEN_DAYS = $previousRefreshDays
    $env:EVERTY_CONTROL_PLANE_MAX_REQUEST_BODY_BYTES = $previousMaxRequestBytes

    Remove-Item -LiteralPath $statePath,"$statePath.tmp","$statePath.ready" -ErrorAction SilentlyContinue
}
