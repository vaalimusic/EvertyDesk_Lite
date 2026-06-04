param()

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $root
try {
    $statusLines = @(& git status --short 2>$null)
    if ($LASTEXITCODE -ne 0) {
        throw "git status failed"
    }

    function Get-Bucket {
        param([string]$Path)

        if ($Path -like "control-plane/*") { return "control-plane" }
        if ($Path -like "relay-node/*") { return "relay-node" }
        if ($Path -like "receiver-native/*") { return "receiver-native" }
        if ($Path -like "app/*") { return "android-app" }
        if ($Path -like "scripts/*") { return "scripts" }
        if ($Path -like "docs/*") { return "docs" }
        if ($Path -like "deploy/*") { return "deploy" }
        if ($Path -like "artifacts/*") { return "artifacts" }
        return "repo-root"
    }

    $entries = [System.Collections.Generic.List[object]]::new()
    foreach ($line in $statusLines) {
        if ([string]::IsNullOrWhiteSpace($line) -or $line.Length -lt 4) {
            continue
        }

        $status = $line.Substring(0, 2)
        $path = $line.Substring(3).Trim()
        $entries.Add([pscustomobject]@{
            status = $status
            path = $path
            bucket = Get-Bucket -Path $path
            untracked = $status -eq "??"
        })
    }

    $bucketSummary = $entries |
        Group-Object bucket |
        Sort-Object Name |
        ForEach-Object {
            [pscustomobject]@{
                bucket = $_.Name
                total = $_.Count
                tracked = @($_.Group | Where-Object { -not $_.untracked }).Count
                untracked = @($_.Group | Where-Object { $_.untracked }).Count
                samplePaths = @($_.Group | Select-Object -ExpandProperty path -First 4)
            }
        }

    $suggestedCommitOrder = @(
        "repo-root + deploy + docs + scripts",
        "control-plane + relay-node",
        "receiver-native + android-app"
    )

    [pscustomobject]@{
        status = "ok"
        totalChanges = $entries.Count
        trackedChanges = @($entries | Where-Object { -not $_.untracked }).Count
        untrackedChanges = @($entries | Where-Object { $_.untracked }).Count
        buckets = $bucketSummary
        suggestedCommitOrder = $suggestedCommitOrder
    } | ConvertTo-Json -Depth 6
} finally {
    Pop-Location
}
