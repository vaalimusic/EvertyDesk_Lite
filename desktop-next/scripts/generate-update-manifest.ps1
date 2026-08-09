param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [Parameter(Mandatory = $true)]
    [string]$BaseUrl,
    [string]$DistDir = "",
    [string]$OutFile = "",
    [string]$Notes = ""
)

$ErrorActionPreference = "Stop"

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Manifest version must be numeric X.Y.Z, got '$Version'"
}
if ($Version.Length -gt 32) {
    throw "Manifest version must be at most 32 bytes"
}
if (!$BaseUrl.StartsWith("https://", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "BaseUrl must start with https://"
}
if ($Notes.Length -gt 8192) {
    throw "Manifest notes must be at most 8192 bytes"
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
if ([string]::IsNullOrWhiteSpace($DistDir)) {
    $DistDir = Join-Path $root "dist"
}
$DistDir = (Resolve-Path -LiteralPath $DistDir).Path
if ([string]::IsNullOrWhiteSpace($OutFile)) {
    $OutFile = Join-Path $DistDir "latest.json"
}

$BaseUrl = $BaseUrl.TrimEnd("/")

function Get-Artifact($Pattern) {
    $matches = @(Get-ChildItem -LiteralPath $DistDir -File -Filter $Pattern | Sort-Object Name)
    if ($matches.Count -gt 1) {
        $names = ($matches | Select-Object -ExpandProperty Name) -join ", "
        throw "Multiple artifacts match '$Pattern': $names"
    }
    if ($matches.Count -eq 0) {
        return $null
    }

    $file = $matches[0]
    $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
    if ($sha256 -notmatch '^[0-9a-f]{64}$') {
        throw "Computed SHA256 has an unexpected shape for $($file.Name): $sha256"
    }
    $shaFile = "$($file.FullName).sha256"
    if (Test-Path -LiteralPath $shaFile) {
        $declared = (Get-Content -LiteralPath $shaFile -Raw).Trim().Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)[0]
        if ($declared -notmatch '^[0-9a-fA-F]{64}$') {
            throw "Sidecar SHA256 has an unexpected shape for $($file.Name): $declared"
        }
        if (![string]::IsNullOrWhiteSpace($declared) -and $declared.ToLowerInvariant() -ne $sha256) {
            throw "SHA256 mismatch for $($file.Name): sidecar has $declared, computed $sha256"
        }
    }

    [ordered]@{
        url = "$BaseUrl/$($file.Name)"
        sha256 = $sha256
        size_bytes = $file.Length
    }
}

$windows = Get-Artifact "EvertyDeskNext-$Version-x64.msi"
$macos = Get-Artifact "EvertyDeskNext-$Version.dmg"
$windowsPortable = Get-Artifact "EvertyDeskNext-$Version-windows-x64-portable.zip"

if ($null -eq $windows -and $null -eq $macos -and $null -eq $windowsPortable) {
    throw "No release artifacts found in $DistDir for version $Version"
}

$manifest = [ordered]@{
    version = $Version
    notes = $Notes
}
if ($null -ne $windows) {
    $manifest.windows = $windows
}
if ($null -ne $macos) {
    $manifest.macos = $macos
}
if ($null -ne $windowsPortable) {
    $manifest.windows_portable = $windowsPortable
}

$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutFile -Encoding utf8

[pscustomobject]@{
    Version = $Version
    Manifest = (Resolve-Path -LiteralPath $OutFile).Path
    Windows = $null -ne $windows
    Macos = $null -ne $macos
    WindowsPortable = $null -ne $windowsPortable
}
