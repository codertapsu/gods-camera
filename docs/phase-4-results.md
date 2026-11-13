# Phase 4 results so far

Date: 2026-11-12 · Plan: [implementation-plan.md](implementation-plan.md)

Windows work began on 2026-11-12, when you brought the PC in. That is earlier than the plan's start point, so Phase 4 now runs beside what is left of Phase 3. This note covers the first day: the toolchain, the shared core on Windows, the models on the RTX 3050, the virtual camera, the engine that feeds it, the app's window, the reader that turns photos of a page into text, and the scanner that uses it.

## What you did, and what it showed

You installed the camera driver, allowed the app to use your USB camera, and ran the Mac's checks. On the PC I verified the first two on 2026-11-12, with your app running:

| Check | Result |
|---|---|
| The driver's registration | Right: the class points at `C:\Program Files\GodsCamera\GodsCameraSource.dll`, and that file is the one that was built |
| Windows' camera service, read as a meeting app reads it | "God's Camera (Windows Virtual Camera)" is listed. 301 pictures in 10 seconds, all from your app, no placeholder pictures. The first picture came after 8 ms |
| Chrome 157 | Lists the camera and gets 1280×720 at 30 a second: 121 pictures in 4 seconds |
| Edge 157 | The same |
| Your USB camera as the source | Works in your running app: the pictures above are its pictures. I did not open the camera myself and kept no picture |
| Two programs reading at once | The second gets "device in use" from Windows. That is how Windows treats most cameras; it is not our placeholder picture |

This answers what spike S2 could not know before: the camera service's account can open the app's pipe, the pipe's name reaches the driver, and browsers accept the camera.

Still open here:

- **The browsers were run without a window**, on a profile folder of their own. Meet in a normal Chrome window, the Windows Camera app, and Zoom or Teams, are for you to try; the last two are not installed on this PC.
- **Uneven timing with the real camera.** Pictures arrived 32 ms apart at the median and 48 ms at the 95th percentile. With the test clip it was 33 and 35. Either the camera delivers unevenly, which dim light causes, or the app adds it. The preview's "pictures a second" tells which; the app writes no log yet that I could read it from.
- **The Mac's checks pass**, since 2026-11-13. When you ran them, `tools/check.sh` stopped in the new text reader's tests, and `swift test` found no package because it has to be run in `apps/apple/GodsCameraKit`; my instruction did not say so. With the permission you gave this PC, I then ran them on the Mac myself, in a work folder of its own at the PC's code: `tools/check.sh` passes (244 tests, the lint, and the builds for Android and Windows), `tools/build-core-apple.sh` builds the framework, and `swift test` passes all 47 tests. Why the reader's tests failed is under "Reading text". Your own checkout on the Mac was not touched: it needs `git pull`, and the Mac app has not been rebuilt or tried with this code.
- **Found on the Mac while looking.** The same thing that tripped the reader is true of the Mac's face detector and of the model that reads a face's identity. The code opens them "on the processor", and on a Mac the inference library then runs them through Core ML instead, in half precision. That is faster than the processor would be: 7 ms against 25 ms for the large detector, 2 ms against 12 ms for the identity model, measured on your Mac while it was busy. So the Mac's live picture has always run this way, and its measurements were taken this way. I changed nothing there, corrected the comments that said otherwise, and left a note for the Mac's session to decide with the app at hand.

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

- **Tests.** 264 pass in a full run on the PC. Four more are run by hand: three print speeds, and one listens on every network interface (see the firewall note below).
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

**What registration showed** (see the top of this note): the camera service's account opens the pipe, the pipe's name reaches the media source, and Chrome and Edge accept the camera. A second program that opens the camera while one reads it is turned away by Windows itself, so the pipe's one reader is enough.

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
| Settings | The camera's state, "Remove the camera", "Delete all my data", and the folders, under "For development" |

