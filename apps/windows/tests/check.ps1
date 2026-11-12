# Check the Windows camera's media source without registering anything.
#
#   powershell -ExecutionPolicy Bypass -File apps\windows\tests\check.ps1 [-Bin <folder with the built files>]
#
# Build first (tools\build-windows.ps1). Needs no administrator and changes
# nothing on the computer: the media source's DLL is loaded into the probe's
# own process. Takes about a minute and a half. It does not test Windows'
# camera service; that needs the registration step (see apps\windows\README.md).
#
# It will not run while God's Camera, or a test host, holds the camera's pipe.
# Works in Windows PowerShell 5 and in PowerShell 7.

param(
    [string]$Bin
)

$ErrorActionPreference = 'Stop'

if (-not $Bin) {
    $Bin = Join-Path (Split-Path -Parent $PSScriptRoot) 'build\bin'
}
$probe = Join-Path $Bin 'gc-vcam-probe.exe'
$hostExe = Join-Path $Bin 'gc-vcam-host.exe'
$dll = Join-Path $Bin 'GodsCameraSource.dll'
foreach ($file in $probe, $hostExe, $dll) {
    if (-not (Test-Path -LiteralPath $file)) {
        Write-Host "Not built yet: $file is missing. Run tools\build-windows.ps1 first."
        exit 1
    }
}

