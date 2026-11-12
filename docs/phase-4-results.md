# Phase 4 results so far

Date: 2026-11-12 · Plan: [implementation-plan.md](implementation-plan.md)

Windows work began on 2026-11-12, when you brought the PC in. That is earlier than the plan's start point, so Phase 4 now runs beside what is left of Phase 3. This note covers the first day: the toolchain, the shared core on Windows, the models on the RTX 3050, and the first version of the virtual camera.

## The PC

| Part | State |
|---|---|
| Windows | Windows 11 Pro 25H2, build 26200.8655 |
| Hardware | Core i5-13400F, 32 GB, GeForce RTX 3050 with 8 GB, driver 616.92 |
| C++ | Visual Studio Build Tools 2022 (17.14) with the C++ tools, Windows SDK 10.0.26100, CMake, Ninja |
| Rust | 1.99.1, MSVC toolchain |
| Python | 3.12 with uv |
| .NET | SDK 8.0.422. Its support ended on 2026-11-10; the app shell will need .NET 10 |
| Camera | None attached. The virtual camera is tested with a generated picture |
| Apps for the camera check | Chrome and Edge are installed. Zoom, OBS, Discord and Slack are not |

Your account is an administrator, and this PC approves administrator requests without asking. My sessions still run without those rights, and I do not use them unless you say so.

## The shared core on Windows

The core had only ever been cross-compiled for Windows from the Mac. It now builds and runs here.

- **Tests.** 145 pass in a full run on the PC. Two more are run by hand: one prints model speeds, and one listens on every network interface (see the firewall note below).
- **Same pictures as the Mac.** The parity tests compare the Rust pipeline with InsightFace's own on its sample photo, and pass here. The reference numbers made on this PC differ from the Mac's by at most 0.19 pixels for a detected keypoint, which is ordinary difference between processors.
- **What had to change for Windows.** The hub's tests used a shell script as their stand-in text reader; they now use PowerShell here. Development builds optimise the tracker, the image code and the live engine, because unoptimised they are seven times slower on this processor and the tests that time them failed for that reason alone.

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

## Spike S2: the virtual camera

Still being built when this note was written; see the section added below it when it lands.

## Open

- The virtual camera: registration, the check in real apps, and the frame hand-off measurement.
- Spike S8, the shared renderer, and then the Windows app itself: capture, render graph, hub with a firewall rule made by the installer, WinUI shell, tray icon, hot keys, installer.
- Text reading on Windows (PP-OCRv5) and its Vietnamese accuracy.
- The occlusion mask at half rate on machines with one accelerator.
- The core changes made here still have to pass `tools/check.sh` on the Mac.
