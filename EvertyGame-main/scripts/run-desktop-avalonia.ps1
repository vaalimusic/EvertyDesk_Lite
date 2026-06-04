Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Push-Location $PSScriptRoot/..
try {
    dotnet run --project desktop-avalonia/Everty.Desktop.Avalonia.csproj
}
finally {
    Pop-Location
}
