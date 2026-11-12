# Save a picture of every screen of the Windows app, to look at it without
# sitting in front of it.
#
#   powershell -ExecutionPolicy Bypass -File tools\snapshot-windows-app.ps1 [-Language en|vi] [-Theme light|dark] [-Demo] [-Out FOLDER] [-Data FOLDER]
#
# The app is started on a data folder of its own, with the test picture as
# its source, so nothing of the user's is opened or changed. Its window shows
# for about ten seconds. The pictures land in -Out, or in a new folder under
# %TEMP%, whose name is printed at the end.
#
# -Demo puts the app through its paces first: it records and plays a loop,
# adds the sample photo as a face and turns the swap on. That needs the model
# files (tools\fetch-packs.py) and the sample photo (tools\make-reference.py),
# uses the graphics card, and takes about a minute.
#
# Only one program can hold the camera's pipe. If the user's own app is
# running, the pictures will say so on the Camera screen; nothing breaks.
#
# Plain ASCII on purpose: Windows PowerShell reads a script without a
# byte-order mark in the machine's code page.

param(
    [ValidateSet('en', 'vi')] [string] $Language = 'en',
    [ValidateSet('light', 'dark')] [string] $Theme = 'light',
    [switch] $Demo,
    [string] $Out,
    # A data folder to show instead of an empty one, for pictures with saved faces and loops in them.
    [string] $Data
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$program = Join-Path $repo 'apps\windows\app\bin\Release\net10.0-windows10.0.26100.0\win-x64\GodsCamera.exe'
if (-not (Test-Path $program)) { throw 'The app is not built. Run tools\build-windows-app.ps1 first.' }

$work = Join-Path $env:TEMP ('gc-app-shots-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
if (-not $Out) { $Out = Join-Path $work "$Language-$Theme" }
if (-not $Data) {
    $Data = Join-Path $work 'data'
    New-Item -ItemType Directory -Force $Data | Out-Null
    Set-Content -Path (Join-Path $Data 'settings.json') -Value '{"source": "pattern"}' -Encoding ascii
}
New-Item -ItemType Directory -Force $Out | Out-Null

# The model files beside the repository, as tools\fetch-packs.py puts them.
$packs = Join-Path (Split-Path -Parent $repo) 'packs'
$arguments = @('--data', $Data, '--packs', $packs, '--language', $Language, '--theme', $Theme, '--snapshots', $Out)
if ($Demo) {
    $photo = Join-Path $repo 'core\gc-infer\tests\golden\input.png'
    if (-not (Test-Path $photo)) { throw 'The sample photo is missing. Run tools\make-reference.py first.' }
    $arguments += @('--demo', $photo)
}
$process = Start-Process $program -ArgumentList $arguments -PassThru
if (-not $process.WaitForExit(360000)) {
    $process.Kill()
    throw 'The app did not finish taking its pictures.'
}
Get-ChildItem $Out -Filter *.png | ForEach-Object { Write-Host $_.FullName }
$failed = Join-Path $Out 'failed.txt'
if (Test-Path $failed) { Get-Content $failed | Select-Object -First 12; throw 'The app could not finish its demonstration.' }
