param(
    [string]$BaseUrl = "http://127.0.0.1:5202",
    [int]$ProviderPort = 5203,
    [string]$OperatorKey = "payment-provider-smoke-key",
    [ValidateSet("", "hold", "capture", "settle")]
    [string]$FailAction = "",
    [switch]$FailOnce
)

$ErrorActionPreference = "Stop"

$statePath = Join-Path $env:TEMP "everty-control-plane-payment-provider-smoke-$([Guid]::NewGuid().ToString('N')).json"
$providerLogPath = Join-Path $env:TEMP "everty-payment-provider-smoke-$([Guid]::NewGuid().ToString('N')).jsonl"
$out = Join-Path $env:TEMP "everty-control-plane-payment-provider.out"
$err = Join-Path $env:TEMP "everty-control-plane-payment-provider.err"
$process = $null
$providerJob = $null
$providerEndpoint = "http://127.0.0.1:$ProviderPort/payments/"

$previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
$previousOperatorKey = $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY
$previousPaymentProvider = $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER
$previousPaymentProviderEndpoint = $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT
$previousPaymentProviderApiKey = $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY

function Invoke-ControlPlaneJson {
    param(
        [string]$Method,
        [string]$Path,
        [object]$Body = $null,
        [string]$Token = "",
        [string]$OperatorKeyHeader = ""
    )

    $headers = @{}
    if ($Token) {
        $headers["Authorization"] = "Bearer $Token"
    }
    if ($OperatorKeyHeader) {
        $headers["X-Everty-Operator-Key"] = $OperatorKeyHeader
    }

    if ($null -eq $Body) {
        return Invoke-RestMethod -Method $Method -Uri "$BaseUrl$Path" -Headers $headers -TimeoutSec 5
    }

    return Invoke-RestMethod -Method $Method `
        -Uri "$BaseUrl$Path" `
        -Headers $headers `
        -Body ($Body | ConvertTo-Json -Depth 8) `
        -ContentType "application/json" `
        -TimeoutSec 5
}

try {
    Remove-Item -LiteralPath $out,$err,$providerLogPath -ErrorAction SilentlyContinue

    $providerJob = Start-Job -ScriptBlock {
        param($Prefix, $LogPath, $FailAction, $FailOnce)

        $listener = [System.Net.HttpListener]::new()
        $listener.Prefixes.Add($Prefix)
        $listener.Start()
        $failedActions = @{}

        while ($listener.IsListening) {
            try {
                $context = $listener.GetContext()
                $reader = [System.IO.StreamReader]::new($context.Request.InputStream, $context.Request.ContentEncoding)
                $body = $reader.ReadToEnd()
                $request = $body | ConvertFrom-Json
                $action = [string]$request.action
                $sessionId = [string]$request.sessionId
                Add-Content -LiteralPath $LogPath -Value $body

                if ($FailAction -and $action -eq $FailAction -and (-not $FailOnce -or -not $failedActions.ContainsKey($action))) {
                    $failedActions[$action] = $true
                    $errorResponse = @{
                        error = "forced_$action_failure"
                    } | ConvertTo-Json -Compress
                    $errorBytes = [System.Text.Encoding]::UTF8.GetBytes($errorResponse)
                    $context.Response.StatusCode = 503
                    $context.Response.ContentType = "application/json"
                    $context.Response.OutputStream.Write($errorBytes, 0, $errorBytes.Length)
                    $context.Response.Close()
                    continue
                }

                $response = @{
                    providerReferenceId = "callbacktest-$action-$sessionId"
                    provider = "callbacktest"
                    recordedUtc = [DateTimeOffset]::UtcNow.ToString("O")
                } | ConvertTo-Json -Compress

                $bytes = [System.Text.Encoding]::UTF8.GetBytes($response)
                $context.Response.StatusCode = 200
                $context.Response.ContentType = "application/json"
                $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
                $context.Response.Close()
            } catch {
                if ($listener.IsListening) {
                    throw
                }
            }
        }
    } -ArgumentList $providerEndpoint,$providerLogPath,$FailAction,$FailOnce.IsPresent

    Start-Sleep -Milliseconds 500
    if ($providerJob.State -ne "Running") {
        throw "payment provider listener did not start: $($providerJob | Receive-Job -Keep)"
    }

    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath
    $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY = $OperatorKey
    $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER = "callbacktest"
    $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT = $providerEndpoint
    $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY = "payment-smoke-secret"

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

    $runtimeConfig = Invoke-ControlPlaneJson -Method "GET" -Path "/api/config/runtime"
    if ($runtimeConfig.paymentProvider -ne "callbacktest" -or $runtimeConfig.paymentProviderMode -ne "external_http" -or $runtimeConfig.paymentProviderEndpointConfigured -ne $true) {
        throw "runtime config did not report external_http provider: $($runtimeConfig | ConvertTo-Json -Compress)"
    }

    $provider = Invoke-ControlPlaneJson -Method "GET" -Path "/api/admin/billing/provider" -OperatorKeyHeader $OperatorKey
    if ($provider.mode -ne "external_http" -or $provider.externalCallsEnabled -ne $true -or $provider.endpointConfigured -ne $true) {
        throw "billing provider endpoint did not report external HTTP mode: $($provider | ConvertTo-Json -Compress)"
    }

    $device = Invoke-ControlPlaneJson -Method "POST" -Path "/api/auth/device-login" -Body @{
        deviceLabel = "payment-provider-smoke-device"
        platform = "powershell"
    }

    $registeredHost = Invoke-ControlPlaneJson -Method "POST" -Path "/api/hosts/register" -Body @{
        displayName = "payment-provider-smoke-host"
        region = "local"
        directAddress = "127.0.0.1"
        directPort = 5001
        encoderBackends = @("nvenc")
        supportsHevc = $true
        supportsAudio = $true
        supportsGamepad = $true
        capabilities = @{
            cpuModel = "smoke"
            gpuModel = "smoke"
            ramGb = 16
            maxWidth = 1920
            maxHeight = 1080
            maxFps = 60
        }
    }

    [void](Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/hosts/$($registeredHost.hostId)/offer" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{
            listed = $true
            pricePerHour = 3.25
            currency = "USD"
            description = "payment provider smoke offer"
        })

    $session = Invoke-ControlPlaneJson -Method "POST" -Path "/api/sessions" -Token $device.accessToken -Body @{
        hostId = $registeredHost.hostId
        clientLabel = "payment-provider-smoke-client"
        clientRegion = "local"
        codecPreference = "h265"
        preferRelay = $false
        audioRequested = $true
        controllerCount = 1
        leaseMinutes = 5
        receiverAddress = "127.0.0.1"
        receiverPort = 5001
        requestedWidth = 1280
        requestedHeight = 720
        requestedFps = 60
        requestedBitrateBps = 8000000
        captureCursor = $true
        adaptiveMode = $false
    }

    $billingSession = Invoke-ControlPlaneJson -Method "GET" -Path "/api/billing/sessions/$($session.sessionId)" -Token $device.accessToken
    if ($billingSession.paymentProvider -ne "callbacktest" -or $billingSession.providerHoldId -notlike "callbacktest-hold-*") {
        throw "billing hold did not use external provider: $($billingSession | ConvertTo-Json -Compress)"
    }

    [void](Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/sessions/$($session.sessionId)/stop" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{ reason = "payment_provider_smoke_stop" })

    if ($FailAction -eq "capture") {
        $failedBilling = Invoke-ControlPlaneJson -Method "GET" -Path "/api/billing/sessions/$($session.sessionId)" -Token $device.accessToken
        if ($failedBilling.status -ne 5 -or -not $failedBilling.lastPaymentError -or $failedBilling.providerCaptureId) {
            throw "billing capture failure was not recorded correctly: $($failedBilling | ConvertTo-Json -Compress)"
        }

        $reconciliation = Invoke-ControlPlaneJson -Method "GET" -Path "/api/admin/billing/reconciliation" -OperatorKeyHeader $OperatorKey
        if (-not ($reconciliation | Where-Object { $_.sessionId -eq $session.sessionId -and $_.actionRequired -eq "capture" })) {
            throw "billing reconciliation did not include failed capture: $($reconciliation | ConvertTo-Json -Compress)"
        }

        if ($FailOnce) {
            $retriedBilling = Invoke-ControlPlaneJson -Method "POST" `
                -Path "/api/admin/billing/sessions/$($session.sessionId)/retry" `
                -OperatorKeyHeader $OperatorKey `
                -Body @{
                    action = "auto"
                    reason = "payment_provider_smoke_retry_capture"
                }

            if ($retriedBilling.status -ne 2 -or $retriedBilling.providerCaptureId -notlike "callbacktest-capture-*" -or $retriedBilling.lastPaymentError) {
                throw "billing retry did not recover capture failure: $($retriedBilling | ConvertTo-Json -Compress)"
            }
        }

        $providerRequests = Get-Content -LiteralPath $providerLogPath | ConvertFrom-Json
        if (-not ($providerRequests | Where-Object { $_.action -eq "capture" -and $_.sessionId -eq $session.sessionId })) {
            throw "payment provider did not receive forced capture failure request: $($providerRequests | ConvertTo-Json -Compress)"
        }

        [pscustomobject]@{
            runtimeConfig = "ok"
            billingProvider = "ok"
            externalHold = "ok"
            forcedCaptureFailure = "ok"
            reconciliation = "ok"
            retryCapture = if ($FailOnce) { "ok" } else { "skipped" }
            providerCallbacks = ($providerRequests | Measure-Object).Count
        }
        return
    }

    $settledBilling = Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/billing/sessions/$($session.sessionId)/settle" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{ reason = "payment_provider_smoke_settle" }

    if ($settledBilling.providerCaptureId -notlike "callbacktest-capture-*" -or $settledBilling.providerSettlementId -notlike "callbacktest-settle-*") {
        throw "billing settle did not use external provider callback ids: $($settledBilling | ConvertTo-Json -Compress)"
    }

    $providerRequests = Get-Content -LiteralPath $providerLogPath | ConvertFrom-Json
    foreach ($requiredAction in @("hold", "capture", "settle")) {
        if (-not ($providerRequests | Where-Object { $_.action -eq $requiredAction -and $_.sessionId -eq $session.sessionId })) {
            throw "payment provider did not receive $requiredAction for session $($session.sessionId): $($providerRequests | ConvertTo-Json -Compress)"
        }
    }

    [pscustomobject]@{
        runtimeConfig = "ok"
        billingProvider = "ok"
        externalHold = "ok"
        externalCapture = "ok"
        externalSettlement = "ok"
        providerCallbacks = ($providerRequests | Measure-Object).Count
    }
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit(5000) | Out-Null
    }
    if ($providerJob) {
        Stop-Job -Job $providerJob -ErrorAction SilentlyContinue
        Remove-Job -Job $providerJob -Force -ErrorAction SilentlyContinue
    }

    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $previousStatePath
    $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY = $previousOperatorKey
    $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER = $previousPaymentProvider
    $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_ENDPOINT = $previousPaymentProviderEndpoint
    $env:EVERTY_CONTROL_PLANE_PAYMENT_PROVIDER_API_KEY = $previousPaymentProviderApiKey

    Remove-Item -LiteralPath $statePath,$providerLogPath -ErrorAction SilentlyContinue
}
