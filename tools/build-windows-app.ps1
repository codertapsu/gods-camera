# Build the God's Camera app for Windows: the shared core, the C# description
# of it, and the window.
#
#   powershell -ExecutionPolicy Bypass -File tools\build-windows-app.ps1 [-CoreBuild debug] [-Run]
#
# Needs, all without administrator rights:
#   - Rust, as for the core.
#   - The .NET 10 SDK. If `dotnet` on the path is older, this script looks in
#     %LOCALAPPDATA%\Microsoft\dotnet, where Microsoft's dotnet-install.ps1
#     puts one for the current user:
#       dotnet-install.ps1 -Channel 10.0 -InstallDir $env:LOCALAPPDATA\Microsoft\dotnet -NoPath
#   - uniffi-bindgen-cs, of the version that matches the core's uniffi:
#       cargo install uniffi-bindgen-cs --locked --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.12.0+v0.32.0
#
# The program lands in apps\windows\app\bin\Release\...\win-x64\GodsCamera.exe
# with everything it needs beside it.
#
# Plain ASCII on purpose: Windows PowerShell reads a script without a
# byte-order mark in the machine's code page.

param(
    # Which build of the core to use: release, or debug for a quicker turn-around.
    [ValidateSet('release', 'debug')] [string] $CoreBuild = 'release',
    # Start the app after building it.
    [switch] $Run
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$app = Join-Path $repo 'apps\windows\app'
$core = Join-Path $repo 'core'

function Find-Dotnet {
    $candidates = @()
    $onPath = Get-Command dotnet -ErrorAction SilentlyContinue
    if ($onPath) { $candidates += $onPath.Source }
    $candidates += (Join-Path $env:LOCALAPPDATA 'Microsoft\dotnet\dotnet.exe')
    foreach ($candidate in $candidates) {
        if (-not (Test-Path $candidate)) { continue }
        $sdks = & $candidate --list-sdks 2>$null
        if ($sdks | Where-Object { $_ -match '^10\.' }) { return $candidate }
    }
    throw 'The .NET 10 SDK was not found. See the top of this script for how to install it for the current user.'
}

$dotnet = Find-Dotnet
$env:DOTNET_ROOT = Split-Path -Parent $dotnet
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_NOLOGO = '1'

if (-not (Get-Command uniffi-bindgen-cs -ErrorAction SilentlyContinue)) {
    throw 'uniffi-bindgen-cs was not found. See the top of this script for how to install it.'
}

Write-Host "== the shared core ($CoreBuild) =="
Push-Location $core
try {
    if ($CoreBuild -eq 'release') { cargo build --release -p gc-ffi } else { cargo build -p gc-ffi }
    if ($LASTEXITCODE -ne 0) { throw 'the core did not build' }
} finally { Pop-Location }

Write-Host '== the C# description of the core =='
$library = Join-Path $core "target\$CoreBuild\gc_ffi.dll"
# From the core's folder: the tool asks cargo about the workspace it is in.
Push-Location $core
try {
    uniffi-bindgen-cs --library $library --no-format --out-dir (Join-Path $app 'Core')
    if ($LASTEXITCODE -ne 0) { throw 'the C# description could not be made' }
} finally { Pop-Location }

Write-Host '== the window =='
& $dotnet build (Join-Path $app 'GodsCamera.csproj') -c Release "-p:CoreBuild=$CoreBuild"
if ($LASTEXITCODE -ne 0) { throw 'the app did not build' }

$program = Join-Path $app 'bin\Release\net10.0-windows10.0.26100.0\win-x64\GodsCamera.exe'
Write-Host "Built $program"
if ($Run) { Start-Process $program }
