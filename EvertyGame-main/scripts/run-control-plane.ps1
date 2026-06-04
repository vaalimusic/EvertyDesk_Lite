param(
    [string]$Url = "http://127.0.0.1:5180",
    [string]$StatePath = "",
    [string]$PublishDir = "artifacts/platform/control-plane"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$publishPath = Join-Path $root $PublishDir
$dllPath = Join-Path $publishPath "Everty.ControlPlane.dll"

if (-not (Test-Path $dllPath)) {
    throw "Published control-plane artifact was not found: $dllPath. Run scripts/publish-platform.ps1 first."
}

$env:ASPNETCORE_URLS = $Url
if ($StatePath) {
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $StatePath
}

dotnet $dllPath