$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$pipeName = "godscamera.frames.$sid"
$scratch = Join-Path $env:TEMP ('gc-camera-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null

$script:checks = 0
$script:failed = New-Object System.Collections.Generic.List[string]

function Check([string]$What, [bool]$Held, [string]$Detail = '') {
    $script:checks++
    if ($Held) {
        Write-Host "  ok    $What"
    }
    else {
        Write-Host "  FAIL  $What  $Detail"
        $script:failed.Add($What)
    }
}

# The test host in the background: the pipe only, never the camera.
function Start-TestHost([string]$Options) {
    $process = Start-Process -FilePath $hostExe -ArgumentList "--no-camera $Options" -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 700
    return $process
}

function Wait-TestHost($Process) {
    if ($Process -and -not $Process.WaitForExit(60000)) {
        $Process.Kill()
    }
}

# Run the probe and hand back its report.
function Read-Camera([string[]]$Options) {
    $lines = & $probe @Options
    return (@($lines)[-1] | ConvertFrom-Json)
}

# The probe in the background, its report in a file.
function Start-Reader([string]$Options, [string]$ReportFile) {
    return Start-Process -FilePath $probe -ArgumentList $Options -PassThru -WindowStyle Hidden -RedirectStandardOutput $ReportFile
}

function Get-Slate($Report) {
    return @($Report.timeline | Where-Object { $_.kind -eq 'slate' })[-1]
}

# A good run of the reader: every picture the host made, in order, with the right colours.
function Check-GoodRun([string]$Title, $Report, [int]$Seconds) {
    # On a busy computer a picture can come late now and then: the source then
    # shows the last one again and may pass over one. Two in a run is still a pass.
    $frames = $Report.frames
    $inOrder = ($frames.restarts -eq 0) -and ($frames.missing -le 2) -and
        (($frames.new + $frames.missing) -eq ($frames.last_number - $frames.first_number + 1))
    $sourceChecks = @($Report.source_checks.PSObject.Properties | Where-Object { $_.Name -ne 'interfaces_missing' })
    $allTrue = @($sourceChecks | Where-Object { $_.Value -ne $true }).Count -eq 0
    Check "${Title}: the probe read the camera" ($Report.ok -eq $true)
    Check "${Title}: about thirty pictures a second" ($Report.samples -ge ($Seconds * 24)) "samples: $($Report.samples)"
    Check "${Title}: the pictures in order, with at most a stray late one" ($inOrder -and $frames.repeats -le 2) "new $($frames.new), missing $($frames.missing), repeats $($frames.repeats)"
    Check "${Title}: no slate and nothing unknown" ($Report.slates -eq 0 -and $Report.others -eq 0) "slates $($Report.slates), others $($Report.others)"
    Check "${Title}: the colours are right" ($Report.colours.checked -gt 0 -and $Report.colours.wrong -eq 0)
    Check "${Title}: the source has what the camera service asks for" ($allTrue -and @($Report.source_checks.interfaces_missing).Count -eq 0)
}

try {
    # Never get in the way of the app: if something holds the pipe, stop here.
    $holder = & $hostExe --no-camera --seconds 1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "The camera's pipe is taken: God's Camera or a test host is running. Close it and run this again."
        Write-Host "  $holder"
        exit 1
    }

    Write-Host 'Reading the camera as an app would'
    foreach ($run in @(
            @{ Size = '1280x720'; Format = 'NV12'; Seconds = 5 },
            @{ Size = '1920x1080'; Format = 'NV12'; Seconds = 4 },
            @{ Size = '1280x720'; Format = 'YUY2'; Seconds = 3 },
            @{ Size = '1920x1080'; Format = 'YUY2'; Seconds = 3 })) {
        $testHost = Start-TestHost "--seconds $($run.Seconds + 3)"
        $report = Read-Camera @('--in-process', $dll, '--seconds', $run.Seconds, '--size', $run.Size, '--format', $run.Format, '--pipe-attribute', 'user')
        Wait-TestHost $testHost
        Check-GoodRun "$($run.Size) $($run.Format)" $report $run.Seconds
        Check "$($run.Size) $($run.Format): that size and format was agreed" ("$($report.negotiated.width)x$($report.negotiated.height)" -eq $run.Size -and $report.negotiated.format -eq $run.Format)
    }

    Write-Host 'Driving the source as the camera service does'
    $testHost = Start-TestHost '--seconds 8'
    $report = Read-Camera @('--drive', $dll, '--pipe-attribute', 'user')
    Wait-TestHost $testHost
    $failedSteps = @($report.steps | Where-Object { -not $_.ok } | ForEach-Object { $_.step }) -join '; '
    Check 'with the host: every step held' ($report.ok -eq $true -and $report.failed -eq 0) $failedSteps
    Check 'with the host: pictures all the way, no slate' ($report.pictures.frames -gt 0 -and $report.pictures.slates -eq 0 -and $report.pictures.others -eq 0) "frames $($report.pictures.frames), slates $($report.pictures.slates)"
    $report = Read-Camera @('--drive', $dll)
    $failedSteps = @($report.steps | Where-Object { -not $_.ok } | ForEach-Object { $_.step }) -join '; '
    Check 'without a host: every step held' ($report.ok -eq $true -and $report.failed -eq 0) $failedSteps
    Check 'without a host: the slate all the way' ($report.pictures.slates -gt 0 -and $report.pictures.frames -eq 0 -and $report.pictures.others -eq 0)

    Write-Host 'The host goes away and comes back'
    $first = Start-TestHost '--seconds 4'
    $reportFile = Join-Path $scratch 'recovery.json'
    $reader = Start-Reader "--in-process `"$dll`" --seconds 12" $reportFile
    Wait-TestHost $first
    Start-Sleep -Seconds 3
    $second = Start-TestHost '--seconds 8'
    [void]$reader.WaitForExit(60000)
    Wait-TestHost $second
    $report = @(Get-Content -LiteralPath $reportFile)[-1] | ConvertFrom-Json
    # The slate may come as two stretches: its reason changes in the moment
    # between the source finding the new host and the host's first picture.
    $kinds = @($report.timeline | ForEach-Object { $_.kind }) -join ', '
    Check 'pictures, then the slate, then pictures again' ($kinds -match '^frames(, slate)+, frames$') "timeline: $kinds"
    Check 'the second host was noticed as a new one' ($report.frames.restarts -eq 1 -and $report.frames.gaps -eq 0)
    $slate = @($report.timeline | Where-Object { $_.kind -eq 'slate' })[0]
    Check 'the slate came half a second after the last picture' ($slate -and ($slate.from_ms - $report.timeline[0].to_ms) -lt 150 -and $report.frames.repeats -ge 5 -and $report.frames.repeats -le 12) "repeats: $($report.frames.repeats)"
    Check 'the slate said the pipe was gone' ($slate -and $slate.why -eq "the app's pipe does not exist" -and $slate.error -eq 2)

    Write-Host 'The slate says why it is showing'
    $report = Read-Camera @('--in-process', $dll, '--seconds', 2)
    $slate = Get-Slate $report
    Check 'no app: the pipe does not exist' ($slate.why -eq "the app's pipe does not exist" -and $slate.error -eq 2 -and $slate.pipe_named_by_app -eq $false) "$($slate.why)"
    Check 'no app: the first slate comes after the grace of 0.3 s, not before' ($report.first_sample_ms -ge 280 -and $report.first_sample_ms -lt 600) "first sample after $($report.first_sample_ms) ms"

    $testHost = Start-TestHost '--seconds 5 --size 1920x1080'
    $slate = Get-Slate (Read-Camera @('--in-process', $dll, '--seconds', 2))
    Wait-TestHost $testHost
    Check 'the app sends another size: said so' ($slate.why -eq 'connected, and the app sends another size than asked for') "$($slate.why)"

    $testHost = Start-TestHost '--seconds 8'
    $reader = Start-Reader "--in-process `"$dll`" --seconds 5" (Join-Path $scratch 'first-reader.json')
    Start-Sleep -Milliseconds 1200
    $slate = Get-Slate (Read-Camera @('--in-process', $dll, '--seconds', 2))
    [void]$reader.WaitForExit(60000)
    Wait-TestHost $testHost
    Check 'a second reader: the pipe is busy' ($slate.why -eq "the app's pipe is busy with another reader" -and $slate.error -eq 231) "$($slate.why)"

    # A pipe of the right name that this account may not open: only the system account may.
    $security = New-Object System.IO.Pipes.PipeSecurity
    $security.SetSecurityDescriptorSddlForm("O:${sid}D:P(A;;GA;;;SY)")
    if ($PSVersionTable.PSEdition -eq 'Core') {
        $server = [System.IO.Pipes.NamedPipeServerStreamAcl]::Create($pipeName, 'InOut', 1, 'Byte', 'Asynchronous', 4096, 4096, $security)
    }
    else {
        $server = New-Object System.IO.Pipes.NamedPipeServerStream($pipeName, 'InOut', 1, 'Byte', 'Asynchronous', 4096, 4096, $security)
    }
    try {
        $slate = Get-Slate (Read-Camera @('--in-process', $dll, '--seconds', 2))
    }
    finally {
        $server.Dispose()
    }
    Check 'a pipe this account may not open: refused' ($slate.why -eq "Windows refused to open the app's pipe" -and $slate.error -eq 5) "$($slate.why)"

    # A pipe that accepts the reader and then sends nothing.
    $server = New-Object System.IO.Pipes.NamedPipeServerStream($pipeName, 'InOut', 1, 'Byte', 'Asynchronous')
    try {
        $waiting = $server.BeginWaitForConnection($null, $null)
        $reader = Start-Reader "--in-process `"$dll`" --seconds 2 --size 1920x1080" (Join-Path $scratch 'silent.json')
        $connected = $waiting.AsyncWaitHandle.WaitOne(10000)
        $request = New-Object byte[] 28
        $read = 0
        if ($connected) {
            $server.EndWaitForConnection($waiting)
            $read = $server.Read($request, 0, 28)
        }
        [void]$reader.WaitForExit(60000)
        $slate = Get-Slate (@(Get-Content -LiteralPath (Join-Path $scratch 'silent.json'))[-1] | ConvertFrom-Json)
    }
    finally {
        $server.Dispose()
    }
    Check 'a silent app: the source connected and asked for its size' ($read -eq 28 -and [BitConverter]::ToUInt32($request, 0) -eq 0x52564347 -and [BitConverter]::ToUInt32($request, 8) -eq 1920 -and [BitConverter]::ToUInt32($request, 12) -eq 1080)
    Check 'a silent app: said so' ($slate.why -eq 'connected, and the app sends nothing') "$($slate.why)"

    Write-Host 'Which pipe names the source takes from the app'
    $names = @(
        @{ Title = "this user's pipe"; Name = 'user'; Taken = $true },
        @{ Title = 'the prefix and another SID'; Name = '\\.\pipe\godscamera.frames.S-1-5-18'; Taken = $true },
        @{ Title = 'another pipe'; Name = '\\.\pipe\some.other.pipe'; Taken = $false },
        @{ Title = 'the prefix, a SID, and more'; Name = "\\.\pipe\godscamera.frames.$sid\..\other"; Taken = $false },
        @{ Title = 'the prefix and no SID'; Name = '\\.\pipe\godscamera.frames.hello'; Taken = $false },
        @{ Title = 'a pipe on another computer'; Name = '\\192.0.2.1\pipe\godscamera.frames.S-1-5-18'; Taken = $false },
        @{ Title = 'a file on another computer'; Name = '\\192.0.2.1\share\godscamera.frames.S-1-5-18'; Taken = $false },
        @{ Title = 'a file on this computer'; Name = 'C:\Windows\win.ini'; Taken = $false })
    foreach ($name in $names) {
        $began = Get-Date
        $slate = Get-Slate (Read-Camera @('--in-process', $dll, '--seconds', 1, '--pipe-attribute', $name.Name))
        $took = ((Get-Date) - $began).TotalSeconds
        $verdict = if ($name.Taken) { 'taken' } else { 'not taken' }
        # A name that is not taken is never opened: a try at another computer would take far longer than this.
        Check "$($name.Title): $verdict" ($slate.pipe_named_by_app -eq $name.Taken -and $took -lt 6) "taken: $($slate.pipe_named_by_app), $([int]$took) s"
    }

    Write-Host 'What registering would write'
    # The real HKEY_LOCAL_MACHINE is never touched. For the two calls only, this process's view of it is
    # pointed at a scratch key under HKEY_CURRENT_USER, so what the DLL would write for the whole computer
    # lands there and can be read back. If that cannot be set up, the DLL's code is not called at all.
    if (-not ('GcCameraRegistrationCheck' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class GcCameraRegistrationCheck
{
    [DllImport("kernel32", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr LoadLibraryW(string path);
    [DllImport("kernel32", SetLastError = true, CharSet = CharSet.Ansi)]
    static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("advapi32")]
    static extern int RegOverridePredefKey(IntPtr key, IntPtr newKey);
    [DllImport("advapi32", CharSet = CharSet.Unicode)]
    static extern int RegCreateKeyExW(IntPtr key, string subKey, int reserved, string cls, int options, int access,
        IntPtr security, out IntPtr result, out int disposition);
    [DllImport("advapi32")]
    static extern int RegCloseKey(IntPtr key);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    delegate int Entry();

    static readonly IntPtr CurrentUser = new IntPtr(unchecked((int)0x80000001));
    static readonly IntPtr LocalMachine = new IntPtr(unchecked((int)0x80000002));

    public static int CallRedirected(string dll, string entryName, string scratchSubKey)
    {
        IntPtr module = LoadLibraryW(dll);
        if (module == IntPtr.Zero) throw new Exception("LoadLibrary failed: " + Marshal.GetLastWin32Error());
        IntPtr address = GetProcAddress(module, entryName);
        if (address == IntPtr.Zero) throw new Exception("no such entry point: " + entryName);
        Entry entry = (Entry)Marshal.GetDelegateForFunctionPointer(address, typeof(Entry));

        IntPtr scratch;
        int disposition;
        int status = RegCreateKeyExW(CurrentUser, scratchSubKey, 0, null, 0, 0xF003F, IntPtr.Zero, out scratch, out disposition);
        if (status != 0) throw new Exception("could not make the scratch key: " + status);
        try
        {
            status = RegOverridePredefKey(LocalMachine, scratch);
            if (status != 0) throw new Exception("could not redirect HKEY_LOCAL_MACHINE: " + status);
            try
            {
                return entry();
            }
            finally
            {
                RegOverridePredefKey(LocalMachine, IntPtr.Zero);
            }
        }
        finally
        {
            RegCloseKey(scratch);
        }
    }
}
'@
    }
    $classId = '{1E1EB430-08A7-42F4-9F7D-C53A00B36024}'
    $realKey = "HKLM:\SOFTWARE\Classes\CLSID\$classId"
    $realBefore = Test-Path -LiteralPath $realKey
    $scratchSubKey = 'Software\GodsCameraCheck-' + [guid]::NewGuid().ToString('N')
    $written = "HKCU:\$scratchSubKey\SOFTWARE\Classes\CLSID\$classId"
    try {
        $result = [GcCameraRegistrationCheck]::CallRedirected($dll, 'DllRegisterServer', $scratchSubKey)
        Check 'registering succeeds' ($result -eq 0) ('0x{0:X8}' -f $result)
        $server = $null
        if (Test-Path -LiteralPath "$written\InprocServer32") {
            $server = Get-ItemProperty -LiteralPath "$written\InprocServer32"
        }
        Check 'it names this DLL as the in-process server' ($server -and $server.'(default)' -eq $dll) "$($server.'(default)')"
        Check 'it says the class may be used from any thread' ($server -and $server.ThreadingModel -eq 'Both')
        $result = [GcCameraRegistrationCheck]::CallRedirected($dll, 'DllUnregisterServer', $scratchSubKey)
        Check 'unregistering removes all of it' ($result -eq 0 -and -not (Test-Path -LiteralPath $written))
        $result = [GcCameraRegistrationCheck]::CallRedirected($dll, 'DllUnregisterServer', $scratchSubKey)
        Check 'unregistering again is no error' ($result -eq 0) ('0x{0:X8}' -f $result)
    }
    finally {
        if (Test-Path -LiteralPath "HKCU:\$scratchSubKey") {
            Remove-Item -LiteralPath "HKCU:\$scratchSubKey" -Recurse -Force
        }
    }
    Check "the computer's own registration was left as it was" ((Test-Path -LiteralPath $realKey) -eq $realBefore)
}
finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
if ($script:failed.Count -eq 0) {
    Write-Host "All $($script:checks) checks held."
    exit 0
}
Write-Host "$($script:failed.Count) of $($script:checks) checks failed:"
foreach ($what in $script:failed) {
    Write-Host "  $what"
}
exit 1
