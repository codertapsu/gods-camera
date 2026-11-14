# Build God's Camera's Windows installer: one .msi with the app, the camera
# other apps see, the scanner's firewall rules and the model files.
#
#   powershell -ExecutionPolicy Bypass -File tools\build-windows-installer.ps1 [-SkipApp]
#
# What it does, and where:
#   1. builds the shared core and the app for release into
#      apps\windows\build\installer\app. The user's own build folder, and an
#      app running from it, are not touched. -SkipApp uses the app built there before.
#   2. gathers the model files the Windows app loads from ..\packs, beside the
#      repository, into apps\windows\build\installer\packs, as hard links
#      (nothing is copied; ..\packs and the repository are on one drive).
#   3. builds apps\windows\build\installer\GodsCamera-<version>.msi from
#      apps\windows\installer\Package.wxs.
#
# It needs WiX 5 and its utility extension, installed once for the user:
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add --global WixToolset.Util.wixext/5.0.2
#
# Building needs no administrator. Installing the .msi does: it registers the
# camera for the whole computer and changes Windows Firewall. That is the
# user's step.
#
# Plain ASCII on purpose: Windows PowerShell reads a script without a
# byte-order mark in the machine's code page.

param([switch]$SkipApp)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$work = Join-Path $repo 'apps\windows\build\installer'
$app = Join-Path $work 'app'
$gathered = Join-Path $work 'packs'
$store = Join-Path (Split-Path -Parent $repo) 'packs'
$project = [xml](Get-Content (Join-Path $repo 'apps\windows\app\GodsCamera.csproj'))
$version = @($project.Project.PropertyGroup | ForEach-Object { $_.Version } | Where-Object { $_ })[0]

$wix = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
if (-not (Test-Path $wix)) { $wix = (Get-Command wix -ErrorAction SilentlyContinue).Source }
if (-not $wix) { throw 'WiX was not found. See the top of this script for how to install it.' }

# The model files the Windows app loads (docs/implementation-plan.md, section
# 6.9), with HyperSwap and the occlusion mask, so that every option works
# without a download. The original inswapper_128.onnx, which only a computer
# without a graphics card would use, is left out: the app needs the card.
$models = @(
    'buffalo_l\det_10g.onnx', 'buffalo_l\w600k_r50.onnx', 'buffalo_s\det_500m.onnx',
    'inswapper_128.mixed.onnx', 'inswapper_128.emap.bin', 'hyperswap_1b_256.mixed.onnx', 'xseg_3.onnx',
    'PP-OCRv6_small_det.onnx', 'asternet_vi.onnx', 'asternet_vi.vocab.json'
)

New-Item -ItemType Directory -Force $work | Out-Null

if (-not $SkipApp) {
    Write-Host '== the app, for release =='
    if (Test-Path $app) { Remove-Item -Recurse -Force $app }
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo 'tools\build-windows-app.ps1') -OutputFolder $app
    if ($LASTEXITCODE -ne 0) { throw 'the app did not build' }
}
foreach ($needed in 'GodsCamera.exe', 'gc_ffi.dll', 'gc-ocr.exe', 'Camera\GodsCameraSource.dll', 'Network\allow-windows-hub.ps1') {
    if (-not (Test-Path (Join-Path $app $needed))) {
        throw "The built app lacks $needed. Build the camera with tools\build-windows.ps1, then run this again."
    }
}

Write-Host '== the model files =='
if (Test-Path $gathered) { Remove-Item -Recurse -Force $gathered }
foreach ($model in $models) {
    $from = Join-Path $store $model
    if (-not (Test-Path $from)) { throw "The model file $model is missing from $store. Run python tools\fetch-packs.py." }
    $to = Join-Path $gathered $model
    New-Item -ItemType Directory -Force (Split-Path -Parent $to) | Out-Null
    New-Item -ItemType HardLink -Path $to -Target $from | Out-Null
}
$size = (Get-ChildItem -Recurse -File $gathered | Measure-Object -Sum Length).Sum
Write-Host ("{0} files, {1:N0} MB" -f $models.Count, ($size / 1MB))

Write-Host '== the installer =='
$msi = Join-Path $work "GodsCamera-$version.msi"
& $wix build -arch x64 -ext WixToolset.Util.wixext `
    -d "Version=$version" -d "AppDir=$app" -d "PacksDir=$gathered" `
    -o $msi (Join-Path $repo 'apps\windows\installer\Package.wxs')
if ($LASTEXITCODE -ne 0) { throw 'the installer did not build' }
Write-Host ("Built {0}, {1:N0} MB" -f $msi, ((Get-Item $msi).Length / 1MB))
