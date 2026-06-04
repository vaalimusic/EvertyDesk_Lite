param(
    [string]$ControlPlaneUrl = "http://46.45.217.19:5180",
    [switch]$Advanced
)

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$receiverNativeProject = Join-Path $projectRoot "receiver-native\ReceiverNative.csproj"
$receiverNativeExe = Join-Path $projectRoot "receiver-native\bin\Debug\net8.0-windows\ReceiverNative.exe"

dotnet build $receiverNativeProject | Out-Host

$args = @(
    "--role", "host",
    "--control-plane-url", $ControlPlaneUrl,
    "--lock-role"
)

if ($Advanced.IsPresent)
{
    $args += "--advanced"
}

Start-Process -FilePath $receiverNativeExe -ArgumentList $args | Out-Null
