param(
    [Parameter(Mandatory = $true)]
    [string]$Version,
    [string]$DistDir = "",
    [string]$Manifest = "",
    [switch]$RequireWindows,
    [switch]$RequireMacos,
    [switch]$RequireWindowsPortable,
    [switch]$SkipManifest
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression.FileSystem

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Release version must be numeric X.Y.Z, got '$Version'"
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
if ([string]::IsNullOrWhiteSpace($DistDir)) {
    $DistDir = Join-Path $root "dist"
}
if (!(Test-Path -LiteralPath $DistDir -PathType Container)) {
    throw "Distribution directory does not exist: $DistDir"
}
$DistDir = (Resolve-Path -LiteralPath $DistDir).Path

if ([string]::IsNullOrWhiteSpace($Manifest)) {
    $Manifest = Join-Path $DistDir "latest.json"
}

function Test-Sha256Text([string]$Value, [string]$Label) {
    if ($Value -notmatch '^[0-9a-fA-F]{64}$') {
        throw "$Label must be exactly 64 hex characters"
    }
}

function Get-SingleArtifact([string]$Pattern, [bool]$Required) {
    $matches = @(Get-ChildItem -LiteralPath $DistDir -File -Filter $Pattern | Sort-Object Name)
    if ($matches.Count -gt 1) {
        $names = ($matches | Select-Object -ExpandProperty Name) -join ", "
        throw "Multiple artifacts match '$Pattern': $names"
    }
    if ($matches.Count -eq 0) {
        if ($Required) {
            throw "Required artifact is missing: $Pattern"
        }
        return $null
    }
    return $matches[0]
}

function Test-Artifact([System.IO.FileInfo]$File, [string]$Kind) {
    if ($null -eq $File) {
        return $null
    }
    if ($File.Length -le 0) {
        throw "$Kind artifact is empty: $($File.FullName)"
    }

    $computed = (Get-FileHash -Algorithm SHA256 -LiteralPath $File.FullName).Hash.ToLowerInvariant()
    Test-Sha256Text $computed "$Kind computed SHA256"

    $shaFile = "$($File.FullName).sha256"
    if (!(Test-Path -LiteralPath $shaFile -PathType Leaf)) {
        throw "$Kind sidecar SHA256 file is missing: $shaFile"
    }
    $declared = (Get-Content -LiteralPath $shaFile -Raw).Trim().Split(" ", [System.StringSplitOptions]::RemoveEmptyEntries)[0]
    Test-Sha256Text $declared "$Kind sidecar SHA256"
    if ($declared.ToLowerInvariant() -ne $computed) {
        throw "$Kind SHA256 mismatch: sidecar has $declared, computed $computed"
    }

    [pscustomobject]@{
        Kind = $Kind
        Name = $File.Name
        Path = $File.FullName
        SizeBytes = $File.Length
        Sha256 = $computed
    }
}

function Test-WindowsPortableZip([System.IO.FileInfo]$File) {
    if ($null -eq $File) {
        return
    }

    $archive = [System.IO.Compression.ZipFile]::OpenRead($File.FullName)
    try {
        $entries = @($archive.Entries | Where-Object { ![string]::IsNullOrWhiteSpace($_.Name) })
        if ($entries.Count -eq 0) {
            throw "windows_portable zip has no file entries"
        }

        $entryByName = @{}
        foreach ($entry in $entries) {
            $normalized = $entry.FullName.Replace("\", "/")
            if ($normalized.StartsWith("/", [System.StringComparison]::Ordinal) -or
                $normalized.Contains("../") -or
                $normalized.Contains("..\")) {
                throw "windows_portable zip contains unsafe entry path: $($entry.FullName)"
            }
            if ($normalized.Contains("/")) {
                throw "windows_portable zip must keep files at top level, found nested entry: $($entry.FullName)"
            }
            $entryByName[$entry.Name] = $entry
        }

        foreach ($required in @(
            "evertydesk-launcher.exe",
            "evertydesk-viewer.exe",
            "evertydesk-rdp-viewer.exe",
            "README.txt"
        )) {
            if (!$entryByName.ContainsKey($required)) {
                throw "windows_portable zip is missing required entry: $required"
            }
            if ($entryByName[$required].Length -le 0) {
                throw "windows_portable zip entry is empty: $required"
            }
        }
    }
    finally {
        $archive.Dispose()
    }
}

function Test-ManifestArtifact($ManifestJson, [string]$Property, [System.IO.FileInfo]$File, [bool]$Required) {
    if ($null -eq $File) {
        if ($Required -and $null -eq $ManifestJson.$Property) {
            throw "Manifest is missing required '$Property' artifact"
        }
        return
    }
    if ($null -eq $ManifestJson.$Property) {
        throw "Manifest is missing '$Property' for artifact $($File.Name)"
    }
    $entry = $ManifestJson.$Property
    if ([string]::IsNullOrWhiteSpace($entry.url) -or !$entry.url.StartsWith("https://", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Manifest '$Property.url' must be HTTPS"
    }
    if (!$entry.url.EndsWith("/$($File.Name)", [System.StringComparison]::Ordinal)) {
        throw "Manifest '$Property.url' must end with artifact name '$($File.Name)'"
    }
    Test-Sha256Text $entry.sha256 "Manifest '$Property.sha256'"
    $computed = (Get-FileHash -Algorithm SHA256 -LiteralPath $File.FullName).Hash.ToLowerInvariant()
    if ($entry.sha256.ToLowerInvariant() -ne $computed) {
        throw "Manifest '$Property.sha256' does not match $($File.Name)"
    }
    if ($null -ne $entry.size_bytes -and [int64]$entry.size_bytes -ne $File.Length) {
        throw "Manifest '$Property.size_bytes' does not match $($File.Name)"
    }
}

$windows = Get-SingleArtifact "EvertyDeskNext-$Version-x64.msi" $RequireWindows.IsPresent
$macos = Get-SingleArtifact "EvertyDeskNext-$Version.dmg" $RequireMacos.IsPresent
$windowsPortable = Get-SingleArtifact "EvertyDeskNext-$Version-windows-x64-portable.zip" $RequireWindowsPortable.IsPresent

if ($null -eq $windows -and $null -eq $macos -and $null -eq $windowsPortable) {
    throw "No release artifacts found in $DistDir for version $Version"
}

$artifacts = @(
    (Test-Artifact $windows "windows"),
    (Test-Artifact $macos "macos"),
    (Test-Artifact $windowsPortable "windows_portable")
) | Where-Object { $null -ne $_ }

Test-WindowsPortableZip $windowsPortable

if ($SkipManifest) {
    $Manifest = $null
}
elseif (Test-Path -LiteralPath $Manifest -PathType Leaf) {
    $manifestJson = Get-Content -LiteralPath $Manifest -Raw | ConvertFrom-Json
    if ($manifestJson.version -ne $Version) {
        throw "Manifest version '$($manifestJson.version)' does not match expected '$Version'"
    }
    Test-ManifestArtifact $manifestJson "windows" $windows $RequireWindows.IsPresent
    Test-ManifestArtifact $manifestJson "macos" $macos $RequireMacos.IsPresent
    Test-ManifestArtifact $manifestJson "windows_portable" $windowsPortable $RequireWindowsPortable.IsPresent
}
elseif ($RequireWindows -or $RequireMacos -or $RequireWindowsPortable) {
    throw "Manifest file is missing: $Manifest"
}

[pscustomobject]@{
    Version = $Version
    DistDir = $DistDir
    Manifest = if (![string]::IsNullOrWhiteSpace($Manifest) -and (Test-Path -LiteralPath $Manifest -PathType Leaf)) { (Resolve-Path -LiteralPath $Manifest).Path } else { $null }
    ArtifactCount = @($artifacts).Count
    Artifacts = $artifacts
}
