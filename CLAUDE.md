# God's Camera: notes for an assistant working in this repository

This file travels with the repository, so it reads the same on the Mac and on the Windows PC. Keep it short and true. Things that only one machine knows belong in its section at the end.

## What this is

A personal app for macOS, Windows and Android: a virtual camera for meeting apps, a loop of a recorded clip, a live face swap, and a scanner that turns photos from a phone or tablet into one text on the desktop. 3D scanning, try-on and a language-model layer come later.

- The plan, with a checkbox for everything: `docs/implementation-plan.md`. Read the phase you work on before changing anything.
- What was measured and what the user still has to check: `docs/phase-N-results.md`, one per phase.
- The evidence behind the plan: `docs/research-findings.md`.

## Decisions the user has made

- Personal, non-commercial use. Open-source libraries and research-licensed models are fine. Do not raise model licensing as a blocker; keep `commercial_ok` in the pack manifests for a later release.
- Minimum requirements are firm and are never lowered to fit a device. Android's is Android 16 (API 36).
- Safety defaults stay on: the "AI" badge, a consent statement for every face, faces encrypted at rest.
- The apps should be clear, easy to use, clean, modern and friendly, in plain words. Developer paths go under "For development". Every string exists in English and Vietnamese.
- Since 2026-11-14 the camera, the loop and the face swap are one screen, in the desktop apps and in the mobile app, so that they are used together. The interface may change a lot as the plan's features arrive; what counts is a modern, smooth, easy experience. The shared design is in section 6.12 of the plan.
- On 2026-11-14 the user asked for the coding work of every remaining phase to be done, so that they can test everything at once. Phases 11 (no iPhone yet) and 12 (only for a release) wait. Checks that need a person are collected for them, not done one by one.
- Everything the app needs to run is bundled with it.
- Go through the plan's phases without asking for a go-ahead for each. Stop for real questions and for checks that need a person.
- Since 2026-11-12 the work runs on two machines, and each platform's own work is done on its own machine. That brought the Windows start forward; Phase 4 runs beside what is left of Phase 3.

## Rules that are easy to break

- **Secrets.** Never ask for the value of an API key, and never put a key, token or pairing code into the repository, a log, or a command line. Keys live in the Keychain on the Mac and in Credential Manager on Windows; the user enters them in the app.
- **The user's details.** Names, email addresses and device names stay out of the repository and out of anything sent to an outside service.
- **Reference repositories** beside this one on the Mac (`../insightface`, `../Deep-Live-Cam` and others) are read-only. Code may be ported only from InsightFace, which is MIT. Ideas from the GPL and AGPL ones are re-implemented without copying.
- **The user's running app comes first.** Before replacing the installed app, running its self-test, starting a hub on its ports (47800, 47801) or starting a heavy build, check whether the user is using it. Tell them afterwards what was restarted.
- **The tablet.** It has a screen lock: when it is locked, ask; never try to get past it. Do not open the photo library or capture the camera's view. The emulator `gc_test` is for tests; `xq_api36` is the user's own.
- **Look before reporting.** A screen is checked by looking at it: `tools/snapshot-mac-app.sh` on the Mac, `tools\snapshot-windows-app.ps1` on the PC, emulator or device screenshots on Android.
- **Reports** are read by one person switching in from other work: plain sentences, flat lists, what was done, what was not verified, what they should do next.

## Working from two machines

Both machines work on the branch `dev` of the same GitHub repository. `main` holds Phase 0 only.

1. When a session starts on a machine: `git pull` (it rebases; `tools/setup-git.sh` set that up).
2. Commit at every verified step, with a message that says what changed and why.
3. Before the session leaves a machine, and after every larger step: `git push`. Unpushed work is invisible to the other machine.

What does not travel through Git, and how each machine gets it:

