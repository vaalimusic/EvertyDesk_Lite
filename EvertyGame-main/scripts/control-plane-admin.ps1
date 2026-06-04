param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("summary", "billing-summary", "billing-provider", "billing-accounts", "billing-ledger", "billing-reconciliation", "sessions", "stop-session", "settle-billing", "retry-billing", "set-host", "set-relay", "set-offer")]
    [string]$Command,
    [string]$BaseUrl = "http://127.0.0.1:5180",
    [string]$OperatorKey = "",
    [string]$SessionId = "",
    [string]$HostId = "",
    [string]$RelayId = "",
    [string]$Availability = "",
    [decimal]$PricePerHour = 0,
    [string]$Currency = "USD",
    [string]$Description = "",
    [string]$Reason = "",
    [ValidateSet("auto", "capture", "settle")]
    [string]$RetryAction = "auto",
    [int]$Limit = 100,
    [switch]$StopActiveSession,
    [switch]$Unlisted
)

$ErrorActionPreference = "Stop"

if (-not $OperatorKey) {
    $OperatorKey = $env:EVERTY_CONTROL_PLANE_OPERATOR_KEY
}

if (-not $OperatorKey) {
    throw "Operator key is required. Pass -OperatorKey or set EVERTY_CONTROL_PLANE_OPERATOR_KEY."
}

$normalizedBaseUrl = $BaseUrl.Trim().TrimEnd("/")

function Invoke-AdminJson {
    param(
        [string]$Method,
        [string]$Path,
        [object]$Body = $null
    )

    $headers = @{
        "X-Everty-Operator-Key" = $OperatorKey
    }

    try {
        if ($null -eq $Body) {
            return Invoke-RestMethod -Method $Method -Uri "$normalizedBaseUrl$Path" -Headers $headers -TimeoutSec 10
        }

        return Invoke-RestMethod -Method $Method `
            -Uri "$normalizedBaseUrl$Path" `
            -Headers $headers `
            -Body ($Body | ConvertTo-Json -Depth 8) `
            -ContentType "application/json" `
            -TimeoutSec 10
    } catch {
        $responseBody = ""
        if ($_.Exception.Response) {
            $reader = [System.IO.StreamReader]::new($_.Exception.Response.GetResponseStream())
            $responseBody = $reader.ReadToEnd()
        }
        throw "$Method $Path failed: $($_.Exception.Message) $responseBody"
    }
}

$result = switch ($Command) {
    "summary" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/summary"
    }
    "billing-summary" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/billing/summary"
    }
    "billing-provider" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/billing/provider"
    }
    "billing-accounts" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/billing/accounts"
    }
    "billing-ledger" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/billing/ledger?limit=$Limit"
    }
    "billing-reconciliation" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/billing/reconciliation"
    }
    "sessions" {
        Invoke-AdminJson -Method "GET" -Path "/api/admin/sessions"
    }
    "stop-session" {
        if (-not $SessionId) {
            throw "-SessionId is required for stop-session."
        }
        Invoke-AdminJson -Method "POST" -Path "/api/admin/sessions/$SessionId/stop" -Body @{
            reason = if ($Reason) { $Reason } else { "operator_cli_stop" }
        }
    }
    "settle-billing" {
        if (-not $SessionId) {
            throw "-SessionId is required for settle-billing."
        }
        Invoke-AdminJson -Method "POST" -Path "/api/admin/billing/sessions/$SessionId/settle" -Body @{
            reason = if ($Reason) { $Reason } else { "operator_cli_settle" }
        }
    }
    "retry-billing" {
        if (-not $SessionId) {
            throw "-SessionId is required for retry-billing."
        }
        Invoke-AdminJson -Method "POST" -Path "/api/admin/billing/sessions/$SessionId/retry" -Body @{
            action = $RetryAction
            reason = if ($Reason) { $Reason } else { "operator_cli_retry" }
        }
    }
    "set-host" {
        if (-not $HostId) {
            throw "-HostId is required for set-host."
        }
        if (-not $Availability) {
            throw "-Availability is required for set-host."
        }
        Invoke-AdminJson -Method "POST" -Path "/api/admin/hosts/$HostId/availability" -Body @{
            availability = $Availability
            reason = if ($Reason) { $Reason } else { "operator_cli_host_availability" }
            stopActiveSession = $StopActiveSession.IsPresent
        }
    }
    "set-relay" {
        if (-not $RelayId) {
            throw "-RelayId is required for set-relay."
        }
        if (-not $Availability) {
            throw "-Availability is required for set-relay."
        }
        Invoke-AdminJson -Method "POST" -Path "/api/admin/relays/$RelayId/availability" -Body @{
            availability = $Availability
            reason = if ($Reason) { $Reason } else { "operator_cli_relay_availability" }
        }
    }
    "set-offer" {
        if (-not $HostId) {
            throw "-HostId is required for set-offer."
        }
        Invoke-AdminJson -Method "POST" -Path "/api/admin/hosts/$HostId/offer" -Body @{
            listed = -not $Unlisted.IsPresent
            pricePerHour = $PricePerHour
            currency = if ($Currency) { $Currency } else { "USD" }
            description = if ($Description) { $Description } else { $null }
        }
    }
}

$result | ConvertTo-Json -Depth 12
