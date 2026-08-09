param(
    [string]$Configuration = "debug",
    [string]$BinaryDirectory = "",
    [switch]$StopAllExistingLaunchers,
    [switch]$LeaveRunning
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$targetDir = if ([string]::IsNullOrWhiteSpace($BinaryDirectory)) {
    Join-Path $root "target\$Configuration"
} else {
    (Resolve-Path -LiteralPath $BinaryDirectory).Path
}
$launcher = Join-Path $targetDir "evertydesk-launcher.exe"
$viewer = Join-Path $targetDir "evertydesk-viewer.exe"
$rdpViewer = Join-Path $targetDir "evertydesk-rdp-viewer.exe"

if (!(Test-Path -LiteralPath $launcher)) {
    throw "Launcher exe not found: $launcher"
}
if (!(Test-Path -LiteralPath $viewer)) {
    throw "Viewer exe not found: $viewer"
}
if (!(Test-Path -LiteralPath $rdpViewer)) {
    throw "RDP viewer exe not found: $rdpViewer"
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
    Where-Object { $StopAllExistingLaunchers -or $_.Path -eq $launcher }
foreach ($process in $existing) {
    Stop-Process -Id $process.Id -Force
}

$workingDirectory = if ([string]::IsNullOrWhiteSpace($BinaryDirectory)) { $root } else { $targetDir }
$process = Start-Process -FilePath $launcher -WorkingDirectory $workingDirectory -PassThru
try {
    $deadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        if ($process.HasExited) {
            if ($process.ExitCode -eq 0) {
                $otherLaunchers = Get-Process evertydesk-launcher -ErrorAction SilentlyContinue |
                    Where-Object { $_.Path -ne $launcher }
                if ($otherLaunchers) {
                    $paths = ($otherLaunchers | Select-Object -ExpandProperty Path -Unique) -join "; "
                    throw "Launcher exited cleanly because another EvertyDesk instance is already primary: $paths. Re-run with -StopAllExistingLaunchers to isolate this build."
                }
            }
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

    $samePathProcesses = Get-CimInstance Win32_Process -Filter "name='evertydesk-launcher.exe'" |
        Where-Object { $_.ExecutablePath -eq $launcher }
    $primaryProcesses = @($samePathProcesses | Where-Object { $_.CommandLine -notmatch ' --host-agent(\s|$)' })
    $hostAgents = @($samePathProcesses | Where-Object { $_.CommandLine -match ' --host-agent(\s|$)' })

    if ($primaryProcesses.Count -ne 1) {
        throw "Expected exactly one primary launcher for this build, got $($primaryProcesses.Count)"
    }

    $logPath = Join-Path $env:LOCALAPPDATA "EvertyDesk\desktop-next.log"
    [pscustomobject]@{
        Launcher      = $launcher
        Viewer        = $viewer
        RdpViewer     = $rdpViewer
        WorkingDir    = $workingDirectory
        ProcessId     = $process.Id
        WindowTitle   = $process.MainWindowTitle
        Responding    = $process.Responding
        HostAgents    = $hostAgents.Count
        Icon          = "$($icon.Width)x$($icon.Height)"
        DiagnosticLog = $logPath
    }
}
finally {
    if (!$LeaveRunning) {
        Get-Process evertydesk-launcher -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -eq $launcher } |
            Stop-Process -Force
    }
}
