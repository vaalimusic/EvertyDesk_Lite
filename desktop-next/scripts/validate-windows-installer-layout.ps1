param(
    [string]$Configuration = "release"
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$binDir = Join-Path $root "target\$Configuration"
$wixDir = Join-Path $root "wix"
$assetsDir = Join-Path $root "assets"
$wxs = Join-Path $wixDir "main.wxs"

function Require-File([string]$Path, [string]$Label) {
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    $item = Get-Item -LiteralPath $Path
    if ($item.Length -le 0) {
        throw "$Label is empty: $Path"
    }
    return $item
}

Require-File (Join-Path $binDir "evertydesk-launcher.exe") "launcher binary" | Out-Null
Require-File (Join-Path $binDir "evertydesk-viewer.exe") "viewer binary" | Out-Null
Require-File (Join-Path $binDir "evertydesk-rdp-viewer.exe") "rdp viewer binary" | Out-Null
Require-File (Join-Path $assetsDir "logo.ico") "installer icon" | Out-Null
Require-File (Join-Path $wixDir "license.rtf") "installer license" | Out-Null
Require-File $wxs "WiX manifest" | Out-Null

$wxsText = Get-Content -LiteralPath $wxs -Raw -Encoding utf8
foreach ($bad in @("Рџ", "Рћ", "РЈ", "РЎ", "вЂ", "Ð", "Ñ")) {
    if ($wxsText.Contains($bad)) {
        throw "WiX manifest appears to contain mojibake marker '$bad': $wxs"
    }
}

[xml]$xml = $wxsText
$ns = New-Object System.Xml.XmlNamespaceManager($xml.NameTable)
$ns.AddNamespace("w", "http://wixtoolset.org/schemas/v4/wxs")
$ns.AddNamespace("ui", "http://wixtoolset.org/schemas/v4/wxs/ui")

$package = $xml.SelectSingleNode("/w:Wix/w:Package", $ns)
if ($null -eq $package) {
    throw "WiX Package node is missing"
}
if ($package.Name -ne "EvertyDesk Next") {
    throw "Unexpected MSI package name: $($package.Name)"
}
if ($package.Version -ne '$(var.EvertyDeskVersion)') {
    throw "MSI package version must come from EvertyDeskVersion variable"
}
if ($package.UpgradeCode -ne "8f2b8c2a-6b1a-4b1e-9f0a-9f3c9c9a2f11") {
    throw "Unexpected MSI UpgradeCode: $($package.UpgradeCode)"
}

foreach ($xpath in @(
    "/w:Wix/w:Package/w:MajorUpgrade",
    "/w:Wix/w:Package/w:MediaTemplate[@EmbedCab='yes']",
    "/w:Wix/w:Package/w:Icon[@Id='EvertyDeskIcon.ico']",
    "/w:Wix/w:Package/w:Property[@Id='ARPPRODUCTICON'][@Value='EvertyDeskIcon.ico']",
    "/w:Wix/w:Package/w:WixVariable[@Id='WixUILicenseRtf']",
    "/w:Wix/w:Package/ui:WixUI[@Id='WixUI_InstallDir'][@InstallDirectory='INSTALLFOLDER']"
)) {
    if ($null -eq $xml.SelectSingleNode($xpath, $ns)) {
        throw "Required WiX node is missing: $xpath"
    }
}

foreach ($fileId in @("LauncherExeFile", "ViewerExeFile", "RdpViewerExeFile")) {
    if ($null -eq $xml.SelectSingleNode("//w:File[@Id='$fileId'][@KeyPath='yes']", $ns)) {
        throw "Required installed file is missing or not KeyPath: $fileId"
    }
}

foreach ($shortcutId in @("EvertyDeskStartMenuShortcut", "EvertyDeskDesktopShortcut")) {
    $shortcut = $xml.SelectSingleNode("//w:Shortcut[@Id='$shortcutId']", $ns)
    if ($null -eq $shortcut) {
        throw "Required shortcut is missing: $shortcutId"
    }
    if ($shortcut.Target -ne "[INSTALLFOLDER]evertydesk-launcher.exe") {
        throw "Shortcut '$shortcutId' does not target the launcher"
    }
    if ($shortcut.Icon -ne "EvertyDeskIcon.ico") {
        throw "Shortcut '$shortcutId' does not use the product icon"
    }
}

[pscustomobject]@{
    Status = "ok"
    Root = $root.Path
    Configuration = $Configuration
    WixManifest = (Resolve-Path -LiteralPath $wxs).Path
}