- **Checked by looking.** `tools\snapshot-windows-app.ps1 -Demo` starts the app on a data folder of its own, records and plays a loop, adds the sample photo as a face through the same code the buttons call, turns the swap on, and saves a picture of every screen. I looked at them in English and Vietnamese, light and dark. On the Face screen the swap ran on the graphics card: a new face every 37 ms, 54 ms behind the picture.
- **Vietnamese.** The window uses the Mac app's translations, plus 23 texts that only Windows has. The build fails if a text has no Vietnamese.
- **Not checked.** I pressed no button with a mouse. The pictures show that the screens draw and that the model behind them works; whether every button is wired to the right call is for you to try, or for a test that clicks.
- **A camera or a video file as the source.** Windows decodes either and hands the app one picture at a time. A test writes a short film, reads it back and checks that it is the right way up, has the right colours, plays at 30 pictures a second and starts again at its end. I also started the real app with that film as its source and looked at the picture: it plays. The camera goes through the same reader; it has not been turned on. Of the modes a camera offers, the app takes the one nearest to 1280×720 at 30 a second.
- **Beside the clock.** Closing the window leaves the app running with an icon in the notification area, so other apps keep their camera; its menu opens the window, records, plays the loop and quits. While the window is hidden and no app reads God's Camera, no picture is made and your own camera is switched off; it comes back by itself when an app starts reading. A setting turns this off, and closing then ends the app.
- **Hot keys.** Ctrl+Alt+Shift+R records or stops, Ctrl+Alt+Shift+L plays the loop or goes back to live, also while another app has the keyboard.
- **One copy at a time.** Starting the app again brings the running copy's window forward.
- **A log.** `Logs\gods-camera.log` in the app's data folder holds what the app did and what went wrong, never a picture or a file name. It found its first fault on the day it was added.
- **Checked without a person.** `apps\windows\tests\check-app.ps1` starts the app on a data folder of its own and checks 22 things about the above by sending the window the messages a hot key, a click on the icon and a second copy cause. All hold. No key was pressed and no mouse moved: pressing the hot keys and using the icon's menu are for you to try.
- **Not in the window yet:** going back to live when you start to speak, the diagnostics report, and the installer. The Scanner screen has a section of its own below.
- **The model files are not inside the app yet.** The installer will put them there. Until then, the build script's `-Run` points the app at the ones beside the repository.

For the window I installed two tools for your user account only, without administrator rights: the .NET 10 SDK, and the program that writes the C# description of the core. `tools\build-windows-app.ps1` builds everything.

## Reading text

The scanner needs a reader that turns a photo of a page into lines of text. The plan named PP-OCRv5 for Windows. It cannot write Vietnamese, so the reader is built from two other models.

**What was found**

- **PP-OCR cannot write Vietnamese.** Its "Latin" recogniser lists Vietnamese among its languages, but its character list has none of the 90 Vietnamese letters with a tone mark that Unicode keeps from U+1EA0 to U+1EF9, such as ạ, ấ, ề and ỹ. Version 6, published in June 2026, lacks 88 of them. I read the lists in the published model files, not the papers.
- **Windows' own reader has no Vietnamese** on this PC, only English.
- **Four recognisers were tried** on drawn lines of the test page's text in Times New Roman:

| Recogniser | Characters right | Note |
|---|---|---|
| VietOCR, sequence model | 97.7 % | Drops tone marks. It squeezes every line to one width, so a line of 80 letters reads at 85 % |
| VietOCR, transformer | 97.9 % | Writes capital letters small. Four times slower |
| Tesseract 5 with its Vietnamese data | 99.3 % | |
| AsterNet | 99.5 % | A Vietnamese recogniser published in October 2026 under the MIT licence |

- **On ten fonts**, drawn clean and as a phone would photograph them, Tesseract read 99.2 % and 99.4 % of the characters right, and AsterNet 99.4 % and 99.6 %. Georgia was left out: on this PC it has no Vietnamese letters and draws boxes.
- **On photographed test documents the two part ways.** With the same line finder in front, Tesseract got 19 to 38 of a Vietnamese document's 398 words wrong, and AsterNet 3 to 6.

**What is built**

