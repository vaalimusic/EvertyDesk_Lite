param(
    [string]$BaseUrl = "http://127.0.0.1:5181",
    [string]$OperatorKey = "admin-cli-smoke-key"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$statePath = Join-Path $env:TEMP "everty-control-plane-admin-cli-smoke-$([Guid]::NewGuid().ToString('N')).json"
$out = Join-Path $env:TEMP "everty-control-plane-admin-cli.out"
$err = Join-Path $env:TEMP "everty-control-plane-admin-cli.err"
$process = $null

$previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
$previousOperatorKey = $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY

function Invoke-ControlPlaneJson {
    param(
        [string]$Method,
        [string]$Path,
        [object]$Body = $null,
        [string]$Token = ""
    )

    $headers = @{}
    if ($Token) {
        $headers["Authorization"] = "Bearer $Token"
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

function Invoke-AdminCli {
    param([string[]]$Arguments)

    $output = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "scripts/control-plane-admin.ps1") @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "control-plane-admin.ps1 failed: $output"
    }

    return ($output | Out-String | ConvertFrom-Json)
}

try {
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath
    $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY = $OperatorKey

    Remove-Item -LiteralPath $out,$err -ErrorAction SilentlyContinue
    $process = Start-Process -FilePath dotnet `
        -ArgumentList @("run", "--project", "control-plane/Everty.ControlPlane.csproj", "--urls", $BaseUrl) `
        -WorkingDirectory $root `
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

    $device = Invoke-ControlPlaneJson -Method "POST" -Path "/api/auth/device-login" -Body @{
        deviceLabel = "admin-cli-smoke-device"
        platform = "powershell"
    }

    $registeredHost = Invoke-ControlPlaneJson -Method "POST" -Path "/api/hosts/register" -Body @{
        displayName = "admin-cli-smoke-host"
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

    $relay = Invoke-ControlPlaneJson -Method "POST" -Path "/api/relay/register" -Body @{
        displayName = "admin-cli-smoke-relay"
        region = "local"
        publicAddress = "127.0.0.1"
        udpPort = 6293
        availability = 1
    }

    $session = Invoke-ControlPlaneJson -Method "POST" -Path "/api/sessions" -Token $device.accessToken -Body @{
        hostId = $registeredHost.hostId
        clientLabel = "admin-cli-smoke-client"
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

    $billingSummary = Invoke-AdminCli -Arguments @("-Command", "billing-summary", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey)
    if ($billingSummary.totalHolds -lt 1) {
        throw "admin CLI billing-summary failed: $($billingSummary | ConvertTo-Json -Compress)"
    }

    $billingProvider = Invoke-AdminCli -Arguments @("-Command", "billing-provider", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey)
    if ($billingProvider.provider -ne "manual" -or $billingProvider.configured -ne $true) {
        throw "admin CLI billing-provider failed: $($billingProvider | ConvertTo-Json -Compress)"
    }

    $billingAccounts = Invoke-AdminCli -Arguments @("-Command", "billing-accounts", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey)
    if (-not ($billingAccounts | Where-Object { $_.hostId -eq $registeredHost.hostId })) {
        throw "admin CLI billing-accounts failed: $($billingAccounts | ConvertTo-Json -Compress)"
    }

    $billingLedger = Invoke-AdminCli -Arguments @("-Command", "billing-ledger", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey, "-Limit", "10")
    if (-not ($billingLedger | Where-Object { $_.sessionId -eq $session.sessionId -and $_.kind -eq "hold_reserved" })) {
        throw "admin CLI billing-ledger failed: $($billingLedger | ConvertTo-Json -Compress)"
    }

    $billingReconciliation = Invoke-AdminCli -Arguments @("-Command", "billing-reconciliation", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey)
    [void]$billingReconciliation

    $summary = Invoke-AdminCli -Arguments @("-Command", "summary", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey)
    if ($summary.operatorAuthConfigured -ne $true) {
        throw "admin CLI summary failed: $($summary | ConvertTo-Json -Compress)"
    }

    $sessions = Invoke-AdminCli -Arguments @("-Command", "sessions", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey)
    if (-not ($sessions | Where-Object { $_.sessionId -eq $session.sessionId })) {
        throw "admin CLI sessions did not include created session."
    }

    $offer = Invoke-AdminCli -Arguments @("-Command", "set-offer", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey, "-HostId", $registeredHost.hostId, "-PricePerHour", "3.75", "-Currency", "EUR", "-Description", "admin cli smoke offer")
    if ($offer.hostId -ne $registeredHost.hostId -or $offer.pricePerHour -ne 3.75 -or $offer.currency -ne "EUR") {
        throw "admin CLI set-offer failed: $($offer | ConvertTo-Json -Compress)"
    }

    $stopped = Invoke-AdminCli -Arguments @("-Command", "stop-session", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey, "-SessionId", $session.sessionId, "-Reason", "admin_cli_smoke_stop")
    if ($stopped.status -ne 2 -or $stopped.stopReason -ne "admin_cli_smoke_stop") {
        throw "admin CLI stop-session failed: $($stopped | ConvertTo-Json -Compress)"
    }

    $settled = Invoke-AdminCli -Arguments @("-Command", "settle-billing", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey, "-SessionId", $session.sessionId, "-Reason", "admin_cli_smoke_settle")
    if ($settled.status -ne 3) {
        throw "admin CLI settle-billing failed: $($settled | ConvertTo-Json -Compress)"
    }

    $disabledHost = Invoke-AdminCli -Arguments @("-Command", "set-host", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey, "-HostId", $registeredHost.hostId, "-Availability", "Disabled", "-Reason", "admin_cli_smoke_disable")
    if ($disabledHost.availability -ne 3) {
        throw "admin CLI set-host failed: $($disabledHost | ConvertTo-Json -Compress)"
    }

    $disabledRelay = Invoke-AdminCli -Arguments @("-Command", "set-relay", "-BaseUrl", $BaseUrl, "-OperatorKey", $OperatorKey, "-RelayId", $relay.relayId, "-Availability", "Disabled", "-Reason", "admin_cli_smoke_disable")
    if ($disabledRelay.availability -ne 2) {
        throw "admin CLI set-relay failed: $($disabledRelay | ConvertTo-Json -Compress)"
    }

    [pscustomobject]@{
        adminCliSummary = "ok"
        adminCliBillingSummary = "ok"
        adminCliBillingProvider = "ok"
        adminCliBillingAccounts = "ok"
        adminCliBillingLedger = "ok"
        adminCliBillingReconciliation = "ok"
        adminCliSessions = "ok"
        adminCliSetOffer = "ok"
        adminCliStopSession = "ok"
        adminCliSettleBilling = "ok"
        adminCliSetHost = "ok"
        adminCliSetRelay = "ok"
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
