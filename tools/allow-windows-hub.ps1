# Let phones and tablets on a private network reach God's Camera's scanner
# through Windows Firewall. Or, with -Remove, take that away again.
#
#   powershell -File tools\allow-windows-hub.ps1 [-Remove] [-Program FILE] [-Port 47801] [-Tidy FOLDER]
#
# -Program names GodsCamera.exe. Without it, the one in this repository's
# build folder is taken. The app carries a copy of this script and starts it
# when "Allow devices on my network" is clicked.
#
# Run it in a PowerShell window opened with "Run as administrator": only an
# administrator may change the firewall's rules.
#
# What it changes, and nothing else: two rules in Windows Firewall, both for
# that one program and that one port.
#   - "God's Camera - devices on a private network" lets calls in that come
#     from a private network, such as home Wi-Fi.
#   - "God's Camera - nothing from public networks" turns calls from public
#     networks away. Windows would do the same by itself, but only after
#     putting a question on the screen the first time the scanner is turned on.
# Rules of these names made earlier, for a copy of the program in another
# folder, are replaced.
#
# -Tidy FOLDER also removes the rules Windows made by itself for programs
# under FOLDER that no longer exist. Test programs that listened on the
# network leave such rules behind; give it the repository's core\target folder.
#
# Works in Windows PowerShell 5 and in PowerShell 7. Plain ASCII on purpose.

param(
    [switch]$Remove,
    [string]$Program,
    [int]$Port = 47801,
    [string]$Tidy
)

$ErrorActionPreference = 'Stop'

$private = "God's Camera - devices on a private network"
$public = "God's Camera - nothing from public networks"
$root = Split-Path -Parent $PSScriptRoot

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'This script was not started as administrator, so it has changed nothing.'
    Write-Host 'Open PowerShell with "Run as administrator" and run it again:'
    Write-Host "  powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`"$(if ($Remove) { ' -Remove' })"
    exit 1
}

# The rules are found by name; the same name is their identifier, so that the
# app can ask Windows for them by it.
foreach ($name in $private, $public) {
    Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue | Remove-NetFirewallRule
}

if ($Tidy) {
    $folder = [IO.Path]::GetFullPath($Tidy).TrimEnd('\') + '\'
    $gone = 0
    foreach ($filter in Get-NetFirewallApplicationFilter -ErrorAction SilentlyContinue) {
        $file = $filter.Program
        if ($file -and $file.StartsWith($folder, [StringComparison]::OrdinalIgnoreCase) -and -not (Test-Path -LiteralPath $file)) {
            $filter | Get-NetFirewallRule | Remove-NetFirewallRule
            $gone++
        }
    }
    Write-Host "Removed $gone rule(s) for programs under $folder that no longer exist."
}

if ($Remove) {
    Write-Host "Removed. Windows Firewall no longer lets devices reach God's Camera's scanner."
    exit 0
}

if (-not $Program) {
    $Program = Join-Path $root 'apps\windows\app\bin\Release\net10.0-windows10.0.26100.0\win-x64\GodsCamera.exe'
}
if (-not (Test-Path -LiteralPath $Program -PathType Leaf)) {
    Write-Host "The program was not found: $Program"
    Write-Host 'Nothing was allowed.'
    exit 1
}
$Program = (Resolve-Path -LiteralPath $Program).Path

New-NetFirewallRule -Name $private -DisplayName $private `
    -Description "Phones and tablets on a private network may send scans to God's Camera." `
    -Direction Inbound -Action Allow -Program $Program -Protocol TCP -LocalPort $Port -Profile Private | Out-Null
New-NetFirewallRule -Name $public -DisplayName $public `
    -Description "God's Camera's scanner is not reachable from public networks." `
    -Direction Inbound -Action Block -Program $Program -Protocol TCP -LocalPort $Port -Profile Public | Out-Null

Write-Host "Allowed: devices on a private network can reach God's Camera's scanner on port $Port."
Write-Host "Program: $Program"
exit 0
