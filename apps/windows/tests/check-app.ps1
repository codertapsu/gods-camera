# Check what the app's window does outside its screens: the hot keys, closing
# to the notification area, switching the picture off while nobody looks, one
# copy at a time, a clean end, and the scanner taking a page and reading it.
#
#   powershell -ExecutionPolicy Bypass -File apps\windows\tests\check-app.ps1 [-Program FILE]
#
# The app is started on a data folder of its own with the test picture as its
# source, so nothing of the user's is opened or changed, and no camera is
# turned on. Its window shows for about half a minute, and a second God's
# Camera icon sits beside the clock for that time. If the user's own app is
# running, it is left alone: it holds the camera's pipe, and the copy started
# here then simply has no camera for other apps.
#
# The scanner of a copy on a data folder of its own stays on this computer:
# its hub takes ports the system picks and listens on 127.0.0.1 only, so no
# other device can reach it and Windows Firewall has nothing to ask. A page is
# sent to it with the core's own tool, as a device on a cable would send it.
#
# The window's messages are sent to it directly: the message a hot key causes,
# the message the icon sends when it is clicked, and the request to close. No
# key is pressed and no mouse is moved. Whether Windows gave the app its hot
# keys is reported; pressing them is for a person to try.
#
# Plain ASCII on purpose: Windows PowerShell reads a script without a
# byte-order mark in the machine's code page.

