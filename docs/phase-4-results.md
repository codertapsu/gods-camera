# Phase 4 results so far

Date: 2026-11-12 · Plan: [implementation-plan.md](implementation-plan.md)

Windows work began on 2026-11-12, when you brought the PC in. That is earlier than the plan's start point, so Phase 4 now runs beside what is left of Phase 3. This note covers the first day: the toolchain, the shared core on Windows, the models on the RTX 3050, the virtual camera, and the engine that will feed it.

## What you need to do

1. **Register the camera driver.** This is the one step that needs administrator rights, so I have not done it. Open PowerShell with "Run as administrator", go to the repository folder, and run:

   `powershell -ExecutionPolicy Bypass -File tools\register-windows-camera.ps1`

   It copies one DLL to `C:\Program Files\GodsCamera\` and registers it. Nothing else changes. `-Unregister` undoes it. You can also tell me to run it; this PC grants administrator rights without a prompt, which is why I wait for your word.
2. **Say whether I may turn on your USB camera, or try it yourself.** A camera named "HD Web Camera" was attached during the day. I have not opened it. The app can now use a camera, but that part is tested only with a video file, which comes through the same reader. To try it yourself: start the app, and on the Camera screen click "Allow camera access". If I am to test it, I would turn it on for a few seconds at a time, read frame sizes and rates, and keep no picture.

After step 1, I can check the camera in Windows' own camera service, in Chrome and in Edge without you. Zoom, Teams, Discord, Slack and OBS are not installed on this PC; a check in those needs you.

## The PC

| Part | State |
|---|---|
| Windows | Windows 11 Pro 25H2, build 26200.8655 |
| Hardware | Core i5-13400F, 32 GB, GeForce RTX 3050 with 8 GB, driver 616.92 |
| C++ | Visual Studio Build Tools 2022 (17.14) with the C++ tools, Windows SDK 10.0.26100, CMake, Ninja |
| Rust | 1.99.1, MSVC toolchain |
| Python | 3.12 with uv |
| .NET | SDK 8.0.422. Its support ended on 2026-11-10; the app shell will need .NET 10 |
| Camera | A USB camera was attached during the day. Not used yet; see above |
| Apps for the camera check | Chrome and Edge are installed. Zoom, OBS, Discord and Slack are not |

Your account is an administrator, and this PC approves administrator requests without asking. My sessions still run without those rights, and I do not use them unless you say so.

## The shared core on Windows

The core had only ever been cross-compiled for Windows from the Mac. It now builds and runs here.

- **Tests.** 224 pass in a full run on the PC. Four more are run by hand: three print speeds, and one listens on every network interface (see the firewall note below).
- **Same pictures as the Mac.** The parity tests compare the Rust pipeline with InsightFace's own on its sample photo, and pass here. The reference numbers made on this PC differ from the Mac's by at most 0.19 pixels for a detected keypoint, which is ordinary difference between processors.
- **What had to change for Windows.** The hub's tests used a shell script as their stand-in text reader; they now use PowerShell here. Development builds optimise the tracker, the image code, the live engine and the camera pipeline, because unoptimised they are seven times slower on this processor and the tests that time them failed for that reason alone.

**The firewall alert you saw.** Three kinds of test program listened on every network interface, so Windows Firewall asked about each of them, and a test program gets a new name with every build. You could not answer the alert, and Windows kept a block rule for each program, for public networks only. The tests now listen on this PC alone, and a full test run leaves the firewall's rules untouched. Fourteen old block rules remain. They do no harm: they name test programs that no longer exist in that form, and this PC's network is marked private.

## Spike S3: the models on the RTX 3050

Time for one run of each model, alone, in milliseconds. "Off" is how far the output is from the full-precision result on the processor, in levels out of 255 on a real face crop.

| Model | Processor | DirectML | CUDA | Picture through DirectML |
|---|---|---|---|---|
| `inswapper_128`, as downloaded | 568 | 58 | 58 | Right: 0.2 off InsightFace's own crop |
| `inswapper_128`, half precision | 564 | 18 | 37 | **Wrong: 14 off on average, 133 at worst** |
| `inswapper_128`, mixed precision | 602 | **22** | 40 | Right: 0.3 off |
| HyperSwap 256, as downloaded | 275 | 18 | 26 | **Wrong: 7 off on average, 126 at worst** |
| HyperSwap 256, mixed precision | 285 | **21** | 28 | Right: 0.05 off |
| Detector `det_10g` at 640 | 48 | 10 | 12 | Same faces, within a pixel |
| Identity encoder | 51 | 7 | 7 | Same direction, cosine above 0.9995 |
| Occlusion mask | 76 | 17 | 28 | Same mask |

What this means:

- **Half precision is fast and wrong through DirectML.** The same half-precision file is right on this PC's processor and on the Mac's Neural Engine. On the card, the normalisation inside the model loses too much in 16 bits. HyperSwap is stored in half precision, so the file as downloaded is affected too. Speed alone would have made both look fine; the picture check caught it.
- **Mixed precision fixes it.** A copy that keeps the normalisation in full precision and the rest in half is as fast and gives the right picture. `tools/modelkit/mixed_precision.py` makes the copies, the manifests pin their hashes, and `tools/fetch-packs.py` makes them on any machine.
- **DirectML is the provider.** It is one 18 MB library that works on any DirectX 12 card. NVIDIA's CUDA provider was no faster on this card for any model, and needs about 2 GB of libraries bundled. TensorRT did not load in this quick comparison; I did not pursue it, because DirectML already meets the target. It stays an option if a later feature needs more speed.
- **A guard against going back.** A Windows test compares every model on the card with the reference pictures.

Video memory: about 0.4 GB for `inswapper_128` mixed, 0.75 GB for HyperSwap, and 0.14 GB each for the detector and the occlusion mask. The card has 8 GB.

## The live engine on the PC

`gc replay`, 1280×720 at 30 frames a second for 10 seconds, face moving as in ordinary talking.

| Measure | `inswapper_128` | HyperSwap 256 | Without the occlusion mask |
|---|---|---|---|
| Time for one swap | 39 ms | 40 ms | 24 ms |
| Swaps a second | 25 | 25 | 30 |
| From a frame's arrival to its swapped face | 59 ms | 60 ms | 26 ms |
| Age of the face on screen, median and 95th percentile | 2 and 3 frames | 2 and 3 frames | 1 and 2 frames |
| Detector, on the card | 28 ms | 27 ms | 24 ms |
| Frame thread, median | 2.4 ms | 3.1 ms | 2.5 ms |
| Keypoint wobble | 1.2 px | 1.2 px | 1.2 px |

- **The target is met.** The plan asks for 720p at 30 frames a second with the swap on. Output is always 30; the face refreshes 25 times a second with the occlusion mask and 30 without.
- **One card does everything here.** The Mac spreads the models over its processor, graphics chip and Neural Engine. The PC has one accelerator, so the swap model, the detector and the occlusion mask take turns on it. That is why the mask costs 15 ms a swap here and 1 ms on the Mac. Making the mask every second swap would win most of it back; not done yet.
- **The frame thread is over its 2 ms target** on this processor, at 2.4 to 3.1 ms. It has 33 ms, so nothing is dropped; it is noted, not fixed.
- **The large detector runs on the card** on Windows: 10 ms against 48 ms on this processor.

**An improvement found here that the Mac gets too.** The first time a face is seen, the detector's answer is for a frame that has already passed, and nothing was being followed yet to carry it forward. The engine took the face up where it had been, and later detections only nudge by a tenth, so the swap sat slightly off for a while. A slow first detection made it obvious on the PC. The engine now follows the face from the first detection without showing it, looks again at once, and takes that second detection whole. On the test clip the keypoint offset fell from 4.5 to 2.4 pixels and the wobble from 1.04 to 0.47.

## Spike S8: how the outgoing picture is drawn

The plan asked for a test of one shared graphics-card renderer for Windows and Android against a native Direct3D one. I decided it differently: on Windows the outgoing picture is drawn on the processor, by a new crate, `gc-render`.

| Picture | 1280×720 | 1920×1080 |
|---|---|---|
| Plain | 1.1 ms | 3.0 ms |
| With the swapped face | 3.2 ms | 5.2 ms |
| Blurred, with the face | 4.5 ms | 7.9 ms |
| A cross-fade of two pictures | 4.6 ms | 8.2 ms |
| With the face, from a camera of the other size | 5.1 ms | 10.2 ms |

One thread, with the conversion to the camera's format included. A frame has 33 ms.

Why:

- The camera wants its pictures in ordinary memory, to write them to a pipe. A picture drawn on the card would have to be read back first.
- The card is the one accelerator on this PC, and the models already take turns on it. Drawing there would join the queue.
- The same code runs and is tested on both machines, with no graphics driver in the way.

It can be revisited for Android, where a phone's processor is weaker. Nothing in the engine depends on where the picture is drawn.

## Spike S2: the virtual camera

Windows 11 can show a camera that a program provides. The program supplies a "media source", a DLL that Windows loads into its own camera service. The camera then appears in every app's list for as long as our app runs.

What is built, in `apps/windows`:

- **The media source**, `GodsCameraSource.dll`. It offers 1280×720 and 1920×1080 at 30 frames a second, as NV12 and as YUY2.
- **The way pictures reach it.** The camera service runs as a different account, so it cannot see the app's memory. The app owns a named pipe, and the media source connects to it when an app starts reading the camera. A connected pipe is also how the app knows that something is watching.
- **A placeholder picture.** If the app sends nothing for half a second, the camera shows its own slate instead of freezing. A row of small squares on the slate says why, so a problem inside the camera service can be read from the picture.
- **Two test tools.** One stands in for the app and sends a test picture. The other reads the camera as a meeting app would and reports what arrived.

What is measured, with the DLL loaded into the test tool instead of the camera service:

| Check | Result |
|---|---|
| 1280×720, 20 seconds | 601 of 601 pictures in order, none repeated. From the app making a picture to the reader having it: 0.7 to 0.9 ms at the median, 1.3 to 3.1 ms at the 95th percentile |
| 1920×1080, 20 seconds, six runs | 601 of 601 each time. 1.5 to 2.7 ms at the median, 2.2 to 5.0 ms at the 95th percentile |
| The app stops sending | The last picture repeats for 450 ms, then the slate. Pictures return about 30 ms after the app is back |
| No app at all | The slate from 0.3 seconds on, 30 a second |
| Every call the camera service makes | 39 of 39 steps answered correctly |
| The whole check script | 57 of 57 checks, in 85 seconds. I ran it again myself: the same |
| **The face swap through it** | 450 of 450 pictures in 15 seconds, none repeated, no slate, with the engine below as the sender |

The plan asked to compare shared memory with a shared texture for the hand-off. The pipe copies each picture four times and still takes 1 to 3 ms, so I did not build either. Shared memory would copy once; it is there if a 4K picture ever needs it.

**Not known until the driver is registered:**

- Whether the camera service's account can open the pipe. The pipe's permissions name it, but only the real service can show that they are right.
- Whether the pipe's name reaches the media source inside the service.
- Whether the Camera app, Chrome, Edge and Teams accept the camera.
- Whether any app loads the media source into its own process as well. The pipe takes one reader. A second one would get the slate, and the slate would say so.

Safety, as built:

- Only you, and the camera service, can open the pipe. Nobody on the network can.
- The media source opens no pipe but yours. It checks the name's form and who owns the pipe, so another account on the PC cannot feed your camera.
- The DLL is loaded from `C:\Program Files`, which only an administrator can change.
- One gap is left: another user of the same PC could write a program that reads your pipe while the app is sending. A secret that the app hands to its own camera would close it. On a PC with one user it does not matter.

## The engine that feeds the camera

The Mac app does this work in Swift. For Windows it is written in Rust, in three new crates, so that it can be tested without a window and behaves like the Mac's.

- **`gc-win`** is the app's end of the camera: the pipe, and the call that asks Windows to show the camera.
- **`gc-camera`** is the pipeline. It takes frames from a source, runs the face swap, draws the outgoing picture with the "AI" badge, and hands it to the camera in the size the reading app asked for. It records the outgoing picture as a clip and plays a clip as a loop.
- **`gc-app`** will hold what the screens show and do. Saved faces are in it so far.

The loop follows the Mac's rules:

- Where it starts and ends is chosen so that the jump is hardest to see.
- A loop that ends as it began plays start to end with a cross-fade. One where you moved plays back and forth.
- Going from live to the loop and back is a fade over eight frames, and the fade never shows the unswapped face.

Recorded clips are kept as one JPEG per frame, so any frame can be shown without the ones before it. Five seconds of the test picture take 7 MB at 1280×720 and 13 MB at 1920×1080. A room with more detail in it will take more, up to about 30 MB.

`gc camera` is a command-line tool that does what the app will do, with a moving-face clip in place of a camera. With `--loop-after 2` it goes live, records five seconds and plays them as a loop. Measured through the real media source over 14 seconds of live, recording and loop, face swap on:

| Measure | Asked for 1280×720 | Asked for 1920×1080 |
|---|---|---|
| Pictures made, and received by the reader | 421 and 421 | 420 and 420 |
| Repeated, missing or placeholder pictures | 0 | 0 |
| Time between two pictures: median, 95th percentile, longest | 33.3, 34.5, 36.9 ms | 33.4, 35.0, 38.2 ms |
| Swapped face | Renewed 25 times a second, 61 ms behind the frame | The same |

Drawing one picture takes 4.4 ms at 1280×720, and handing a frame to the engine 0.4 ms. The camera in the clip is 1280×720; for a reader that asks for 1920×1080 the engine enlarges it.

Two faults showed in these runs and are fixed:

- **Enlarging was too slow.** Showing a 1280×720 camera at 1920×1080 took over 30 ms a picture, so every third picture was passed over. It now takes 10 ms. A camera of one size and a meeting app asking for another is an everyday case.
- **The loop's timing was coarse.** A loop's pictures were up to 15 ms late on Windows, and the camera filled the gaps by repeating 22 pictures in seven seconds. The loop now has a timer of its own that is exact to a fraction of a millisecond.

A test also reads 330 pictures across live, recording and loop with a stand-in for the media source: one every 33 ms throughout, none missing, no pause at either change. I looked at a live picture and a loop picture from that run: the swapped face is clean, and the "AI" badge is in the corner of both.

Saved faces are encrypted on disk, as on the Mac. The key is kept apart from the files, in Windows' Credential Manager, where only your account can read it.

## The app's window

The window is WinUI 3 in C#, as planned, with the Rust core beside it as one DLL. What each screen shows and does is in the core (`gc-app`), with tests; the C# only draws it.

| Screen | State |
|---|---|
| Camera | Works: the live picture, where it comes from (a camera, a picture file, a video file or the test picture), mirroring, and whether other apps can see the camera. A camera stays off until you click "Allow camera access" |
| Loop | Works: record, play, how it repeats, the LOOP badge, save, saved loops, the warning when the room's light has changed |
| Face | Works: add a face from photos, the question whose face it is, choose, rename, remove a photo, delete, the swap's switch with its rules shown first, quality, the options, the "AI" badge |
| Scanner | A placeholder. It waits for text reading on Windows |
| Settings | The camera's state, "Delete all my data", and the folders, under "For development" |

- **Checked by looking.** `tools\snapshot-windows-app.ps1 -Demo` starts the app on a data folder of its own, records and plays a loop, adds the sample photo as a face through the same code the buttons call, turns the swap on, and saves a picture of every screen. I looked at them in English and Vietnamese, light and dark. On the Face screen the swap ran on the graphics card: a new face every 37 ms, 54 ms behind the picture.
- **Vietnamese.** The window uses the Mac app's translations, plus 23 texts that only Windows has. The build fails if a text has no Vietnamese.
- **Not checked.** I pressed no button with a mouse. The pictures show that the screens draw and that the model behind them works; whether every button is wired to the right call is for you to try, or for a test that clicks.
- **A camera or a video file as the source.** Windows decodes either and hands the app one picture at a time. A test writes a short film, reads it back and checks that it is the right way up, has the right colours, plays at 30 pictures a second and starts again at its end. I also started the real app with that film as its source and looked at the picture: it plays. The camera goes through the same reader; it has not been turned on. Of the modes a camera offers, the app takes the one nearest to 1280×720 at 30 a second.
- **Not in the window yet:** the tray icon, hot keys, going back to live when you start to speak, and the installer.

For the window I installed two tools for your user account only, without administrator rights: the .NET 10 SDK, and the program that writes the C# description of the core. `tools\build-windows-app.ps1` builds everything.

## Open

- **Registration**, then the check in Windows' camera service and in real apps. See the top of this note.
- **A test with the real USB camera**, once you say I may turn it on or have tried it yourself.
- **The rest of the Windows app.** The tray icon, hot keys, the hub with a firewall rule made by the installer, the scanner, and the installer that registers the camera.
- Text reading on Windows (PP-OCRv5) and its Vietnamese accuracy.
- The occlusion mask at half rate on machines with one accelerator.
- The core changes made here still have to pass `tools/check.sh` on the Mac.
