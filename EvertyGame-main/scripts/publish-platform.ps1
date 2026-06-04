param(
    [string]$Configuration = "Release",
    [string]$OutputRoot = "artifacts/platform",
    [string]$Runtime = "",
    [switch]$SelfContained,
    [string]$Version = "",
    [string]$Channel = "local"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$outputRootPath = Join-Path $root $OutputRoot
New-Item -ItemType Directory -Force -Path $outputRootPath | Out-Null

function Get-GitValue {
    param([string[]]$Arguments)

    $value = & git -C $root @Arguments 2>$null
    if ($LASTEXITCODE -ne 0) {
        return ""
    }

    return ($value | Select-Object -First 1)
}

$gitCommit = Get-GitValue -Arguments @("rev-parse", "HEAD")
$gitShortCommit = Get-GitValue -Arguments @("rev-parse", "--short", "HEAD")
$gitStatus = & git -C $root status --porcelain 2>$null
$gitDirty = $LASTEXITCODE -eq 0 -and $null -ne $gitStatus
$buildVersion = if ($Version) { $Version } elseif ($env:EVERTY_BUILD_VERSION) { $env:EVERTY_BUILD_VERSION } else { "0.1.0-local" }

function Publish-Project {
    param(
        [string]$Name,
        [string]$Project,
        [string]$MainAssembly
    )

    $output = Join-Path $outputRootPath $Name
    if (Test-Path $output) {
        Remove-Item -LiteralPath $output -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $output | Out-Null

    $args = @("publish", (Join-Path $root $Project), "-c", $Configuration, "-o", $output)
    if ($Runtime) {
        $args += @("-r", $Runtime)
        $args += @("--self-contained", $SelfContained.IsPresent.ToString().ToLowerInvariant())
    }

    $publishOutput = & dotnet @args --nologo 2>&1
    foreach ($line in $publishOutput) {
        Write-Host $line
    }

    if ($LASTEXITCODE -ne 0) {
        throw "dotnet publish failed for $Name with exit code $LASTEXITCODE"
    }

    [pscustomobject]@{
        name = $Name
        project = $Project
        output = $output
        mainDll = Join-Path $output $MainAssembly
        fileCount = (Get-ChildItem -LiteralPath $output -File | Measure-Object).Count
    }
}

$published = @(
    Publish-Project -Name "control-plane" -Project "control-plane/Everty.ControlPlane.csproj" -MainAssembly "Everty.ControlPlane.dll"
    Publish-Project -Name "relay-node" -Project "relay-node/Everty.RelayNode.csproj" -MainAssembly "Everty.RelayNode.dll"
)

$manifest = [pscustomobject]@{
    schema = "everty.platform.publish.v1"
    createdUtc = [DateTimeOffset]::UtcNow.ToString("O")
    outputRoot = $outputRootPath
    configuration = $Configuration
    runtime = if ($Runtime) { $Runtime } else { "portable" }
    selfContained = $SelfContained.IsPresent
    version = $buildVersion
    channel = $Channel
    gitCommit = $gitCommit
    gitShortCommit = $gitShortCommit
    gitDirty = $gitDirty
    published = $published
}

$manifestPath = Join-Path $outputRootPath "publish-manifest.json"
$manifest | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 -Path $manifestPath
$manifest | ConvertTo-Json -Compress -Depth 6
