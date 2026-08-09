param(
    [int]$CpuSampleSeconds = 10,
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"

if ($CpuSampleSeconds -lt 1) {
    throw "CpuSampleSeconds must be >= 1"
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $root "diagnostics\next-$timestamp"
}
$OutDir = (New-Item -ItemType Directory -Force -Path $OutDir).FullName

function Write-Json($Path, $Value) {
    $Value | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Path -Encoding utf8
}

$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os = Get-CimInstance Win32_OperatingSystem | Select-Object -First 1
$gpus = Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue |
    Select-Object Name, DriverVersion, AdapterRAM, VideoProcessor

$gitInfo = [ordered]@{
    available = $false
}
Push-Location $root
try {
    $commit = git rev-parse HEAD 2>$null
    if ($LASTEXITCODE -eq 0) {
        $gitInfo = [ordered]@{
            available = $true
            commit = ($commit -join "`n").Trim()
            branch = ((git rev-parse --abbrev-ref HEAD 2>$null) -join "`n").Trim()
            status = ((git status --short 2>$null) -join "`n")
        }
    }
}
finally {
    Pop-Location
}

Write-Json (Join-Path $OutDir "system.json") ([ordered]@{
    generated_at = (Get-Date).ToString("o")
    cpu_sample_seconds = $CpuSampleSeconds
    cpu = [ordered]@{
        name = $cpu.Name
        manufacturer = $cpu.Manufacturer
        cores = $cpu.NumberOfCores
        logical_processors = $cpu.NumberOfLogicalProcessors
        max_clock_mhz = $cpu.MaxClockSpeed
    }
    os = [ordered]@{
        caption = $os.Caption
        version = $os.Version
        build_number = $os.BuildNumber
        architecture = $os.OSArchitecture
    }
    gpus = $gpus
    git = $gitInfo
})

Get-Process -ErrorAction SilentlyContinue |
    Sort-Object CPU -Descending |
    Select-Object -First 80 ProcessName, Id, CPU, WorkingSet64, Path |
    Export-Csv -LiteralPath (Join-Path $OutDir "processes-top-cumulative.csv") -NoTypeInformation -Encoding utf8

& (Join-Path $PSScriptRoot "diagnose-vm-cpu.ps1") -Seconds $CpuSampleSeconds |
    Export-Csv -LiteralPath (Join-Path $OutDir "vm-cpu-sample.csv") -NoTypeInformation -Encoding utf8

$logCandidates = @()
if ($env:LOCALAPPDATA) {
    $logCandidates += Join-Path $env:LOCALAPPDATA "EvertyDesk\desktop-next.log"
    $logCandidates += Join-Path $env:LOCALAPPDATA "EvertyDesk\desktop-next.log.old"
}
$logCandidates += Join-Path $root "desktop-next\evertydesk_host_log.txt"
$logCandidates += Join-Path $root "evertydesk_host_log.txt"
$logDir = New-Item -ItemType Directory -Force -Path (Join-Path $OutDir "logs")
foreach ($log in $logCandidates) {
    if (Test-Path -LiteralPath $log) {
        Copy-Item -LiteralPath $log -Destination (Join-Path $logDir.FullName (Split-Path -Leaf $log)) -Force
    }
}

$vmware = [ordered]@{
    vmrun_in_path = $false
    common_paths = @()
    running_processes = @()
}
$vmrun = Get-Command vmrun -ErrorAction SilentlyContinue
if ($null -ne $vmrun) {
    $vmware.vmrun_in_path = $true
    $vmware.vmrun_path = $vmrun.Source
}
foreach ($path in @(
    "C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe",
    "C:\Program Files\VMware\VMware Workstation\vmrun.exe",
    "C:\Program Files (x86)\VMware\VMware Player\vmrun.exe"
)) {
    if (Test-Path -LiteralPath $path) {
        $vmware.common_paths += $path
    }
}
$vmware.running_processes = @(Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -like "vmware*" -or $_.ProcessName -eq "vmrun" } |
    Select-Object ProcessName, Id, CPU, WorkingSet64, Path)
Write-Json (Join-Path $OutDir "vmware.json") $vmware

$zip = "$OutDir.zip"
if (Test-Path -LiteralPath $zip) {
    Remove-Item -LiteralPath $zip -Force
}
Compress-Archive -Path (Join-Path $OutDir "*") -DestinationPath $zip -CompressionLevel Optimal

[pscustomobject]@{
    DiagnosticsDirectory = $OutDir
    Zip = $zip
}