- **`gc-ocr`**, a program in the shared core. It takes the same arguments and prints the same thing as the Mac's reader, so the hub uses either. PP-OCRv6's small detector (Apache-2.0) finds the lines, in any language, and AsterNet reads each one. Both run on the processor through the inference library the app already carries, so nothing new has to be bundled but the two model files.
- **The models** are in the pack manifests, 52 MB together, each at a fixed revision with its hash. `python tools/fetch-packs.py` fetches them.
- **`tools/ocrkit`** has the same steps in Python. That is where the models were chosen and the settings measured, and it holds the comparison above.
- **A line too wide for the recogniser** is cut between words and read in pieces. AsterNet takes a line up to 32 times as wide as it is high; a line across a whole A4 page is about that.
- **Two kinds of nonsense are dropped**: the sliver of a line that a photo's top or bottom edge leaves over, and a shadow taken for text, which the recogniser reads with little confidence.
- **A speckled scan is cleaned before it is read.** This came from your tablet's first page; see "The first real page" below.

**Measured** on the seven test documents of Phase 3, made again on the PC with the same generator. The last column is what the Mac's reader scored on its copies:

| Page | Shots | Words wrong | Characters right | The Mac's reader |
|---|---|---|---|---|
| Vietnamese, three columns | 6, in a 3 × 2 grid | 4 of 398 | 99.77 % | 99.94 % |
| Vietnamese, two columns | 8, in a 4 × 2 grid | 5 | 99.72 % | 99.94 % |
| Vietnamese, three columns | 9, in a 3 × 3 grid | 3 | 99.66 % | 99.72 % |
| Vietnamese, one wide column | 5, in a strip | 6 | 99.66 % | 99.72 % |
| English, two columns | 4, in a 2 × 2 grid | 0 of 248 | 100 % | 100 % |
| English, three columns | 6, in a 3 × 2 grid | 1 | 99.93 % | 100 % |
| English, two columns, every shot narrower than a column | 9, in a 3 × 3 grid | 1 | 99.93 % | 100 % |

The target is 98 %.

- **What is still wrong.** Most slips are a full stop in the middle of a line read as a comma. The rest are one tone mark each: "đê" read as "để", "lẩu" as "lầu".
- **One page does not come out as one document**: the three-column Vietnamese page in a 3 × 3 grid. Its bottom row of shots shows only the last line or two of text, too little to place them. The text is right all the same, because lines read twice are dropped. The Mac has the same weak spot with this generator.
- **Through the hub.** `gc hub` on this PC with `gc-ocr` as its reader, and `gc scan` sending the shots to it: the six-shot Vietnamese document came back with the same 4 words wrong, and the four-shot English one with none.
- **Speed.** About 1 second a photo: 0.45 s to find the lines and 0.03 s to read each, on 4 processor threads, plus 1 s to start. A speckled scan takes 0.7 to 1 s longer. The six-shot document takes 6 seconds, where the Mac takes 1. Another project's build was running on the PC while I measured, so a quiet machine may do better. Not tried yet: finding the lines on the graphics card, reading several lines at once, and keeping the models loaded between photos.
- **Tests.** 33 for the reader: 25 on its own steps and 8 on the program as a whole, six of those with the real models on three small pictures in the repository.

**The first real page, and what it changed**

On 2026-11-13 your tablet sent one page of a printed book through your own app. It read badly, and the reader now does one thing more because of it.

- **The picture.** The tablet's scanner had turned the page to black and white, and the paper was sprinkled with small grey specks. One pixel in eighteen stood alone against its neighbours. In a photo, fewer than one in two thousand do.
- **Read as it came: 92.97 % of the characters right**, 108 of the page's 456 words wrong. The specks were read as full stops, commas and tone marks. I typed the page out myself to have something to compare with.
- **What the reader does now.** It counts the pixels that stand alone. When they are 2 % of the picture or more, and the lines are at least 32 pixels high, it blurs the picture by a fiftieth of the line height and stretches its brightness, so that the faint grey left of a speck turns white and the strokes turn black again. It then finds the lines a second time, in the cleaned picture, and reads them from it.
- **Read that way: 98.55 %**, 28 words wrong. The target is 98 %. What is left is what the generator's pages show too: a full stop read as a comma, a tone mark, a d for a đ.
- **Photos are left alone.** None of the 47 shots of the seven test documents counts as speckled, and all seven documents read exactly as before. The same cleaning would cost a clean photo 2 to 5 characters in a thousand, which is why it is not done to every picture.
- **One page is one page.** To have more than one, I made 18 imitations of such a scan from three of the generator's pages: three kinds of speckle, lighter and heavier than yours, at two sizes of writing. All 18 read better cleaned:

