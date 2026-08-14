param(
    [switch]$SkipVideo
)

# Reproducible on-screen activity for the EVRTCK-vs-VNC wire-trace bench.
# Run this ON THE MACHINE BEING REMOTED INTO (the VNC/EvertyDesk Next host,
# e.g. 192.168.0.16) -- once while the observer side is capturing the VNC
# session, once while it's capturing the EvertyDesk Next session. Same
# script both times keeps the two traces comparable.
#
# Five phases, each written to the console so the capture side can line up
# timestamps against phase boundaries afterwards:
#   1. idle       -- static desktop, ~15s (near-zero-traffic baseline)
#   2. typing     -- Notepad, fixed passage typed at a human-ish pace
#   3. scrolling  -- steady scroll through a long text
#   4. window-move-- drag/resize a window around the screen
#   5. video      -- optional, needs a local video file (skip with -SkipVideo)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

function Phase([string]$name) {
    Write-Output "=== PHASE START: $name @ $(Get-Date -Format o) ==="
}

function PhaseEnd([string]$name) {
    Write-Output "=== PHASE END:   $name @ $(Get-Date -Format o) ==="
}

# ── Phase 1: idle ─────────────────────────────────────────────────────────
Phase "idle"
Start-Sleep -Seconds 15
PhaseEnd "idle"

# ── Phase 2: typing ───────────────────────────────────────────────────────
Phase "typing"
Start-Process notepad.exe
Start-Sleep -Milliseconds 800
[System.Windows.Forms.SendKeys]::SendWait("%{TAB}") # bring Notepad forward if needed
Start-Sleep -Milliseconds 300

$passage = @"
EvertyDesk Next benchmark scenario. This paragraph is typed at a fixed,
repeatable pace so the same on-screen activity happens for both the VNC
capture and the EvertyDesk Next capture. The quick brown fox jumps over
the lazy dog. Pack my box with five dozen liquor jugs. The five boxing
wizards jump quickly. How vexingly quick daft zebras jump!
"@

foreach ($ch in $passage.ToCharArray()) {
    $escaped = switch ($ch) {
        '{' { '{{}' } '}' { '{}}' } '+' { '{+}' } '^' { '{^}' } '%' { '{%}' } '~' { '{~}' } '(' { '{(}' } ')' { '{)}' }
        default { $ch }
    }
    [System.Windows.Forms.SendKeys]::SendWait($escaped)
    Start-Sleep -Milliseconds 45
}
PhaseEnd "typing"

# ── Phase 3: scrolling ────────────────────────────────────────────────────
Phase "scrolling"
[System.Windows.Forms.SendKeys]::SendWait("^{HOME}")
Start-Sleep -Milliseconds 300
for ($i = 0; $i -lt 40; $i++) {
    [System.Windows.Forms.SendKeys]::SendWait("{DOWN 3}")
    Start-Sleep -Milliseconds 150
}
PhaseEnd "scrolling"

# ── Phase 4: window move/resize ───────────────────────────────────────────
Phase "window-move"
$notepad = Get-Process notepad -ErrorAction SilentlyContinue | Select-Object -First 1
if ($notepad) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32 {
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int X, int Y, int nWidth, int nHeight, bool bRepaint);
}
"@
    $hwnd = $notepad.MainWindowHandle
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    for ($i = 0; $i -lt 20; $i++) {
        $x = Get-Random -Minimum 0 -Maximum ([Math]::Max(1, $screen.Width - 700))
        $y = Get-Random -Minimum 0 -Maximum ([Math]::Max(1, $screen.Height - 500))
        [Win32]::MoveWindow($hwnd, $x, $y, 700, 500, $true) | Out-Null
        Start-Sleep -Milliseconds 200
    }
}
PhaseEnd "window-move"

# ── Phase 5: video (optional) ─────────────────────────────────────────────
if (-not $SkipVideo) {
    Phase "video"
    Write-Output "Play ~15s of any local video now if you want this phase populated; sleeping 15s."
    Start-Sleep -Seconds 15
    PhaseEnd "video"
}

if ($notepad) {
    Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
}

Write-Output "=== SCENARIO COMPLETE @ $(Get-Date -Format o) ==="
