$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Dist = Join-Path $Root "dist"
$Zip = Join-Path $Dist "EvertyDesk_Lite_linux_source.zip"
$Prefix = "EvertyDesk_Lite"

if (-not (Test-Path $Dist)) {
    New-Item -ItemType Directory -Path $Dist | Out-Null
}

if (Test-Path $Zip) {
    Remove-Item $Zip
}

$paths = @(
    "Cargo.toml",
    "Cargo.lock",
    "README.md",
    "HABR_ARTICLE.md",
    ".gitignore",
    ".cargo",
    "src",
    "scripts",
    "edesk_lite_logo.png"
)

$compression = [System.IO.Compression.CompressionLevel]::Optimal
$fileStream = [System.IO.File]::Create($Zip)
$archive = [System.IO.Compression.ZipArchive]::new(
    $fileStream,
    [System.IO.Compression.ZipArchiveMode]::Create
)

try {
    foreach ($relative in $paths) {
        $full = Join-Path $Root $relative
        if (-not (Test-Path $full)) {
            continue
        }

        $item = Get-Item -LiteralPath $full
        if ($item.PSIsContainer) {
            $files = Get-ChildItem -LiteralPath $full -Recurse -File -Force
        } else {
            $files = @($item)
        }

        foreach ($file in $files) {
            $sourcePath = $file.FullName
            $rootWithSlash = $Root.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
            $entryRelative = $sourcePath.Substring($rootWithSlash.Length)
            $entryName = "$Prefix/$($entryRelative -replace '\\','/')"

            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive,
                $sourcePath,
                $entryName,
                $compression
            ) | Out-Null
        }
    }
}
finally {
    $archive.Dispose()
    $fileStream.Dispose()
}

Get-Item $Zip | Select-Object FullName, Length, LastWriteTime
