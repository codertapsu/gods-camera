# Build God's Camera's Windows parts: the virtual camera's media source and
# the two tools that test it.
#
#   powershell -File tools\build-windows.ps1 [-Configuration Release|Debug|RelWithDebInfo] [-Clean]
#
# Needs the Visual Studio 2022 Build Tools (or Visual Studio) with the C++
# tools, CMake and Ninja. Everything is put into apps\windows\build\bin.
# Works in Windows PowerShell 5 and in PowerShell 7. Needs no administrator.

param(
    [ValidateSet('Release', 'Debug', 'RelWithDebInfo')]
    [string]$Configuration = 'Release',
    [switch]$Clean
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$source = Join-Path $root 'apps\windows'
$build = Join-Path $source 'build'

# Find the compiler. vswhere comes with every Visual Studio installer.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    throw "Visual Studio's installer was not found. Install the Visual Studio 2022 Build Tools with the 'Desktop development with C++' workload."
}
$visualStudio = & $vswhere -latest -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $visualStudio) {
    throw "No Visual Studio with the C++ tools was found. Add the 'Desktop development with C++' workload in the Visual Studio Installer."
}
$vcvars = Join-Path $visualStudio 'VC\Auxiliary\Build\vcvars64.bat'
if (-not (Test-Path $vcvars)) {
    throw "The C++ tools' setup script is missing: $vcvars"
}

# Run the compiler's setup script in cmd and copy the environment it leaves behind.
$environment = & cmd.exe /c "`"$vcvars`" >nul 2>&1 && set"
if ($LASTEXITCODE -ne 0) {
    throw "The C++ tools' setup script failed: $vcvars"
}
foreach ($line in $environment) {
    $at = $line.IndexOf('=')
    if ($at -gt 0) {
        Set-Item -Path ("env:" + $line.Substring(0, $at)) -Value $line.Substring($at + 1)
    }
}

foreach ($tool in 'cmake', 'ninja', 'cl') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "'$tool' was not found. CMake and Ninja come with the Visual Studio C++ tools, or from cmake.org and ninja-build.org."
    }
}

if ($Clean -and (Test-Path $build)) {
    Remove-Item -Recurse -Force $build
}

& cmake -S $source -B $build -G Ninja "-DCMAKE_BUILD_TYPE=$Configuration"
if ($LASTEXITCODE -ne 0) {
    throw 'CMake could not set the build up.'
}
& cmake --build $build
if ($LASTEXITCODE -ne 0) {
    throw 'The build failed.'
}

Write-Host ''
Write-Host "Built into $(Join-Path $build 'bin'):"
Get-ChildItem (Join-Path $build 'bin') -Include *.dll, *.exe -Recurse | ForEach-Object { Write-Host "  $($_.Name)" }
