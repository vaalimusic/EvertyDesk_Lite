param(
    [string]$ControlPlane = "http://127.0.0.1:5180",
    [int]$UdpPort = 6200,
    [string]$PublicAddress = "127.0.0.1",
    [string]$DisplayName = "Local Relay",
    [string]$Region = "global",
    [string]$PublishDir = "artifacts/platform/relay-node"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$publishPath = Join-Path $root $PublishDir
$dllPath = Join-Path $publishPath "Everty.RelayNode.dll"

if (-not (Test-Path $dllPath)) {
    throw "Published relay-node artifact was not found: $dllPath. Run scripts/publish-platform.ps1 first."
}

$env:EVERTY_RELAY_CONTROL_PLANE = $ControlPlane
$env:EVERTY_RELAY_UDP_PORT = $UdpPort.ToString()
$env:EVERTY_RELAY_PUBLIC_ADDRESS = $PublicAddress
$env:EVERTY_RELAY_DISPLAY_NAME = $DisplayName
$env:EVERTY_RELAY_REGION = $Region

dotnet $dllPath
