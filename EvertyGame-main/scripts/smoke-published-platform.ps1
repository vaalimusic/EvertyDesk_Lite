param(
    [string]$Url = "http://127.0.0.1:5195",
    [int]$RelayUdpPort = 6297,
    [string]$OutputRoot = "artifacts/platform"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$controlDll = Join-Path (Join-Path $root $OutputRoot) "control-plane/Everty.ControlPlane.dll"
$relayDll = Join-Path (Join-Path $root $OutputRoot) "relay-node/Everty.RelayNode.dll"

if (-not (Test-Path $controlDll)) {
    throw "Published control-plane artifact was not found: $controlDll. Run scripts/publish-platform.ps1 first."
}

if (-not (Test-Path $relayDll)) {
    throw "Published relay-node artifact was not found: $relayDll. Run scripts/publish-platform.ps1 first."
}

$stateDir = Join-Path $env:TEMP ("everty-published-smoke-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null

$controlOut = Join-Path $stateDir "control-plane.out.log"
$controlErr = Join-Path $stateDir "control-plane.err.log"
$relayOut = Join-Path $stateDir "relay-node.out.log"
$relayErr = Join-Path $stateDir "relay-node.err.log"
$statePath = Join-Path $stateDir "state.json"

$previousAspnetUrls = $env:ASPNETCORE_URLS
$previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
$previousRelayControlPlane = $env:EVERTY_RELAY_CONTROL_PLANE
$previousRelayUdpPort = $env:EVERTY_RELAY_UDP_PORT
$previousRelayPublicAddress = $env:EVERTY_RELAY_PUBLIC_ADDRESS
$previousRelayDisplayName = $env:EVERTY_RELAY_DISPLAY_NAME
$previousRelayRegion = $env:EVERTY_RELAY_REGION

$controlProcess = $null
$relayProcess = $null

try {
    $env:ASPNETCORE_URLS = $Url
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath

    $controlProcess = Start-Process -FilePath "dotnet" `
        -ArgumentList @("""$controlDll""") `
        -RedirectStandardOutput $controlOut `
        -RedirectStandardError $controlErr `
        -PassThru `
        -WindowStyle Hidden

    $ready = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        Start-Sleep -Milliseconds 500

        if ($controlProcess.HasExited) {
            break
        }

        try {
            $readyResponse = Invoke-RestMethod -Uri "$Url/api/ready" -TimeoutSec 2
            if ($readyResponse.ready -eq $true) {
                $ready = $true
                break
            }
        } catch {
        }
    }

    if (-not $ready) {
        $stdout = if (Test-Path $controlOut) { Get-Content $controlOut -Raw } else { "" }
        $stderr = if (Test-Path $controlErr) { Get-Content $controlErr -Raw } else { "" }
        throw "Published control-plane did not become ready. stdout=$stdout stderr=$stderr"
    }

    $env:EVERTY_RELAY_CONTROL_PLANE = $Url
    $env:EVERTY_RELAY_UDP_PORT = $RelayUdpPort.ToString()
    $env:EVERTY_RELAY_PUBLIC_ADDRESS = "127.0.0.1"
    $env:EVERTY_RELAY_DISPLAY_NAME = "PublishedSmokeRelay"
    $env:EVERTY_RELAY_REGION = "local"

    $relayProcess = Start-Process -FilePath "dotnet" `
        -ArgumentList @("""$relayDll""") `
        -RedirectStandardOutput $relayOut `
        -RedirectStandardError $relayErr `
        -PassThru `
        -WindowStyle Hidden

    Start-Sleep -Seconds 3

    if ($relayProcess.HasExited) {
        $stdout = if (Test-Path $relayOut) { Get-Content $relayOut -Raw } else { "" }
        $stderr = if (Test-Path $relayErr) { Get-Content $relayErr -Raw } else { "" }
        throw "Published relay-node exited early. stdout=$stdout stderr=$stderr"
    }

    [pscustomobject]@{
        controlPlaneReady = $ready
        relayStillRunning = -not $relayProcess.HasExited
        relayUdpPort = $RelayUdpPort
        stateDir = $stateDir
    } | ConvertTo-Json -Compress
} finally {
    if ($relayProcess -ne $null -and -not $relayProcess.HasExited) {
        Stop-Process -Id $relayProcess.Id -Force
        $relayProcess.WaitForExit()
    }

    if ($controlProcess -ne $null -and -not $controlProcess.HasExited) {
        Stop-Process -Id $controlProcess.Id -Force
        $controlProcess.WaitForExit()
    }

    $env:ASPNETCORE_URLS = $previousAspnetUrls
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $previousStatePath
    $env:EVERTY_RELAY_CONTROL_PLANE = $previousRelayControlPlane
    $env:EVERTY_RELAY_UDP_PORT = $previousRelayUdpPort
    $env:EVERTY_RELAY_PUBLIC_ADDRESS = $previousRelayPublicAddress
    $env:EVERTY_RELAY_DISPLAY_NAME = $previousRelayDisplayName
    $env:EVERTY_RELAY_REGION = $previousRelayRegion
}
