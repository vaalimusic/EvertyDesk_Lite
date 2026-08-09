param(
    [int]$Seconds = 10
)

$ErrorActionPreference = "Stop"

if ($Seconds -lt 1) {
    throw "Seconds must be >= 1"
}

$processNames = @(
    "evertydesk-launcher",
    "evertydesk-viewer",
    "evertydesk-rdp-viewer",
    "evertydesk-lite",
    "vmware-vmx",
    "vmware",
    "vmrun",
    "vmware-tray",
    "vmware-authd",
    "vmware-hostd",
    "VirtualBoxVM",
    "VBoxHeadless",
    "VBoxManage",
    "vmconnect",
    "mstsc"
)

$logicalProcessors = (Get-CimInstance Win32_Processor |
    Measure-Object -Property NumberOfLogicalProcessors -Sum).Sum
$logicalProcessors = [Math]::Max(1, [int]$logicalProcessors)

$before = Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $processNames -contains $_.ProcessName } |
    Select-Object Id, ProcessName, CPU, WorkingSet64, Path

Start-Sleep -Seconds $Seconds

$rows = foreach ($sample in $before) {
    $after = Get-Process -Id $sample.Id -ErrorAction SilentlyContinue
    if ($null -eq $after) {
        continue
    }
    $cpuDeltaSeconds = [double]($after.CPU - $sample.CPU)
    [pscustomobject]@{
        ProcessName = $sample.ProcessName
        Id = $sample.Id
        CpuPercent = [Math]::Round(($cpuDeltaSeconds / $Seconds / $logicalProcessors) * 100.0, 2)
        WorkingSetMB = [Math]::Round($after.WorkingSet64 / 1MB, 1)
        Path = $sample.Path
    }
}

$rows | Sort-Object CpuPercent -Descending
