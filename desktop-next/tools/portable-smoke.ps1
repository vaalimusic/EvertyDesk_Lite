param(
    [string]$Configuration = "debug"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$targetDir = Join-Path $root "target\$Configuration"
$launcher = Join-Path $targetDir "evertydesk-launcher.exe"
$viewer = Join-Path $targetDir "evertydesk-viewer.exe"

if (!(Test-Path -LiteralPath $launcher)) {
    throw "Launcher exe not found: $launcher"
}
if (!(Test-Path -LiteralPath $viewer)) {
    throw "Viewer exe not found: $viewer"
}

Add-Type -AssemblyName System.Drawing
$icon = [System.Drawing.Icon]::ExtractAssociatedIcon($launcher)
if ($null -eq $icon) {
    throw "Launcher exe has no associated icon resource"
}
if ($icon.Width -lt 16 -or $icon.Height -lt 16) {
    throw "Launcher icon is too small: $($icon.Width)x$($icon.Height)"
}

$existing = Get-Process evertydesk-launcher -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $launcher }
foreach ($process in $existing) {
    Stop-Process -Id $process.Id -Force
}

$process = Start-Process -FilePath $launcher -WorkingDirectory $root -PassThru
$deadline = (Get-Date).AddSeconds(30)
do {
    Start-Sleep -Milliseconds 500
    $process.Refresh()
    if ($process.HasExited) {
        throw "Launcher exited during startup with code $($process.ExitCode)"
    }
    if ($process.Responding -and $process.MainWindowTitle) {
        break
    }
} while ((Get-Date) -lt $deadline)

if (!$process.Responding) {
    $logPath = Join-Path $env:LOCALAPPDATA "EvertyDesk\desktop-next.log"
    throw "Launcher process is not responding after startup timeout. Check log: $logPath"
}

$logPath = Join-Path $env:LOCALAPPDATA "EvertyDesk\desktop-next.log"
[pscustomobject]@{
    Launcher     = $launcher
    Viewer       = $viewer
    ProcessId    = $process.Id
    WindowTitle  = $process.MainWindowTitle
    Responding   = $process.Responding
    Icon         = "$($icon.Width)x$($icon.Height)"
    DiagnosticLog = $logPath
}
