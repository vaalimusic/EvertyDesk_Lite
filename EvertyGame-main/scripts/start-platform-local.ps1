param(
    [string]$Url = "http://127.0.0.1:5180",
    [int]$RelayUdpPort = 6200,
    [string]$RelayPublicAddress = "127.0.0.1",
    [string]$RelayDisplayName = "Local Relay",
    [string]$RelayRegion = "local",
    [string]$OutputRoot = "artifacts/platform",
    [string]$StatePath = "",
    [switch]$PublishFirst
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$outputRootPath = Join-Path $root $OutputRoot
$controlDll = Join-Path $outputRootPath "control-plane/Everty.ControlPlane.dll"
$relayDll = Join-Path $outputRootPath "relay-node/Everty.RelayNode.dll"
$runDir = Join-Path $outputRootPath "run"
$logDir = Join-Path $outputRootPath "logs"

if ($PublishFirst) {
    & (Join-Path $PSScriptRoot "publish-platform.ps1") -OutputRoot $OutputRoot
}

if (-not (Test-Path $controlDll)) {
    throw "Published control-plane artifact was not found: $controlDll. Run scripts/publish-platform.ps1 first or pass -PublishFirst."
}

if (-not (Test-Path $relayDll)) {
    throw "Published relay-node artifact was not found: $relayDll. Run scripts/publish-platform.ps1 first or pass -PublishFirst."
}

New-Item -ItemType Directory -Force -Path $runDir | Out-Null
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

if (-not $StatePath) {
    $dataDir = Join-Path $outputRootPath "data"
    New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
    $StatePath = Join-Path $dataDir "state.json"
}

$controlPidPath = Join-Path $runDir "control-plane.pid"
$relayPidPath = Join-Path $runDir "relay-node.pid"

foreach ($pidPath in @($controlPidPath, $relayPidPath)) {
    if (Test-Path $pidPath) {
        $pidValue = Get-Content $pidPath -Raw
        $pidValue = $pidValue.Trim()
        if ($pidValue -and (Get-Process -Id ([int]$pidValue) -ErrorAction SilentlyContinue)) {
            throw "Service pid file already points to a running process: $pidPath -> $pidValue. Run scripts/stop-platform-local.ps1 first."
        }
        Remove-Item -LiteralPath $pidPath -Force
    }
}

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
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $StatePath

    $controlProcess = Start-Process -FilePath "dotnet" `
        -ArgumentList @("""$controlDll""") `
        -RedirectStandardOutput (Join-Path $logDir "control-plane.out.log") `
        -RedirectStandardError (Join-Path $logDir "control-plane.err.log") `
        -PassThru `
        -WindowStyle Hidden

    Set-Content -Encoding ASCII -Path $controlPidPath -Value $controlProcess.Id

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
        throw "Control-plane did not become ready at $Url."
    }

    $env:EVERTY_RELAY_CONTROL_PLANE = $Url
    $env:EVERTY_RELAY_UDP_PORT = $RelayUdpPort.ToString()
    $env:EVERTY_RELAY_PUBLIC_ADDRESS = $RelayPublicAddress
    $env:EVERTY_RELAY_DISPLAY_NAME = $RelayDisplayName
    $env:EVERTY_RELAY_REGION = $RelayRegion

    $relayProcess = Start-Process -FilePath "dotnet" `
        -ArgumentList @("""$relayDll""") `
        -RedirectStandardOutput (Join-Path $logDir "relay-node.out.log") `
        -RedirectStandardError (Join-Path $logDir "relay-node.err.log") `
        -PassThru `
        -WindowStyle Hidden

    Set-Content -Encoding ASCII -Path $relayPidPath -Value $relayProcess.Id

    Start-Sleep -Seconds 2
    if ($relayProcess.HasExited) {
        throw "Relay-node exited during startup."
    }

    [pscustomobject]@{
        controlPlaneUrl = $Url
        controlPlanePid = $controlProcess.Id
        relayUdpPort = $RelayUdpPort
        relayPid = $relayProcess.Id
        statePath = $StatePath
        logDir = $logDir
        runDir = $runDir
    } | ConvertTo-Json -Compress
} catch {
    if ($relayProcess -ne $null -and -not $relayProcess.HasExited) {
        Stop-Process -Id $relayProcess.Id -Force
    }
    if ($controlProcess -ne $null -and -not $controlProcess.HasExited) {
        Stop-Process -Id $controlProcess.Id -Force
    }
    throw
} finally {
    $env:ASPNETCORE_URLS = $previousAspnetUrls
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $previousStatePath
    $env:EVERTY_RELAY_CONTROL_PLANE = $previousRelayControlPlane
    $env:EVERTY_RELAY_UDP_PORT = $previousRelayUdpPort
    $env:EVERTY_RELAY_PUBLIC_ADDRESS = $previousRelayPublicAddress
    $env:EVERTY_RELAY_DISPLAY_NAME = $previousRelayDisplayName
    $env:EVERTY_RELAY_REGION = $previousRelayRegion
}
