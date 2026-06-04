param(
    [string]$BaseUrl = "http://127.0.0.1:5180"
)

$ErrorActionPreference = "Stop"

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

    $uri = "$BaseUrl$Path"
    if ($null -eq $Body) {
        return Invoke-RestMethod -Method $Method -Uri $uri -Headers $headers -TimeoutSec 5
    }

    $json = $Body | ConvertTo-Json -Depth 8
    return Invoke-RestMethod -Method $Method -Uri $uri -Headers $headers -Body $json -ContentType "application/json" -TimeoutSec 5
}

$device = Invoke-ControlPlaneJson -Method "POST" -Path "/api/auth/device-login" -Body @{
    deviceLabel = "route-policy-smoke"
    platform = "powershell"
}

$hostRegistration = Invoke-ControlPlaneJson -Method "POST" -Path "/api/hosts/register" -Body @{
    displayName = "route-policy-smoke-host"
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

$session = Invoke-ControlPlaneJson -Method "POST" -Path "/api/sessions" -Token $device.accessToken -Body @{
    hostId = $hostRegistration.hostId
    clientLabel = "route-policy-smoke-client"
    clientRegion = "local"
    codecPreference = "hevc"
    preferRelay = $false
    audioRequested = $false
    controllerCount = 1
    leaseMinutes = 5
    receiverAddress = "127.0.0.1"
    receiverPort = 5001
    requestedWidth = 1280
    requestedHeight = 720
    requestedFps = 60
    requestedBitrateBps = 15000000
    captureCursor = $false
    adaptiveMode = $false
}

$policy = Invoke-ControlPlaneJson -Method "GET" -Path "/api/sessions/$($session.sessionId)/route/policy?sessionToken=$([uri]::EscapeDataString($session.sessionToken))" -Token $device.accessToken

if (-not $policy.sessionId -or $policy.sessionId -ne $session.sessionId) {
    throw "Route policy session id mismatch."
}

foreach ($field in @("routeActionHint", "transportAnomalyKind", "transportAnomalyConfidence", "fallbackWarmupSeconds", "recoveryWarmupSeconds")) {
    if ($null -eq $policy.$field) {
        throw "Route policy response is missing '$field'."
    }
}

Invoke-ControlPlaneJson -Method "POST" -Path "/api/sessions/$($session.sessionId)/stop" -Token $device.accessToken -Body @{
    sessionToken = $session.sessionToken
    reason = "route_policy_smoke_done"
} | Out-Null

[pscustomobject]@{
    sessionId = $policy.sessionId
    routeKind = $policy.routeKind
    routeActionHint = $policy.routeActionHint
    transportAnomalyKind = $policy.transportAnomalyKind
    transportAnomalyConfidence = $policy.transportAnomalyConfidence
    fallbackWarmupSeconds = $policy.fallbackWarmupSeconds
    recoveryWarmupSeconds = $policy.recoveryWarmupSeconds
} | ConvertTo-Json -Compress
