param(
    [switch]$IncludePublishedPlatform,
    [switch]$SkipPaymentProvider,
    [switch]$SkipDockerCompose
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$results = [System.Collections.Generic.List[object]]::new()

function Invoke-ReadinessStep {
    param(
        [string]$Name,
        [scriptblock]$Script
    )

    $started = Get-Date
    Write-Host "==> $Name"
    try {
        & $Script
        $results.Add([pscustomobject]@{
            name = $Name
            status = "ok"
            durationSeconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 2)
        })
    } catch {
        $results.Add([pscustomobject]@{
            name = $Name
            status = "failed"
            durationSeconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 2)
            error = $_.Exception.Message
        })
        throw
    }
}

function Start-TemporaryControlPlane {
    param(
        [string]$BaseUrl
    )

    $stateDir = Join-Path $env:TEMP ("everty-product-readiness-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
    $statePath = Join-Path $stateDir "state.json"
    $out = Join-Path $stateDir "control-plane.out.log"
    $err = Join-Path $stateDir "control-plane.err.log"

    $previousStatePath = $env:EVERTY_CONTROL_PLANE_STATE_PATH
    $env:EVERTY_CONTROL_PLANE_STATE_PATH = $statePath

    $process = Start-Process -FilePath "dotnet" `
        -ArgumentList @("run", "--project", "control-plane/Everty.ControlPlane.csproj", "--urls", $BaseUrl) `
        -WorkingDirectory $root `
        -RedirectStandardOutput $out `
        -RedirectStandardError $err `
        -PassThru

    try {
        $ready = $false
        for ($attempt = 0; $attempt -lt 40; $attempt++) {
            Start-Sleep -Milliseconds 500
            if ($process.HasExited) {
                break
            }

            try {
                $response = Invoke-RestMethod -Method "GET" -Uri "$BaseUrl/api/ready" -TimeoutSec 2
                if ($response.ready -eq $true) {
                    $ready = $true
                    break
                }
            } catch {
            }
        }

        if (-not $ready) {
            $stdout = if (Test-Path $out) { Get-Content -LiteralPath $out -Raw } else { "" }
            $stderr = if (Test-Path $err) { Get-Content -LiteralPath $err -Raw } else { "" }
            throw "temporary control-plane did not become ready. stdout=$stdout stderr=$stderr"
        }

        return [pscustomobject]@{
            Process = $process
            StateDir = $stateDir
            PreviousStatePath = $previousStatePath
        }
    } catch {
        if ($process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        $env:EVERTY_CONTROL_PLANE_STATE_PATH = $previousStatePath
        Remove-Item -LiteralPath $stateDir -Recurse -Force -ErrorAction SilentlyContinue
        throw
    }
}

Push-Location $root
try {
    Invoke-ReadinessStep "build control-plane" {
        dotnet build control-plane\Everty.ControlPlane.csproj --no-restore | Out-Host
    }

    Invoke-ReadinessStep "build relay-node" {
        dotnet build relay-node\Everty.RelayNode.csproj --no-restore | Out-Host
    }

    if (-not $SkipDockerCompose) {
        Invoke-ReadinessStep "docker compose config" {
            docker compose --env-file deploy/docker-compose.env.example config | Out-Null
        }
    }

    Invoke-ReadinessStep "release hygiene audit" {
        powershell -NoProfile -ExecutionPolicy Bypass -File scripts\audit-release-hygiene.ps1 | Out-Host
    }

    Invoke-ReadinessStep "control-plane ready smoke" {
        powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-ready.ps1 -BaseUrl http://127.0.0.1:5221 | Out-Host
    }

    Invoke-ReadinessStep "control-plane persistence smoke" {
        powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-persistence.ps1 -BaseUrl http://127.0.0.1:5222 | Out-Host
    }

    Invoke-ReadinessStep "control-plane security smoke" {
        powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-security.ps1 -BaseUrl http://127.0.0.1:5223 | Out-Host
    }

    Invoke-ReadinessStep "operator admin smoke" {
        powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-admin.ps1 -BaseUrl http://127.0.0.1:5224 | Out-Host
    }

    Invoke-ReadinessStep "operator dashboard smoke" {
        powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-admin-dashboard.ps1 -BaseUrl http://127.0.0.1:5225 | Out-Host
    }

    Invoke-ReadinessStep "operator CLI smoke" {
        powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-control-plane-admin-cli.ps1 -BaseUrl http://127.0.0.1:5226 | Out-Host
    }

    Invoke-ReadinessStep "route policy smoke" {
        $controlPlane = Start-TemporaryControlPlane -BaseUrl http://127.0.0.1:5227
        try {
            powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-route-policy.ps1 -BaseUrl http://127.0.0.1:5227 | Out-Host
        } finally {
            if ($controlPlane.Process -and -not $controlPlane.Process.HasExited) {
                Stop-Process -Id $controlPlane.Process.Id -Force -ErrorAction SilentlyContinue
                $controlPlane.Process.WaitForExit(5000) | Out-Null
            }
            $env:EVERTY_CONTROL_PLANE_STATE_PATH = $controlPlane.PreviousStatePath
            Remove-Item -LiteralPath $controlPlane.StateDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    if (-not $SkipPaymentProvider) {
        Invoke-ReadinessStep "payment provider happy-path smoke" {
            powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5228 -ProviderPort 5229 | Out-Host
        }

        Invoke-ReadinessStep "payment provider retry smoke" {
            powershell -NoProfile -ExecutionPolicy Bypass -File control-plane\smoke-payment-provider.ps1 -BaseUrl http://127.0.0.1:5230 -ProviderPort 5231 -FailAction capture -FailOnce | Out-Host
        }
    }

    if ($IncludePublishedPlatform) {
        Invoke-ReadinessStep "published platform smoke" {
            powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke-published-platform.ps1 -Url http://127.0.0.1:5232 -RelayUdpPort 6232 | Out-Host
        }
    }

    [pscustomobject]@{
        status = "ok"
        checks = $results
    } | ConvertTo-Json -Depth 6
} finally {
    Pop-Location
}