| | As they come | Cleaned first |
|---|---|---|
| Your tablet's page | 92.97 % | 98.55 % |
| The 18 imitations, on average | 93.2 % | 96.5 % |
| The worst of them | 82.4 % | 90.9 % |
| The least gained, and the most | | 0.3 and 9.9 points |

- **Small writing is not cleaned.** In imitations with lines under about 25 pixels high the cleaning did more harm than good: a stroke there is no wider than a speck. And a sharp picture of small writing, a screenshot, has thin strokes that count as specks. Hence the limit of 32 pixels. The lines of your page are 83 pixels high.
- **`gc-ocr --as-it-is`** reads without the cleaning, and `--timing` says how many pictures were cleaned. `tools/ocrkit/read_pages.py` does the same steps in Python.

**Found on the way: two columns read as one**

- **What happened.** In 6 of the 171 readings I made of the imitations, a page of two columns came out with 28 % of its characters right. Every line had been read well. One line's box had come out too wide and reached into the empty strip between the columns. The layout wanted that strip empty, took the page for a single column, and read straight across both.
- **The change**, in `gc-doc`, which the Mac's hub uses too. Where a region has twelve lines or more, one line in twelve may reach into the strip between two columns. Such a line goes to the column its middle is in. With fewer lines the layout is as strict as before.
- **Measured.** The six readings now come out at 82 to 98 %. The other 165 are unchanged, and so are the seven test documents. Three new tests.
- **Still rough.** A line that reaches into the strip also moves its column's left edge, and the line after it can start a new paragraph where there is none. The text is in the right order; a paragraph break may be one too many.

**Not verified**

- **On the Mac the reader first read wrongly, and why.** Three of its tests failed there on 2026-11-13: it read the test page's heading as "Chợ mổi buỗi sớm" and got 46 of the page's 280 characters wrong. The reason was not in the reader's steps. A model that is opened without naming where it runs is given a place by the inference library, and on a Mac that place is Apple's Core ML, which computes in half precision where it can. The line finder then drew other boxes and the recogniser misread. The reader now says "the processor" when it opens its models. With that, the Mac draws the same boxes as the PC and cuts out the same lines, pixel for pixel, and reads the same text. My first guess, the M4's special kernels in ONNX Runtime, was wrong: on the Mac the reader reads the same with them and without.
- **More real pages.** One page is all there is so far: print from a book, flat, scanned by the tablet in poor light. Curved paper, shadows, small print, colour and handwriting are untried. The imitations are my own guess at what such scans look like, tuned to resemble your page; only your page is real.
- **Your page is not in the repository**, and neither is my typed copy of it. The page is from a book and the scan is yours. Both are in this PC's temporary folder, and the tests use a drawn page with drawn specks instead.
- **The Mac's reader on a speckled scan.** Not tried. The Mac reads with Apple's Vision, and it may or may not mind the specks.
- **Whether the tablet should send a filtered picture at all.** The Android app asks Google's scanner for its fullest mode, which is the one that filters the page to black and white. A plainer mode exists. Which one reads better is for the Mac's session to try, where the Android app is built.
- **HEIC photos are not read.** JPEG, PNG, WebP, TIFF and BMP are. A HEIC photo is reported as unreadable and the others in the job are still read.
- **AsterNet is new and little used**: three weeks old, from a publisher I know nothing else about. Its file is a plain model file that the inference library runs; it cannot run code of its own. Its hash is fixed in the manifest.

## The scanner in the Windows app

The Scanner screen is built, and the app runs the hub that phones and tablets send their photos to.

**What is built**

