param(
    [string]$BaseUrl = "http://127.0.0.1:5180",
    [string]$OperatorKey = "admin-smoke-key"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Net.Http

$statePath = Join-Path $env:TEMP "everty-control-plane-admin-smoke-$([Guid]::NewGuid().ToString('N')).json"
$out = Join-Path $env:TEMP "everty-control-plane-admin.out"
$err = Join-Path $env:TEMP "everty-control-plane-admin.err"
$process = $null

$previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
$previousOperatorKey = $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY

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

    try {
        if ($null -eq $Body) {
            return Invoke-RestMethod -Method $Method -Uri "$BaseUrl$Path" -Headers $headers -TimeoutSec 5
        }

        return Invoke-RestMethod -Method $Method `
            -Uri "$BaseUrl$Path" `
            -Headers $headers `
            -Body ($Body | ConvertTo-Json -Depth 8) `
            -ContentType "application/json" `
            -TimeoutSec 5
    } catch {
        $responseBody = ""
        if ($_.Exception.Response) {
            $reader = [System.IO.StreamReader]::new($_.Exception.Response.GetResponseStream())
            $responseBody = $reader.ReadToEnd()
        }
        throw "$Method $Path failed: $($_.Exception.Message) $responseBody"
    }
}

try {
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath
    $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY = $OperatorKey

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
    if ($config.operatorAuthConfigured -ne $true) {
        throw "operator auth was not reported as configured: $($config | ConvertTo-Json -Compress)"
    }

    $httpClient = [System.Net.Http.HttpClient]::new()
    $httpClient.Timeout = [TimeSpan]::FromSeconds(5)

    $unauthorized = $httpClient.GetAsync("$BaseUrl/api/admin/summary").GetAwaiter().GetResult()
    if ([int]$unauthorized.StatusCode -ne 401) {
        throw "admin summary without operator key returned $([int]$unauthorized.StatusCode), expected 401."
    }

    $request = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Get, "$BaseUrl/api/admin/summary")
    $request.Headers.Add("X-Everty-Operator-Key", $OperatorKey)
    $summaryResponse = $httpClient.SendAsync($request).GetAwaiter().GetResult()
    if (-not $summaryResponse.IsSuccessStatusCode) {
        throw "admin summary with operator key returned $([int]$summaryResponse.StatusCode)."
    }

    $summaryJson = $summaryResponse.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    $summary = $summaryJson | ConvertFrom-Json
    if ($summary.operatorAuthConfigured -ne $true) {
        throw "admin summary did not report operator auth configured: $summaryJson"
    }

    $sessionsRequest = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Get, "$BaseUrl/api/admin/sessions")
    $sessionsRequest.Headers.Authorization = [System.Net.Http.Headers.AuthenticationHeaderValue]::new("Bearer", $OperatorKey)
    $sessionsResponse = $httpClient.SendAsync($sessionsRequest).GetAwaiter().GetResult()
    if (-not $sessionsResponse.IsSuccessStatusCode) {
        throw "admin sessions with bearer operator key returned $([int]$sessionsResponse.StatusCode)."
    }

    $device = Invoke-ControlPlaneJson -Method "POST" -Path "/api/auth/device-login" -Body @{
        deviceLabel = "admin-smoke-device"
        platform = "powershell"
    }

    $registeredHost = Invoke-ControlPlaneJson -Method "POST" -Path "/api/hosts/register" -Body @{
        displayName = "admin-smoke-host"
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

    $offer = Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/hosts/$($registeredHost.hostId)/offer" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{
            listed = $true
            pricePerHour = 2.5
            currency = "USD"
            description = "admin smoke offer"
        }
    if ($offer.hostId -ne $registeredHost.hostId -or $offer.pricePerHour -ne 2.5 -or $offer.currency -ne "USD") {
        throw "admin host offer did not return expected offer: $($offer | ConvertTo-Json -Compress)"
    }

    $marketplaceHosts = Invoke-ControlPlaneJson -Method "GET" -Path "/api/marketplace/hosts" -Token $device.accessToken
    if (-not ($marketplaceHosts | Where-Object { $_.hostId -eq $registeredHost.hostId -and $_.currency -eq "USD" })) {
        throw "marketplace hosts did not include listed host offer: $($marketplaceHosts | ConvertTo-Json -Compress)"
    }

    $relay = Invoke-ControlPlaneJson -Method "POST" -Path "/api/relay/register" -Body @{
        displayName = "admin-smoke-relay"
        region = "local"
        publicAddress = "127.0.0.1"
        udpPort = 6294
        availability = 1
    }

    $session = Invoke-ControlPlaneJson -Method "POST" -Path "/api/sessions" -Token $device.accessToken -Body @{
        hostId = $registeredHost.hostId
        clientLabel = "admin-smoke-client"
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

    $billingSummary = Invoke-ControlPlaneJson -Method "GET" -Path "/api/admin/billing/summary" -OperatorKeyHeader $OperatorKey
    if ($billingSummary.totalHolds -lt 1 -or $billingSummary.pendingHolds -lt 1) {
        throw "admin billing summary did not report active hold: $($billingSummary | ConvertTo-Json -Compress)"
    }

    $billingProvider = Invoke-ControlPlaneJson -Method "GET" -Path "/api/admin/billing/provider" -OperatorKeyHeader $OperatorKey
    if ($billingProvider.provider -ne "manual" -or $billingProvider.configured -ne $true) {
        throw "admin billing provider did not report manual provider: $($billingProvider | ConvertTo-Json -Compress)"
    }

    $billingSession = Invoke-ControlPlaneJson -Method "GET" -Path "/api/billing/sessions/$($session.sessionId)" -Token $device.accessToken
    if ($billingSession.status -ne 1 -or $billingSession.holdAmount -lt 0 -or $billingSession.hourlyRate -ne 2.5 -or -not $billingSession.providerHoldId) {
        throw "billing session did not report expected hold state: $($billingSession | ConvertTo-Json -Compress)"
    }

    $billingAccounts = Invoke-ControlPlaneJson -Method "GET" -Path "/api/admin/billing/accounts" -OperatorKeyHeader $OperatorKey
    if (-not ($billingAccounts | Where-Object { $_.hostId -eq $registeredHost.hostId })) {
        throw "admin billing accounts did not include host account: $($billingAccounts | ConvertTo-Json -Compress)"
    }

    $billingLedger = Invoke-ControlPlaneJson -Method "GET" -Path "/api/admin/billing/ledger?limit=10" -OperatorKeyHeader $OperatorKey
    if (-not ($billingLedger | Where-Object { $_.sessionId -eq $session.sessionId -and $_.kind -eq "hold_reserved" })) {
        throw "admin billing ledger did not include hold entry: $($billingLedger | ConvertTo-Json -Compress)"
    }

    $stoppedSession = Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/sessions/$($session.sessionId)/stop" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{ reason = "admin_smoke_stop" }
    if ($stoppedSession.status -ne 2 -or $stoppedSession.stopReason -ne "admin_smoke_stop") {
        throw "admin session stop did not return stopped session: $($stoppedSession | ConvertTo-Json -Compress)"
    }

    $settledBilling = Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/billing/sessions/$($session.sessionId)/settle" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{ reason = "admin_smoke_settle" }
    if ($settledBilling.status -ne 3 -or -not $settledBilling.providerCaptureId -or -not $settledBilling.providerSettlementId) {
        throw "admin billing settle did not settle session: $($settledBilling | ConvertTo-Json -Compress)"
    }

    $disabledHost = Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/hosts/$($registeredHost.hostId)/availability" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{
            availability = "Disabled"
            reason = "admin_smoke_disable"
            stopActiveSession = $false
        }
    if ($disabledHost.availability -ne 3) {
        throw "admin host availability did not disable host: $($disabledHost | ConvertTo-Json -Compress)"
    }

    $disabledRelay = Invoke-ControlPlaneJson -Method "POST" `
        -Path "/api/admin/relays/$($relay.relayId)/availability" `
        -OperatorKeyHeader $OperatorKey `
        -Body @{
            availability = "Disabled"
            reason = "admin_smoke_disable"
        }
    if ($disabledRelay.availability -ne 2) {
        throw "admin relay availability did not disable relay: $($disabledRelay | ConvertTo-Json -Compress)"
    }

    [pscustomobject]@{
        operatorRuntimeConfig = "ok"
        unauthorizedStatus = [int]$unauthorized.StatusCode
        adminSummary = "ok"
        adminSessions = "ok"
        adminBillingSummary = "ok"
        adminBillingProvider = "ok"
        adminBillingAccounts = "ok"
        adminBillingLedger = "ok"
        billingSession = "ok"
        adminHostOffer = "ok"
        marketplaceHosts = "ok"
        adminSessionStop = "ok"
        adminBillingSettle = "ok"
        adminHostAvailability = "ok"
        adminRelayAvailability = "ok"
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