| Thing | Where it lives | How to get it |
|---|---|---|
| Model files | `../packs`, beside the repository | `python tools/fetch-packs.py` |
| Reference pictures for the parity tests | `core/gc-infer/tests/golden/*.png` | `python tools/make-reference.py` |
| The core built for the Mac app | `apps/apple/GodsCameraKit/Frameworks` | `tools/build-core-apple.sh` |
| The core built for the Android app | `apps/android/app/src/main/jniLibs` | `tools/build-core-android.sh` |
| Clone settings | `.git/config` | `tools/setup-git.sh`, once per clone |

Which machine does what:

| Work | Machine |
|---|---|
| `apps/apple`: the Mac app, the Camera Extension, the text reader | Mac |
| `apps/android`: the tablet is attached to the Mac for debugging | Mac |
| `apps/windows`, the Windows camera driver, measurements on the NVIDIA GPU | Windows PC |
| `core`: the shared Rust code | Either. Run `tools/check.sh` on the Mac and `cargo test --workspace` on the PC before calling a core change done |

Text files have LF line endings on both machines (`.gitattributes`). Shell scripts run in Git Bash on Windows.

### Running something on the other machine

A session can run a command on the other machine through Mother Claude (`machine_run`), once that machine's Computers page gives this one the Run permission.

- The command runs in a work folder of its own there, at the calling session's files as they are, uncommitted changes included. That machine's checkout is not touched, and does not get the changes: it still has to `git pull`.
- The work folder has no model files and no reference pictures beside it. Point the tests at the real ones: `GC_PACKS_DIR` for the model store, `GC_GOLDEN_DIR` for `core/gc-infer/tests/golden`. The Swift tests look for the reference pictures inside the repository, so copy the PNG files into the work folder first.
- A call comes back after about half a minute whether the command has ended or not, and the command goes on. For anything longer, send its output to a file under `/tmp` and read the end of that file in later calls.
- Ask for no more time than a step needs (`timeout`), and split a long job into steps. On 2026-11-13 runs sent with a limit of 60 to 600 seconds started at once every time. Of three sent with 1800 seconds and more, one started and two never did, and around them the Mac answered no call at all for up to half an hour. When a call is not answered, ask again in a few minutes; do not send the command a second time before its log file shows that it never started.
- Whatever a command starts is stopped when the command ends, so nothing can be left running in the background.
- Every call moves the work folder to the calling session's files of that moment. Change nothing here while a long command is still running there.

### Waiting for the other machine

Work done on one machine that the other still has to check. Remove a line when it is done.

- **For the Mac:** on 2026-11-13 the PC's session ran the Mac's checks through Mother Claude, in a work folder at the PC's code, and all pass there at the evening's code: `tools/check.sh` (253 Rust tests, the lint, the Android and Windows builds), `tools/build-core-apple.sh`, and `swift test` (47 tests). The Mac's own checkout has not pulled since, and the Mac app has not been rebuilt: after `git pull`, run `tools/install-mac-app.sh` when the user is not using the app, and its self-test. `swift test` is run in `apps/apple/GodsCameraKit`, after `tools/build-core-apple.sh`.
- **For the Mac:** the face detectors, and the two models that read a face's identity when one is added, are opened with `Accelerator::Cpu`. On the Mac that has never been the processor: the library runs them through Core ML with its own settings, in half precision. Measured from the PC on 2026-11-13, on a busy Mac: 7 ms for the large detector that way and 25 ms on the processor, 2 ms and 12 ms for the identity encoder. Nothing was changed, because the live pipeline was measured and tuned as it is; only the comments were put right. To decide on a quiet Mac with the app at hand: keep it and give it its true name in the code, or move the detectors. `cargo test -p gc-infer --features runtime --release --test processor -- --ignored --nocapture` prints the times and the first numbers.
- **For the Mac:** `core/gc-ocr` is new on 2026-11-12: the text reader for Windows, built and tested on every system. Run `python tools/fetch-packs.py` for its two model files (52 MB); without them four of its tests pass with a note instead of reading anything. `gc merge-ocr` now says which photos joined when not all did, and `tools/modelkit/make_document.py` finds its font on Windows too.
- **For the Mac:** the Windows app has its scanner since 2026-11-12, so a hub can be a PC. The Android app's texts say "Mac" wherever they mean the computer (`connect_to_mac`, `macs_nearby` and about forty more, in both languages): make them say "computer", and then change the two texts in the PC's code dialog that quote them (`ScannerScreen.cs`, "Connect to a Mac" and "Enter the Mac's address instead"). `gc-app` now starts a hub (`scanner.rs`) and is built with `gc-link`, `gc-ocr` and `tokio`; `AppConfig` has a new field, `try_out`.
- **For the Mac:** `apps/android`'s unit test that starts `gc hub --lan` can pass `--lan-addr 127.0.0.1` now.
- **For the Mac:** the tablet's first real page reached the PC on 2026-11-13 in black and white and sprinkled with specks, and read at 93 %. The Android app asks ML Kit's scanner for `SCANNER_MODE_FULL` (`MainActivity.kt`), the mode with the filters. `gc-ocr` now cleans such a scan first and reads that page at 98.5 %. Not tried: Vision on such a scan, and whether `SCANNER_MODE_BASE` gives either reader a better picture. Try both on a real page in poor light before changing the mode.
- **For the Mac:** `gc-doc`'s layout changed on 2026-11-13: in a region of twelve lines or more, one line in twelve may reach into the strip between two columns without making one column of them. The PC's seven test documents score as before. Score the Mac's copies again with Vision's lines.

