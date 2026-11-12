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
- The apps should be clear, easy to use, clean, modern and friendly, with one screen per feature and plain words. Developer paths go under "For development". Every string exists in English and Vietnamese.
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

### Waiting for the other machine

Work done on one machine that the other still has to check. Remove a line when it is done.

- **For the Mac:** run `tools/check.sh` and `swift test` after pulling. The core changed on the PC on 2026-11-12: a face is shown only once its place is confirmed (expect a smaller keypoint offset and wobble in the live test, and the first swapped face a frame or two later), the real-time crates are optimised in development builds, and the hub's tests share one stand-in reader. Four crates are new: `gc-render`, `gc-camera` and `gc-app` build and are tested on every system; `gc-win` is empty outside Windows, and the Windows cross-build compiles it with Microsoft's `windows` crate for the first time. Then rebuild the Mac app's core with `tools/build-core-apple.sh` and run the app's self-test.
- **For the Mac:** `apps/android`'s unit test that starts `gc hub --lan` can pass `--lan-addr 127.0.0.1` now.

## The Mac

- macOS 27 on an M4 Pro. The Galaxy Tab S9+ is the Android test device, attached by Wi-Fi debugging.
- In zsh, `log` is a shell builtin: use `/usr/bin/log show ...`.
- Gradle needs JDK 17: `JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home`. The NDK has API levels up to 35, so `cargo ndk` uses `--platform 35`.
- Install the Mac app only with `tools/install-mac-app.sh`; it refuses while the app runs. `--build-only` builds without touching `/Applications`.
- An app that was open while the camera extension was replaced does not list the camera until it is reopened. Change the extension's version only when its code changes.
- Do not probe the screen with AppleScript; it stalls on a permission prompt. Idle time: `ioreg -c IOHIDSystem | awk '/HIDIdleTime/'`.
- Measure speed only on a quiet machine, with a baseline from the same minute.

## The Windows PC

- Windows 11 Pro 25H2, Core i5-13400F, 32 GB, GeForce RTX 3050 with 8 GB. The repository is at `D:\development\projects\gods-camera\gods-camera`.
- Installed: Visual Studio Build Tools 2022 with the C++ tools, Windows SDK 10.0.26100, Rust (MSVC), CMake, Ninja, Python 3.12, uv, .NET SDK 8, JDK 17, GitHub CLI (signed in). For the app's window, installed for the user only: the .NET 10 SDK in `%LOCALAPPDATA%\Microsoft\dotnet`, which is not on the path, and `uniffi-bindgen-cs` in `~/.cargo/bin`. `tools\build-windows-app.ps1` finds both and says how to install them. The window's words are English in the C# code, as `T("...")`; their Vietnamese comes from the Mac app's strings file and `apps/windows/app/Texts/windows.json`, and the build fails if a text has none. The app started by hand uses the user's real data in `%LOCALAPPDATA%\GodsCamera`: for a try-out, start it with `--data` on a scratch folder, as the snapshot script does.
- Sessions are not elevated. The account is an administrator and this PC approves such requests without a prompt, so elevation would work silently: all the more reason not to use it without the user's say-so. Anything that needs administrator rights, such as registering the camera driver for the whole machine, is the user's step, or done only after they agree.
- A USB camera may be attached. Opening it turns its light on and shows the user's room: ask before the first time, and keep no picture from it. Test the virtual camera with a generated picture. The app itself opens a camera only after "Allow camera access" was clicked (`camera_allowed` in its settings file); a tool that starts the app on a scratch data folder therefore never turns the camera on.
- The Windows camera: `tools\build-windows.ps1` builds the driver and its test tools, `apps\windows\tests\check.ps1` runs every check that needs no administrator, and `gc camera` (in `core/gc-cli`) sends the engine's picture to it. Registering the driver is the administrator step: `tools\register-windows-camera.ps1`, which the app's "Install the camera" button also starts, elevated. Never click that button or run that script without the user's word. Only one program can hold the camera's pipe: stop `gc camera` and `gc-vcam-host` before starting another.
- Models run on the RTX 3050 through DirectML. A wholly half-precision model is fast there and gives a wrong picture; Windows uses the mixed-precision copies (`*.mixed.onnx`). Any new model or copy is checked on a real crop with `tools/modelkit/check_swap.py` before it is used, never by speed alone.
- A test or tool must not listen on every network interface: Windows Firewall then asks about the program, and a test program has a new name after every build. Listen on `127.0.0.1`. The app's own hub gets its firewall rule from the installer.
- `cargo test --workspace` in `core/` is the PC's check for a core change. Two tests are run by hand with `-- --ignored`.
- Windows PowerShell reads a script without a byte-order mark in the machine's code page. Keep scripts that it runs in plain ASCII.
- In Git Bash here, a here-document loses one of every two backslashes and fails on an apostrophe. Write a script with backslashes or apostrophes to a file first, then run the file.
