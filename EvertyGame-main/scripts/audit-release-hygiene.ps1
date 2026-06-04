param(
    [switch]$RequireCleanGit
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $root
try {
    $requiredFiles = @(
        "docker-compose.yml",
        "deploy/control-plane.env.example",
        "deploy/relay-node.env.example",
        "deploy/docker-compose.env.example",
        "docs/deployment.md",
        "docs/release-readiness.md",
        "scripts/publish-platform.ps1",
        "scripts/smoke-product-readiness.ps1",
        "scripts/smoke-published-platform.ps1"
    )

    $missing = @()
    foreach ($path in $requiredFiles) {
        if (-not (Test-Path -LiteralPath $path)) {
            $missing += $path
        }
    }

    $publishManifestIgnored = $false
    & git check-ignore artifacts/platform/publish-manifest.json *> $null
    if ($LASTEXITCODE -eq 0) {
        $publishManifestIgnored = $true
    }

    $artifactsIgnored = $false
    & git check-ignore artifacts *> $null
    if ($LASTEXITCODE -eq 0) {
        $artifactsIgnored = $true
    }

    $gitStatusOutput = @(& git status --short 2>$null)
    $gitAvailable = $LASTEXITCODE -eq 0
    $gitDirty = $gitAvailable -and $gitStatusOutput.Count -gt 0

    if ($RequireCleanGit -and $gitDirty) {
        throw "git worktree is dirty"
    }

    $trackedCount = 0
    $untrackedCount = 0
    foreach ($line in $gitStatusOutput) {
        if ($line.StartsWith("??")) {
            $untrackedCount++
        } else {
            $trackedCount++
        }
    }

    $result = [pscustomobject]@{
        status = if ($missing.Count -eq 0 -and $publishManifestIgnored -and $artifactsIgnored) { "ok" } else { "failed" }
        requiredFilesPresent = ($missing.Count -eq 0)
        missingFiles = $missing
        artifactsIgnored = $artifactsIgnored
        publishManifestIgnored = $publishManifestIgnored
        gitAvailable = $gitAvailable
        gitDirty = $gitDirty
        trackedChangeCount = $trackedCount
        untrackedChangeCount = $untrackedCount
    }

    if ($result.status -ne "ok") {
        throw ($result | ConvertTo-Json -Depth 4 -Compress)
    }

    $result | ConvertTo-Json -Depth 4
} finally {
    Pop-Location
}
