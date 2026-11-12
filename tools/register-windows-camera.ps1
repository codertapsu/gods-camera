# Put God's Camera's media source where Windows' camera service can load it,
# and tell Windows about it. Or, with -Unregister, take both away again.
#
#   powershell -File tools\register-windows-camera.ps1 [-Unregister]
#
# Run it in a PowerShell window opened with "Run as administrator". The camera
# service runs as a system account, and only looks for camera sources in the
# part of the registry that belongs to the whole computer.
#
# What it changes, and nothing else:
#   - the file   C:\Program Files\GodsCamera\GodsCameraSource.dll
#   - the key    HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\{1E1EB430-08A7-42F4-9F7D-C53A00B36024}
#
# If an older copy of the file is in use, it stops the two Windows camera
# services for a moment. They start again by themselves when an app next uses
# a camera; a camera that is in use at that moment is interrupted.
#
# Works in Windows PowerShell 5 and in PowerShell 7.

param(
    [switch]$Unregister
)

$ErrorActionPreference = 'Stop'

$classId = '{1E1EB430-08A7-42F4-9F7D-C53A00B36024}'
$classKey = "HKLM:\SOFTWARE\Classes\CLSID\$classId"
$fileName = 'GodsCameraSource.dll'
$installFolder = Join-Path $env:ProgramFiles 'GodsCamera'
$installed = Join-Path $installFolder $fileName
$root = Split-Path -Parent $PSScriptRoot
$built = Join-Path $root "apps\windows\build\bin\$fileName"
$regsvr32 = Join-Path $env:SystemRoot 'System32\regsvr32.exe'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'This script was not started as administrator, so it has changed nothing.'
    Write-Host 'Open PowerShell with "Run as administrator" and run it again:'
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`"$(if ($Unregister) { ' -Unregister' })"
    exit 1
}

# regsvr32 is a windowed program, so PowerShell would not wait for it by itself.
function Invoke-Regsvr32([string[]]$Options) {
    $arguments = @($Options) + ('"' + $installed + '"')
    $process = Start-Process -FilePath $regsvr32 -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden
    return $process.ExitCode
}

# Do something to the installed file; if Windows' camera service has it open,
# stop the camera services and try once more.
function Invoke-WithCameraServicesStopped([scriptblock]$Action) {
    try {
        & $Action
    }
    catch [System.IO.IOException], [System.UnauthorizedAccessException] {
        Write-Host 'The file is in use by the Windows camera service. Stopping the camera services for a moment.'
        Write-Host '(They start again by themselves when an app next uses a camera.)'
        foreach ($service in 'FrameServerMonitor', 'FrameServer') {
            if (Get-Service -Name $service -ErrorAction SilentlyContinue) {
                Stop-Service -Name $service -Force -ErrorAction SilentlyContinue
            }
        }
        Start-Sleep -Seconds 2
        try {
            & $Action
        }
        catch [System.IO.IOException], [System.UnauthorizedAccessException] {
            Write-Host "The file is still in use: $installed"
            Write-Host "Close the apps that are using a camera, and God's Camera or its test host, then run this again."
            exit 1
        }
    }
}

if ($Unregister) {
    if (Test-Path $installed) {
        $code = Invoke-Regsvr32 '/s', '/u'
        if ($code -ne 0) {
            Write-Host "regsvr32 could not unregister the file (code $code). Removing the registry key directly."
        }
    }
    if (Test-Path -LiteralPath $classKey) {
        Remove-Item -LiteralPath $classKey -Recurse -Force
    }
    if (Test-Path $installed) {
        Invoke-WithCameraServicesStopped { Remove-Item -LiteralPath $installed -Force }
    }
    if ((Test-Path $installFolder) -and -not (Get-ChildItem -LiteralPath $installFolder -Force)) {
        Remove-Item -LiteralPath $installFolder -Force
    }
    Write-Host "God's Camera's media source is no longer registered, and its file is removed."
    exit 0
}

if (-not (Test-Path $built)) {
    Write-Host "The media source has not been built yet: $built is missing."
    Write-Host 'Build it first (this needs no administrator):'
    Write-Host '  powershell -ExecutionPolicy Bypass -File tools\build-windows.ps1'
    exit 1
}

if (-not (Test-Path $installFolder)) {
    New-Item -ItemType Directory -Path $installFolder | Out-Null
}
Invoke-WithCameraServicesStopped { Copy-Item -LiteralPath $built -Destination $installed -Force }

$code = Invoke-Regsvr32 '/s'
if ($code -ne 0) {
    Write-Host "regsvr32 could not register the file (code $code)."
    Write-Host "The file was copied to $installed, but Windows has not been told about it."
    exit 1
}

# Read back what was written, rather than trusting the exit code alone.
$registeredPath = $null
if (Test-Path -LiteralPath "$classKey\InprocServer32") {
    $registeredPath = (Get-ItemProperty -LiteralPath "$classKey\InprocServer32").'(default)'
}
if (-not $registeredPath -or -not (Test-Path -LiteralPath $registeredPath)) {
    Write-Host 'regsvr32 reported success, but the registry does not name a file that exists:'
    Write-Host "  $classKey\InprocServer32 = $registeredPath"
    exit 1
}

Write-Host "God's Camera's media source is registered."
Write-Host "  file:     $registeredPath"
Write-Host "  registry: HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\$classId"
Write-Host ''
Write-Host 'The camera itself appears while the app, or the test host, is running:'
Write-Host '  apps\windows\build\bin\gc-vcam-host.exe'
Write-Host 'and this checks it without opening any app:'
Write-Host '  apps\windows\build\bin\gc-vcam-probe.exe --seconds 10'
