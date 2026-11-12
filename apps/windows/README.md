# God's Camera on Windows

The virtual camera, as far as spike S2 has taken it: a camera named "God's Camera" that Windows 11 offers to every app, and two command-line tools that test it. The app itself is not here yet.

Windows 11 only. `MFCreateVirtualCamera` does not exist before it.

## The pieces

| Path | Builds | What it is |
|---|---|---|
| `vcam/` | `GodsCameraSource.dll` | The media source. Windows loads it into its camera service (Frame Server) and asks it for pictures. It fetches them from the app |
| `vcam-client/` | `gc_vcam_client.lib` | The app's end, as a small C library: makes the pipe, learns what size is wanted, sends pictures, says whether anything is watching |
| `vcam-host/` | `gc-vcam-host.exe` | Stands in for the app. Makes the pipe and the camera and sends a test picture |
| `vcam-probe/` | `gc-vcam-probe.exe` | Reads the camera as a meeting app would and reports what arrived, as one line of JSON |
| `shared/` | | The contract between the two ends (`gc_vcam_protocol.h`), the test picture and the slate |
| `tests/check.ps1` | | Every check that can be made without registering the camera, in one run |

The same contract is repeated for Rust in `core/gc-win`; a test there compares it with `shared/gc_vcam_protocol.h`.

## How a picture gets to a meeting app

The camera service runs as the account LOCAL SERVICE, in a session of its own. It cannot see the app's windows or its memory. So the app owns a named pipe, `\\.\pipe\godscamera.frames.<the user's SID>`, and the media source is its client:

1. An app starts reading the camera. The camera service starts the media source's stream.
2. The source opens the app's pipe and says which size it wants (`gc_vcam_request`). For the app, a connected pipe means that something is watching.
3. The app writes one `gc_vcam_frame_header` and the NV12 picture after it, again and again. If a picture is not written before the next is ready, the older one is dropped.
4. The source keeps the newest picture and hands it on. If the next one is more than half a frame late, the last one goes out again. If none came for half a second, or the pipe cannot be opened at all, the source sends its own slate: a dark picture with a light ring and a stroke through it. A reading app never waits for the app.
5. The last reader stops. The source closes the pipe, and the app knows that nothing is watching.

The camera offers 1280 x 720 and 1920 x 1080 at 30 frames a second, as NV12 and as YUY2. The app always sends NV12; the source makes YUY2 from it for the older programs that ask.

How the source learns the pipe's name: the app sets it as a string attribute on the virtual camera (`GC_VCAM_ATTRIBUTE_PIPE_NAME`) before starting it. Without the attribute the source uses the pipe of the account it runs as, which is right when a test loads the DLL into its own process.

The slate says why it is showing. Nothing the source writes inside the camera service can be read without an administrator's tools, but its picture always gets out. So a row of 24 small, dim squares along the slate's bottom edge carries the cause, the Windows error code that goes with it, and whether the pipe's name came from the app. The probe reads them back as `why`, `error` and `pipe_named_by_app` in its `timeline`. The causes: the app's pipe does not exist; Windows refused to open it; it is busy with another reader; it could not be opened for another reason; it does not belong to the user in its name; connected, and the app sends nothing; connected, and the app sends another size than asked for.

## Build

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-windows.ps1
```

Needs the Visual Studio 2022 Build Tools with the C++ tools, CMake and Ninja; no administrator. Everything goes to `apps\windows\build\bin`. The build fails on any compiler warning (`/W4 /WX`). The C++ runtime is linked into each binary, so the DLL needs nothing but Windows inside the camera service.

## Test without registering anything

One command runs everything below and says what held:

```powershell
powershell -ExecutionPolicy Bypass -File apps\windows\tests\check.ps1
```

It takes a minute and a half, needs no administrator and changes nothing on the computer. It refuses to run while the app or a test host holds the camera's pipe. The rest of this section is what it does, by hand.

The probe can load the DLL into its own process. That tests the media source, the pipe and the pacing, and needs no administrator. It does not test the camera service.

```powershell
cd apps\windows\build\bin
Start-Process .\gc-vcam-host.exe -ArgumentList '--no-camera --seconds 30'
.\gc-vcam-probe.exe --in-process .\GodsCameraSource.dll --seconds 20 --size 1280x720
```

A good result has `"ok":true`, about 30 `samples_per_second`, `frames.gaps` and `frames.repeats` at 0, `slates` at 0, `colours.wrong` at 0, and every entry of `source_checks` true. `delay_ms` is the time from the host making a picture to the probe receiving it.

Other things to try:

- No host at all: every sample is a slate, the first after 0.3 seconds, and its `why` is "the app's pipe does not exist".
- Stop the host while the probe runs, then start it again: `timeline` shows frames, then the slate half a second after the last picture, then frames again. `frames.restarts` counts the new host.
- `--size 1920x1080` and `--format YUY2` on the probe.
- `--size 1920x1080` on the host while the probe asks for 1280 x 720: the source refuses a size it did not ask for and shows the slate, with the `why` to match.
- `--pipe-attribute user` on the probe hands the source this user's pipe name the way the app does through the camera, and the slate's `pipe_named_by_app` turns true. Any other text after `--pipe-attribute` shows that the source takes no name but the prefix followed by a SID: it never opens another pipe, a file or a path on another computer.

A reader does not make every call the camera service makes. The second test does:

```powershell
.\gc-vcam-probe.exe --drive .\GodsCameraSource.dll
```

It hands the source an allocator, starts it, pauses and stops the stream alone, asks for another size and another format while pictures flow, stops it while holding pictures, starts it again and shuts it down, and checks each answer. A good result has `"ok":true` and `"failed":0`; each step is listed. It passes with the host running (pictures) and without (slates).