- **The screen.** A switch, "Receive scans on this PC"; the phones and tablets that are connected, each with a Remove button; "Connect a phone or tablet", which shows a six-digit code with its countdown; and the documents received. A finished document can be read in the window, copied, opened, shown in File Explorer or deleted. English and Vietnamese.
- **The hub** is the same one the Mac app runs, on the same ports, and it announces itself on the network in the same way. The Android app should therefore list this PC as it lists the Mac. Its texts still say "Mac" for any computer; the code dialog on the PC says so.
- **Its secrets**, the token and the certificate that connected devices know the PC by, are kept in Windows Credential Manager, apart from the app's files.
- **Windows Firewall.** The hub does not listen on the network until the firewall has rules for the app, so Windows never puts its own question on the screen. Until then the screen says what is missing and offers "Allow devices on my network". That button starts `tools\allow-windows-hub.ps1` with Windows' administrator question. The script adds two rules, for the app's program and port 47801 only: calls from a private network, such as home Wi-Fi, are let in, and calls from public networks are turned away.
- **If the phone does not find the PC**, the code dialog shows the PC's address on the network to enter by hand.
- **Deleting everything** in Settings now also removes the documents, the connected devices, the token and the certificate.

**What I found on this PC**

- **Why Windows asked you about the firewall earlier.** Windows counts this PC as being on a public network as well as on your private Wi-Fi, because of a virtual network adapter. A program that listens on every network without a rule makes Windows ask, and the answer it offered was about public networks. The rules above settle both kinds, so it does not ask again.
- **14 rules are left over** from that day: Windows made them for test programs of this project that no longer exist. They do no harm. `tools\allow-windows-hub.ps1 -Tidy core\target`, run as administrator, removes them.

**Checked**

- **The model of the screen**, in `gc-app`, by 7 tests with a real hub on this computer: on and off and remembered, the code, the firewall's three answers, the secrets, the documents, deleting everything, and a try-out keeping to itself.
- **Windows' side**, in `gc-win`: the PC's name and addresses, and reading the firewall's rules without administrator rights. Announcing a made-up hub on the network was tried once by hand; Windows accepted it and took it back.
- **The app itself**, by `apps\windows\tests\check-app.ps1`, which now holds 31 checks. A copy on a data folder of its own is started with its scanner on, a page is sent to its hub as a device on a cable would send it, and the document appears in the app with the page's heading as its title and its English line right letter for letter.
- **By looking**: pictures of the Scanner screen off, on with documents, with the code dialog, asking for the firewall to be opened, and on a public network, in English and Vietnamese. `tools\snapshot-windows-app.ps1 -ScannerDemo` takes them; it needs neither the model files nor the graphics card.
- **A copy started on a data folder of its own keeps to itself.** Its hub takes free ports on this computer only, and its secrets are files in that folder. Before, such a copy shared the key for saved faces with your own app in Credential Manager, and deleting everything in it would have deleted yours. No check ever did that; now none can.

**What you checked on 2026-11-13, and what is left**

You built the app, turned the scanner on, allowed devices in, and connected the tablet with the code. This is what the PC shows of it:

- **The firewall** has the two rules, as the script makes them: private networks in, public networks out, for the app's program and port 47801.
- **The hub** listens on the network since the moment you allowed it, and its token and certificate are in Credential Manager.
- **One device is connected**, since 19:09, and the hub has heard from it once. So the code works over your Wi-Fi.
- **One page has arrived**, at 19:39, from the tablet: one photo, and the hub made a document of 33 lines from it. How well it read, and what I changed because of it, is under "The first real page" in "Reading text". The document in your app was read by the reader as it was then. To see the difference, rebuild the app and send the page again.
- **Whether the tablet found the PC by itself**, or needed the address, only you saw.

## Open

- **The camera in a normal browser window and in meeting apps**, and the uneven timing with the real camera. See the top of this note.
- **The scanner with your tablet.** See "The scanner in the Windows app".
- **The rest of the Windows app.** The installer, which registers the camera, adds the firewall rules and carries the model files.
- **More real pages for the text reader**, HEIC photos, and its speed. See "Reading text".
- The occlusion mask at half rate on machines with one accelerator.
- The core changes made here still have to pass `tools/check.sh` on the Mac.