## The Mac

- macOS 27 on an M4 Pro. The Galaxy Tab S9+ is the Android test device, attached by Wi-Fi debugging.
- In zsh, `log` is a shell builtin: use `/usr/bin/log show ...`.
- Gradle needs JDK 17: `JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home`. The NDK has API levels up to 35, so `cargo ndk` uses `--platform 35`.
- Install the Mac app only with `tools/install-mac-app.sh`; it refuses while the app runs. `--build-only` builds without touching `/Applications`.
- An app that was open while the camera extension was replaced does not list the camera until it is reopened. Change the extension's version only when its code changes.
- Do not probe the screen with AppleScript; it stalls on a permission prompt. Idle time: `ioreg -c IOHIDSystem | awk '/HIDIdleTime/'`.
- Measure speed only on a quiet machine, with a baseline from the same minute.
- A model opened without naming where it runs does not run on the processor here: the inference library (`ort`) then picks Core ML by itself, which computes in half precision where it can. For a model that must give the same numbers as on the PC, say so: `AutoDevicePolicy::PreferCPU`, as `gc_infer::runtime::Model` does. The face models opened with `Accelerator::Cpu` do not say so, and run through Core ML.

## The Windows PC

- Windows 11 Pro 25H2, Core i5-13400F, 32 GB, GeForce RTX 3050 with 8 GB. The repository is at `D:\development\projects\gods-camera\gods-camera`.
- Installed: Visual Studio Build Tools 2022 with the C++ tools, Windows SDK 10.0.26100, Rust (MSVC), CMake, Ninja, Python 3.12, uv, .NET SDK 8, JDK 17, GitHub CLI (signed in). For the app's window, installed for the user only: the .NET 10 SDK in `%LOCALAPPDATA%\Microsoft\dotnet`, which is not on the path, and `uniffi-bindgen-cs` in `~/.cargo/bin`. `tools\build-windows-app.ps1` finds both and says how to install them. The window's words are English in the C# code, as `T("...")`; their Vietnamese comes from the Mac app's strings file and `apps/windows/app/Texts/windows.json`, and the build fails if a text has none. The app started by hand uses the user's real data in `%LOCALAPPDATA%\GodsCamera`: for a try-out, start it with `--data` on a scratch folder, as the snapshot script does. While the user's app runs from the usual build folder its files are in use: build with `-OutputFolder` into another folder, and give that program to `tools\snapshot-windows-app.ps1 -Program` and `apps\windows\tests\check-app.ps1 -Program`. When the app dies at its start, read `Logs\gods-camera.log` in its data folder first.
- Sessions are not elevated. The account is an administrator and this PC approves such requests without a prompt, so elevation would work silently: all the more reason not to use it without the user's say-so. Anything that needs administrator rights, such as registering the camera driver for the whole machine, is the user's step, or done only after they agree.
- A USB camera may be attached. Opening it turns its light on and shows the user's room: ask before the first time, and keep no picture from it. Test the virtual camera with a generated picture. The app itself opens a camera only after "Allow camera access" was clicked (`camera_allowed` in its settings file); a tool that starts the app on a scratch data folder therefore never turns the camera on.
- The Windows camera: `tools\build-windows.ps1` builds the driver and its test tools, `apps\windows\tests\check.ps1` runs every check that needs no administrator, and `gc camera` (in `core/gc-cli`) sends the engine's picture to it. Registering the driver is the administrator step: `tools\register-windows-camera.ps1`, which the app's "Install the camera" button also starts, elevated. Never click that button or run that script without the user's word. Only one program can hold the camera's pipe: stop `gc camera` and `gc-vcam-host` before starting another.
- The scanner: the app runs the hub on ports 47800 and 47801 when "Receive scans on this PC" is on, and listens on the network only once Windows Firewall has the two rules that `tools\allow-windows-hub.ps1` makes. That script, and the app's "Allow devices on my network" button which starts it, change the firewall for the whole PC: never run or click them without the user's word. Windows counts this PC as on a public network too (a virtual adapter), so a program that listens on every network without a rule brings up Windows' own question.
- A copy of the app started with `--data` is a try-out. It keeps its secrets as files in that folder, and its scanner's hub takes free ports on 127.0.0.1. The user's own app keeps its secrets in Credential Manager: `GodsCamera.FaceStore`, `GodsCamera.HubToken`, `GodsCamera.HubIdentity`. Leave those alone.
- Models run on the RTX 3050 through DirectML. A wholly half-precision model is fast there and gives a wrong picture; Windows uses the mixed-precision copies (`*.mixed.onnx`). Any new model or copy is checked on a real crop with `tools/modelkit/check_swap.py` before it is used, never by speed alone.
- Text in photos is read by `gc-ocr` (`core/gc-ocr`): PP-OCRv6's line detector and the AsterNet recogniser, on the processor. A speckled scan, as a tablet's scanner makes of a page in poor light, is cleaned first (`Cleaning` in its settings); `--as-it-is` reads without. PP-OCR's own recognisers cannot write Vietnamese, whatever their papers say: check a recogniser's character list before trying it. `tools/ocrkit` has the same steps in Python and the comparison of readers. A change to the reader is measured on the test documents before it is kept: `tools/modelkit/make_document.py` makes them, `gc merge-ocr --truth` scores them. A page that came out badly is first looked at for its order: one line's box in the wrong place can make the layout read two columns across, and that looks like 28 % of the characters right when every line was read well.
- The user's scans are in `%LOCALAPPDATA%\GodsCamera\Hub\uploads`. They may be read to measure the reader. They, and any text typed from them, stay out of the repository.
- A test or tool must not listen on every network interface: Windows Firewall then asks about the program, and a test program has a new name after every build. Listen on `127.0.0.1`. The app's own hub gets its firewall rule from the installer.
- `cargo test --workspace` in `core/` is the PC's check for a core change. Two tests are run by hand with `-- --ignored`. Other projects build on this PC too: before measuring speed, or when a test that keeps time fails, look for another `cargo` at work, leave it alone, and run the test again by itself.
- Windows PowerShell reads a script without a byte-order mark in the machine's code page. Keep scripts that it runs in plain ASCII.
- In Git Bash here, a here-document loses one of every two backslashes and fails on an apostrophe. Write a script with backslashes or apostrophes to a file first, then run the file.
