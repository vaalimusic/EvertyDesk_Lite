param(
    [string]$BaseUrl = "http://127.0.0.1:5180"
)

$ErrorActionPreference = "Stop"

function Start-ControlPlaneSmokeProcess {
    param(
        [string]$StatePath,
        [string]$BaseUrl
    )

    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $StatePath
    $out = Join-Path $env:TEMP "everty-control-plane-persistence.out"
    $err = Join-Path $env:TEMP "everty-control-plane-persistence.err"
    Remove-Item -LiteralPath $out,$err -ErrorAction SilentlyContinue
    $process = Start-Process -FilePath dotnet `
        -ArgumentList @("run", "--project", "control-plane/Everty.ControlPlane.csproj", "--urls", $BaseUrl) `
        -WorkingDirectory (Get-Location) `
        -RedirectStandardOutput $out `
        -RedirectStandardError $err `
        -PassThru

    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 500
        try {
            Invoke-RestMethod -Uri "$BaseUrl/api/health" -TimeoutSec 2 | Out-Null
            return $process
        } catch {
        }
    }

    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    throw "control-plane did not start."
}

function Invoke-ControlPlaneJson {
    param(
        [string]$Method,
        [string]$Path,
        [object]$Body = $null,
        [string]$Token = ""
    )

    $headers = @{}
    if ($Token) {
        $headers["Authorization"] = "Bearer $Token"
    }

    if ($null -eq $Body) {
        return Invoke-RestMethod -Method $Method -Uri "$BaseUrl$Path" -Headers $headers -TimeoutSec 5
    }

    return Invoke-RestMethod -Method $Method `
        -Uri "$BaseUrl$Path" `
        -Headers $headers `
        -Body ($Body | ConvertTo-Json -Depth 8) `
        -ContentType "application/json" `
        -TimeoutSec 5
}

$statePath = Join-Path $env:TEMP "everty-control-plane-state-smoke-$([Guid]::NewGuid().ToString('N')).json"
$process = $null

try {
    $process = Start-ControlPlaneSmokeProcess -StatePath $statePath -BaseUrl $BaseUrl
    $device = Invoke-ControlPlaneJson -Method "POST" -Path "/api/auth/device-login" -Body @{
        deviceLabel = "persistence-smoke"
        platform = "powershell"
    }

    $registeredHost = Invoke-ControlPlaneJson -Method "POST" -Path "/api/hosts/register" -Body @{
        displayName = "persistence-smoke-host"
        region = "local"
        directAddress = "127.0.0.1"
        directPort = 5001
        encoderBackends = @("nvenc")
        supportsHevc = $true
        supportsAudio = $true
        supportsGamepad = $true
        capabilities = @{
            cpuModel = "smoke"
            gpuModel = "smoke"
            ramGb = 16
            maxWidth = 1920
            maxHeight = 1080
            maxFps = 60
        }
    }

    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
    $process = Start-ControlPlaneSmokeProcess -StatePath $statePath -BaseUrl $BaseUrl

    $hosts = Invoke-ControlPlaneJson -Method "GET" -Path "/api/hosts" -Token $device.accessToken
    $restored = @($hosts) | Where-Object { $_.hostId -eq $registeredHost.hostId } | Select-Object -First 1
    if ($null -eq $restored) {
        throw "registered host was not restored from snapshot."
    }

    [pscustomobject]@{
        statePath = $statePath
        restoredHostId = $restored.hostId
        restoredDisplayName = $restored.displayName
    } | ConvertTo-Json -Compress
}
finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    Remove-Item -LiteralPath $statePath,"$statePath.tmp" -ErrorAction SilentlyContinue
}