`gc-vcam-host.exe` options: `--seconds N`, `--no-camera` (the pipe only), `--size WxH` (always send this size instead of the size asked for). `gc-vcam-probe.exe` options: `--in-process <dll>`, `--seconds N`, `--size WxH`, `--format NV12|YUY2`, `--pipe-attribute user|<name>`; or `--drive <dll>`, with `--pipe-attribute` if wanted.

## Register the camera: the one step that needs an administrator

The camera service only looks for media sources in the part of the registry that belongs to the whole computer, and must be able to read the DLL. In a PowerShell window opened with "Run as administrator":

```powershell
powershell -ExecutionPolicy Bypass -File tools\register-windows-camera.ps1
```

It copies the built DLL to `C:\Program Files\GodsCamera\GodsCameraSource.dll` and registers it there with `regsvr32`, which writes `HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\{1E1EB430-08A7-42F4-9F7D-C53A00B36024}`. Nothing else is changed. If an older copy of the DLL is in use, it stops the two Windows camera services for a moment; they start again by themselves when an app next uses a camera. After a rebuild, run it again. To undo everything:

```powershell
powershell -ExecutionPolicy Bypass -File tools\register-windows-camera.ps1 -Unregister
```

Registering does not make a camera appear. The camera exists while the app, or the test host, is running:

```powershell
cd apps\windows\build\bin
.\gc-vcam-host.exe                      # no administrator; Ctrl+C ends it and the camera goes away
.\gc-vcam-probe.exe --seconds 10        # in a second window: finds the camera by name and reads it
```

With the host running, the camera "God's Camera" should be in the camera list of the Windows Camera app, Chrome, Edge, Teams and the rest, showing colour bars, a white square that moves, and two rows of squares along the top. Windows may add words of its own to the name. The host prints a line each time something starts or stops watching.

If an app shows the slate instead of the bars, the probe says why, read from the slate itself:

- "Windows refused to open the app's pipe": the camera service's account may not open it.
- "the app's pipe does not exist" with `pipe_named_by_app` false: the pipe's name did not arrive with the camera.
- "the app's pipe is busy with another reader": something else is reading the pipe; see the last section.

Before registration the host ends at once with `"camera_hresult":"0x80040154"`: Windows knows no such class.

## What crosses the pipe

Both structures are packed and little-endian. `shared/gc_vcam_protocol.h` is the source of truth.

Source to app, once after connecting (28 bytes):

| Field | Type | Meaning |
|---|---|---|
| `magic` | u32 | `0x52564347`, "GCVR" |
| `version` | u16 | 1 |
| `size` | u16 | bytes in this structure |
| `width`, `height` | u32, u32 | the picture size wanted |
| `format` | u32 | `0x3231564E`, "NV12" |
| `frame_rate_num`, `frame_rate_den` | u32, u32 | frames a second, as a fraction |

App to source, before every picture (40 bytes), followed at once by the picture: `width * height` bytes of luma, then `width * height / 2` bytes of interleaved chroma, no padding:

| Field | Type | Meaning |
|---|---|---|
| `magic` | u32 | `0x46564347`, "GCVF" |
| `version` | u16 | 1 |
| `header_size` | u16 | bytes in this structure; a longer header from a later version is skipped over |
| `width`, `height` | u32, u32 | the picture's size; must be what was asked for |
| `format` | u32 | "NV12" |
| `payload_size` | u32 | bytes of picture that follow |
| `sequence` | u64 | goes up by one with every picture; never 0 |
| `time_100ns` | i64 | when the picture was made: the performance counter in units of 100 nanoseconds |

## Who may do what

- The pipe is made with the first-instance flag, so the app fails loudly if another program already holds the name. It accepts no connection from another computer.
- The pipe belongs to the user. The user has full access. LOCAL SERVICE, the camera service's own identity and the system account may read and write it, and may not make further instances of it. Nobody else may open it.
- The source takes a pipe name only if it is exactly the prefix followed by a SID, opens it so that the pipe's owner cannot act in the service's name, and checks that the pipe's owner is the user the name says. Another account cannot put a pipe in the way and feed the camera its own pictures.
- The DLL is loaded from `C:\Program Files\GodsCamera`, which only an administrator can write to.
- The source writes a few lines to the debugger output (`OutputDebugString`, prefix `[GodsCamera]`) when its state changes. Never a picture, and never who the user is.

## Not done, and not yet known

- **The camera service has not loaded the DLL yet.** Everything above was run with the DLL in the probe's own process. Whether LOCAL SERVICE can open the user's pipe, whether the pipe-name attribute arrives, and whether Camera, Chrome, Edge, Teams, Zoom and OBS accept the camera are known only after the registration step.
- One reader of the pipe at a time. The camera service shares one media source among all apps, so that is enough if the service is the only one that starts the stream. If some app turns out to start the source in its own process as well, the second one gets the slate.
- Another user of the same PC who writes a program for it could make a camera of their own that reads this user's pipe while this user's app is sending. A secret that the app hands to its own camera would close that. For a PC with one user it does not matter.
- No scaling in the source: a picture of another size than the one asked for is refused, and the slate shows.
- The pipe copies each picture four times on its way; the whole trip takes 1.5 to 3 ms at 1080p on this PC. Shared memory would copy it once; the measurement did not call for it.
- The row of squares on the slate is there for this stage of the work. The app can keep it or drop it; it is one function in `shared/gc_slate.h`.
- The camera lives only while the host runs (`MFVirtualCameraLifetime_Session`). A camera that stays after the app closes, and shows the slate, is a choice for the app.
- No installer, and the DLL is not signed.