param(
    # The built program, if not the one in the usual place.
    [string] $Program
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
if (-not $Program) { $Program = Join-Path $repo 'apps\windows\app\bin\Release\net10.0-windows10.0.26100.0\win-x64\GodsCamera.exe' }
if (-not (Test-Path $Program)) { throw "The app is not built: $Program is missing. Run tools\build-windows-app.ps1 first." }

Add-Type -Namespace GodsCameraCheck -Name User32 -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern uint RegisterWindowMessageW(string name);
'@

# The mark the app makes of its data folder, which its own messages carry in their names.
function Get-FolderMark([string] $folder) {
    $bytes = [Text.Encoding]::UTF8.GetBytes([IO.Path]::GetFullPath($folder).ToLowerInvariant())
    $hash = [Security.Cryptography.SHA256]::Create().ComputeHash($bytes)
    return ([BitConverter]::ToString($hash) -replace '-', '').Substring(0, 16)
}

$WM_CLOSE = 0x0010; $WM_HOTKEY = 0x0312; $TRAY = 0x8001; $WM_LBUTTONUP = 0x0202
$work = Join-Path $env:TEMP ('gc-app-check-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$script:held = 0; $script:failed = 0

function Say([bool] $good, [string] $what) {
    if ($good) { $script:held++; Write-Host "  ok    $what" } else { $script:failed++; Write-Host "  FAIL  $what" }
}

function Start-App([string] $name, [string] $settings, [string[]] $more = @()) {
    $data = Join-Path $work "$name\data"
    New-Item -ItemType Directory -Force $data | Out-Null
    Set-Content -Path (Join-Path $data 'settings.json') -Value $settings -Encoding ascii
    $status = Join-Path $work "$name\status.json"
    $process = Start-Process $Program -ArgumentList (@('--data', $data, '--status', $status, '--language', 'en') + $more) -PassThru
    return @{ Process = $process; Status = $status; Data = $data }
}

function Read-Status($app) {
    try { return Get-Content $app.Status -Raw -ErrorAction Stop | ConvertFrom-Json } catch { return $null }
}

# Wait until the app's status says what is asked, and return it; $null if it never does.
function Wait-Status($app, [scriptblock] $done, [int] $seconds = 30) {
    $until = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $until) {
        $status = Read-Status $app
        if ($status -and (& $done $status)) { return $status }
        Start-Sleep -Milliseconds 200
    }
    return $null
}

function Send($window, [int] $message, [int] $w = 0, [int] $l = 0) {
    [GodsCameraCheck.User32]::PostMessageW($window, $message, [IntPtr]$w, [IntPtr]$l) | Out-Null
}

Write-Host 'The window, its icon and its hot keys'
$app = Start-App 'first' '{"source": "pattern"}'
try {
    $live = Wait-Status $app { param($s) $s.mode -eq 'Live' } 60
    Say ($null -ne $live) 'the app starts and shows the test picture'
    if (-not $live) { throw 'the app did not start' }
    $app.Process.Refresh()
    $window = $app.Process.MainWindowHandle
    Say ($window -ne [IntPtr]::Zero) 'it has a window'
    Say ($live.tray_icon) 'it put its icon beside the clock'
    Say ($live.record_key -and $live.loop_key) "Windows gave it both hot keys (record: $($live.record_key), loop: $($live.loop_key))"

    Write-Host 'The hot keys'
    Send $window $WM_HOTKEY 1
    Say ($null -ne (Wait-Status $app { param($s) $s.recording })) 'the record key starts a recording'
    Start-Sleep -Milliseconds 1500
    Send $window $WM_HOTKEY 1
    Say ($null -ne (Wait-Status $app { param($s) -not $s.recording -and $s.has_clip })) 'the same key ends it, and there is a loop'
    Send $window $WM_HOTKEY 2
    Say ($null -ne (Wait-Status $app { param($s) $s.mode -eq 'Looping' })) 'the loop key plays the loop'
    Send $window $WM_HOTKEY 2
    Say ($null -ne (Wait-Status $app { param($s) $s.mode -eq 'Live' })) 'the same key goes back to live'

    Write-Host 'Closing the window'
    Send $window $WM_CLOSE
    $hidden = Wait-Status $app { param($s) $s.window_hidden }
    Say ($null -ne $hidden) 'closing hides the window'
    Say (-not $app.Process.HasExited) 'and the app keeps running'
    Say (-not [GodsCameraCheck.User32]::IsWindowVisible($window)) 'nothing of it is left on the screen'
    $resting = Wait-Status $app { param($s) $s.resting -and $s.mode -eq 'Idle' }
    Say ($null -ne $resting) 'with the window hidden and no app reading the camera, no picture is made'

    Write-Host 'The icon, and a second copy'
    Send $window $TRAY 0 $WM_LBUTTONUP
    $back = Wait-Status $app { param($s) -not $s.window_hidden -and $s.mode -eq 'Live' }
    Say ($null -ne $back) 'a click on the icon brings the window and the picture back'
    Say ([GodsCameraCheck.User32]::IsWindowVisible($window)) 'the window is on the screen again'

    Send $window $WM_CLOSE
    Wait-Status $app { param($s) $s.window_hidden } | Out-Null
    $second = Start-Process $Program -ArgumentList @('--data', $app.Data, '--language', 'en') -PassThru
    $left = $second.WaitForExit(20000)
    if (-not $left) { $second.Kill() }
    Say $left 'a second copy on the same data leaves at once'
    $shown = Wait-Status $app { param($s) -not $s.window_hidden }
    Say ($null -ne $shown) 'and the first copy shows its window'
    Say (-not $app.Process.HasExited) 'the first copy is still the one running'

    Write-Host 'Asked to quit'
    $quit = [GodsCameraCheck.User32]::RegisterWindowMessageW('GodsCamera.Quit.' + (Get-FolderMark $app.Data))
    Send $window $quit
    $ended = $app.Process.WaitForExit(20000)
    Say $ended 'asked to quit, as an installer would ask, the app ends'
    if ($ended) { Say ($app.Process.ExitCode -eq 0) "without an error (exit code $($app.Process.ExitCode))" }
}
finally {
    if (-not $app.Process.HasExited) { $app.Process.Kill() }
}

Write-Host 'A clean end'
$plain = Start-App 'second' '{"source": "pattern", "keep_running": false}'
try {
    $live = Wait-Status $plain { param($s) $s.mode -eq 'Live' } 60
    Say ($null -ne $live) 'a copy that is set not to keep running starts'
    $plain.Process.Refresh()
    Send $plain.Process.MainWindowHandle $WM_CLOSE
    $ended = $plain.Process.WaitForExit(20000)
    Say $ended 'closing its window ends it'
    if ($ended) { Say ($plain.Process.ExitCode -eq 0) "and it ends without an error (exit code $($plain.Process.ExitCode))" }
}
finally {
    if (-not $plain.Process.HasExited) { $plain.Process.Kill() }
}

Write-Host 'The scanner'
$packs = Join-Path (Split-Path -Parent $repo) 'packs'
$scan = Start-App 'scanner' '{"source": "pattern", "keep_running": false}' @('--scanner', 'on', '--packs', $packs)
try {
    $on = Wait-Status $scan { param($s) $s.scanner_running } 60
    Say ($null -ne $on) 'a copy started with its scanner on has a hub running'
    if ($on) {
        Say ($on.scanner_network -eq 'Ready') "its network side is up ($($on.scanner_network))"
        Say ($on.scanner_address -like 'http://127.0.0.1:*') 'and it says where a device on a cable reaches it, on this computer'
        $gc = @('release', 'debug') | ForEach-Object { Join-Path $repo "core\target\$_\gc.exe" } | Where-Object { Test-Path $_ } | Select-Object -First 1
        $token = Join-Path $scan.Data 'Secrets\HubToken.secret'
        Say (Test-Path $token) 'a try-out keeps its token in its own data folder, not in Credential Manager'
        if (-not $on.scanner_can_read) {
            Write-Host '  skip  reading a page: the model files are not in ..\packs (python tools\fetch-packs.py fetches them)'
        } elseif (-not $gc) {
            Write-Host '  skip  reading a page: the core''s tool gc.exe is not built (cargo build -p gc-cli in core)'
        } else {
            # One page with a heading and five lines, drawn for the reader's own tests.
            $shots = Join-Path $work 'scanner\shots'
            $out = Join-Path $work 'scanner\out'
            New-Item -ItemType Directory -Force $shots | Out-Null
            Copy-Item (Join-Path $repo 'core\gc-ocr\tests\data\page.png') $shots
            # The token goes to the tool in its environment, never on its command line.
            $env:GC_HUB_TOKEN = (Get-Content $token -Raw).Trim()
            try {
                & $gc scan $shots --hub $on.scanner_address --out $out | Out-Null
                $sent = ($LASTEXITCODE -eq 0)
            } finally {
                Remove-Item Env:\GC_HUB_TOKEN
            }
            Say $sent 'a page sent to it is taken and read'
            $done = Wait-Status $scan { param($s) $s.documents_done -ge 1 } 60
            Say ($null -ne $done) 'the document appears in the app'
            if ($done) {
                # The heading is sixteen Vietnamese letters and spaces, the first two of them "Ch".
                $title = [string] $done.newest_document
                Say ($title.Length -eq 16 -and $title.StartsWith('Ch')) "under the page's heading ($($title.Length) characters)"
            }
            if ($sent) {
                $text = Get-Content (Join-Path $out 'document.txt') -Raw -Encoding UTF8
                Say ($text -match 'The market opens at 5 a\.m\. and closes before noon\.') 'and the text has the page''s English line, letter for letter'
            }
        }
    }
    $scan.Process.Refresh()
    Send $scan.Process.MainWindowHandle $WM_CLOSE
    $ended = $scan.Process.WaitForExit(20000)
    Say $ended 'closing the window ends the app and its hub'
}
finally {
    if (-not $scan.Process.HasExited) { $scan.Process.Kill() }
}

Write-Host ''
if ($script:failed -eq 0) { Write-Host "All $($script:held) checks held." } else { Write-Host "$($script:failed) of $($script:held + $script:failed) checks FAILED."; exit 1 }
