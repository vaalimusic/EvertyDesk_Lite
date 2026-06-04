param(
    [string]$Url = "http://192.168.0.5:5180",
    [int]$RelayUdpPort = 6200,
    [string]$RelayPublicAddress = "46.45.217.19",
    [string]$RelayDisplayName = "Local Relay",
    [string]$RelayRegion = "local",
    [string]$OutputRoot = "artifacts/platform",
    [switch]$CleanState,
    [switch]$CleanLogs,
    [switch]$SkipDesktopApp,
    [switch]$UseLegacyReceiverNative,
    [switch]$NoPublish
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$outputRootPath = Join-Path $root $OutputRoot
$runDir = Join-Path $outputRootPath "run"
$logDir = Join-Path $outputRootPath "logs"
$dataDir = Join-Path $outputRootPath "data"
$statePath = Join-Path $dataDir "state.json"
$receiverProject = Join-Path $root "receiver-native\ReceiverNative.csproj"
$receiverExe = Join-Path $root "receiver-native\bin\Debug\net8.0-windows\ReceiverNative.exe"
$avaloniaProject = Join-Path $root "desktop-avalonia\Everty.Desktop.Avalonia.csproj"
$avaloniaExe = Join-Path $root "desktop-avalonia\bin\Debug\net10.0-windows10.0.19041.0\Everty.Desktop.Avalonia.exe"
$receiverLocalLog = Join-Path $env:LOCALAPPDATA "EvertyNativeReceiver\receiver-native.log"
$controlPlaneDebugLog = Join-Path $env:LOCALAPPDATA "EvertyControlPlane\control-plane-debug.log"

function Remove-IfExists {
    param([string]$Path)

    if (Test-Path $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
}

function Stop-ReceiverNativeProcess {
    $targets = @(
        "Everty.Desktop.Avalonia",
        "ReceiverNative",
        "ReceiverNew"
    )

    foreach ($name in $targets) {
        Get-Process -Name $name -ErrorAction SilentlyContinue | Stop-Process -Force
    }
}

function Start-ReceiverNativeProcess {
    if (-not (Test-Path $receiverExe)) {
        throw "ReceiverNative.exe not found: $receiverExe"
    }

    Start-Process -FilePath $receiverExe | Out-Null
}

function Start-AvaloniaDesktopProcess {
    if (-not (Test-Path $avaloniaExe)) {
        throw "Everty.Desktop.Avalonia.exe not found: $avaloniaExe"
    }

    Start-Process -FilePath $avaloniaExe | Out-Null
}

Write-Host "Stopping local platform..."
& (Join-Path $PSScriptRoot "stop-platform-local.ps1") -OutputRoot $OutputRoot | Out-Null
Stop-ReceiverNativeProcess

if ($CleanState) {
    Write-Host "Cleaning state..."
    Remove-IfExists -Path $statePath
}

if ($CleanLogs) {
    Write-Host "Cleaning logs..."
    Remove-IfExists -Path $logDir
    Remove-IfExists -Path $receiverLocalLog
    Remove-IfExists -Path $controlPlaneDebugLog
}

Write-Host "Building ReceiverNative..."
$receiverBuildOutput = & dotnet build $receiverProject 2>&1
foreach ($line in $receiverBuildOutput) {
    Write-Host $line
}
if ($LASTEXITCODE -ne 0) {
    throw "dotnet build failed for receiver-native with exit code $LASTEXITCODE"
}

Write-Host "Building Avalonia desktop..."
$avaloniaBuildOutput = & dotnet build $avaloniaProject 2>&1
foreach ($line in $avaloniaBuildOutput) {
    Write-Host $line
}
if ($LASTEXITCODE -ne 0) {
    throw "dotnet build failed for desktop-avalonia with exit code $LASTEXITCODE"
}

if (-not $NoPublish) {
    Write-Host "Publishing platform..."
    & (Join-Path $PSScriptRoot "publish-platform.ps1") -OutputRoot $OutputRoot | Out-Null
}

Write-Host "Starting local platform..."
$platformJson = & (Join-Path $PSScriptRoot "start-platform-local.ps1") `
    -Url $Url `
    -RelayUdpPort $RelayUdpPort `
    -RelayPublicAddress $RelayPublicAddress `
    -RelayDisplayName $RelayDisplayName `
    -RelayRegion $RelayRegion `
    -OutputRoot $OutputRoot

$platform = $platformJson | ConvertFrom-Json

if (-not $SkipDesktopApp) {
    if ($UseLegacyReceiverNative) {
        Write-Host "Starting ReceiverNative..."
        Start-ReceiverNativeProcess
    }
    else {
        Write-Host "Starting Everty.Desktop.Avalonia..."
        Start-AvaloniaDesktopProcess
    }
}

[pscustomobject]@{
    url = $Url
    relayPublicAddress = $RelayPublicAddress
    relayUdpPort = $RelayUdpPort
    outputRoot = $outputRootPath
    runDir = $runDir
    logDir = $logDir
    statePath = $statePath
    receiverExe = $receiverExe
    avaloniaExe = $avaloniaExe
    receiverLog = $receiverLocalLog
    controlPlaneDebugLog = $controlPlaneDebugLog
    controlPlanePid = $platform.controlPlanePid
    relayPid = $platform.relayPid
    desktopAppStarted = (-not $SkipDesktopApp)
    desktopApp = if ($UseLegacyReceiverNative) { "ReceiverNative" } else { "Everty.Desktop.Avalonia" }
} | ConvertTo-Json -Compress
