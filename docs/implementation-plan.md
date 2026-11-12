# God's Camera — Implementation Plan

Date: 2026-11-12, revision 11 (Phases 1 and 2 built and revised after your first hands-on test; in Phase 3 the scanner and pairing over Wi-Fi are built; Phase 4 began on the Windows PC, a phase early, when you brought it in: its camera driver and its engine are built and wait for the driver's registration; manual checks and the webcam clips are open) · Evidence and sources: [research-findings.md](research-findings.md)

## 1. Summary

God's Camera is a family of native apps for personal use: a desktop app for macOS and Windows, and a mobile app, first for Android and later for iPhone and iPad. The desktop app owns the system virtual camera and the heavy processing, and acts as the server for the phone. The mobile app captures, scans and shows results. It runs models on the device only where the hardware keeps up.

What shapes the plan:

1. **Phones cannot replace the camera for other apps.** Desktop can. On mobile the plan delivers the same outcome through other routes (section 3.3).
2. **InsightFace is the foundation.** Its code is MIT, so we port it directly. Its models are research-licensed, which suits a personal build.
3. **Development is localhost-first.** The Mac app is the only backend. A phone on a USB cable reaches it as localhost.
4. **Android is the first mobile platform.** You own a Galaxy Tab S9+ and a Galaxy S22+, and no iPhone or iPad. The tablet and the Mac are the development pair. The phone joins once it is updated to Android 16. The Apple mobile app waits for hardware.
5. **There are two desktops, and the Mac comes first.** The Mac is the development machine and the stronger one for OCR and language models. The Windows PC adds an NVIDIA GPU with 8 GB of video memory. Nothing is set up on it until a fixed start point, the exit of Phase 3 (section 12).
6. **Safety defaults are on.** A visible AI indicator, a consent attestation per face, and synthetic faces included.
7. **A release in Vietnam is a later, separate step.** It has its own gate (Phase 12) for model licences and Vietnamese law.

Phases 0–3 produce the first usable product: a Mac virtual camera with loop and live face swap, plus an Android companion that scans documents and serves as a camera. That is roughly four to six months of solo work. Everything through try-on (Phases 0–9) is about a year.

Terms used throughout:

- **Hub**: the desktop app acting as a server for phones and test clients.
- **Pack**: a set of model files with a manifest that records source, hash and licence.
- **Offload**: the phone sends work to a desktop and shows the result.
- **Late reprojection**: re-warping the most recent swapped face onto the newest camera frame, so output never waits for the model.
- **Reference device**: one of the four devices you own. Targets are verified only on these.

## 2. Decisions

| ID | Decision | Outcome | Effect on the plan |
|---|---|---|---|
| D1 | Purpose | Personal use (decided 2026-11-11) | Research-licensed models are acceptable. No store, licensing or legal work now |
| D2 | Models | InsightFace's open models, plus open alternatives; no paid licence (decided) | Section 6.5 fixes the model stack |
| D3 | Safety defaults | Accepted (decided) | Indicator on by default, consent attestation, synthetic faces included |
| D4 | Market | Personal now; Vietnam only, later (decided) | Vietnamese is a first-class language. Phase 12 is a release gate, not a launch |
| D5 | Minimum systems | Firm floors (decided). A floor is what the app needs to run well; it is not lowered to fit a device (confirmed 2026-11-11) | Section 4. A device below a floor is not supported until it is updated |
| D6 | Development mode | Desktop app as server, everything on localhost, cost not a constraint (decided) | Section 5 |
| D7 | Cloud models | Keys for Claude, Gemini and OpenAI are available (answered 2026-11-11) | Cloud models are usable from the first language and try-on work. Keys stay on the desktop |
| D8 | Team | One developer (answered) | Sizes assume solo work with an AI coding assistant. Fewer codebases is a design goal |
| D9 | Architecture | Native shell per platform over a shared Rust core (my default) | Best OS integration; hard logic written once |
| D10 | Inference runtime | ONNX Runtime on every platform first; other formats only where a measurement justifies them (my default) | Models stay in one format |
| D11 | Platform order | Mac first, Windows later (decided 2026-11-11). Android is the first mobile platform. iPhone and iPad when you have one | Section 13. I tell you when the Windows start point is reached (section 12) |

## 3. Scope

### 3.1 Feasibility of each idea

| Idea | Verdict | How |
|---|---|---|
| Replace the camera feed on desktop | Yes | Camera Extension on macOS; Media Foundation virtual camera on Windows 11 |
| Replace the camera feed on mobile | Not system-wide | Six substitute routes, section 3.3 |
| Desktop as server, phone as scanner, OCR and consolidation | Yes | Authenticated local link; OCR and merge on the desktop |
| Loop a recorded clip as the camera feed | Yes on desktop | Clip store with seamless loop modes; on mobile inside the routes of 3.3 |
| Scan an object into a 3D model | Yes | Capture on the phone; reconstruction on the desktop |
| Face swap, on-device or offloaded | Yes | Desktop first; phones offload, and run on-device only if they meet the target |
| Body, clothing, accessory and hair try-on | Yes, as generated images | Cloud image models or a local pack; live only for hair recolour and face-anchored accessories |
| Claude, Gemini, OpenAI and local LLMs | Yes | One provider gateway on the desktop; phones go through the hub |

### 3.2 What ships where

| Capability | macOS | Windows | Android (S22+, Tab S9+) | iPhone / iPad (later) |
|---|---|---|---|---|
| System virtual camera | Yes | Yes | No | No |
| Camera loop | Yes | Yes | Within 3.3 routes | Within 3.3 routes |
| Live face swap | On-device | On-device | Through a desktop first; on-device only if it meets the target | Same |
| Scanning | Receives, OCRs, consolidates | Same | Captures and sends; quick preview | Same |
| 3D scan | Reconstructs | Reconstructs | Captures | Captures |
| Try-on | Generates (cloud or local) | Same | Capture, results, live hair recolour | Same |
| LLMs | Cloud, local, serves phones | Same | Through the hub | Through the hub, plus Apple's on-device model |

### 3.3 Mobile substitutes for "camera override"

Ordered by reliability. The first three ship in Phases 3 and 6. The rest are spikes in Phase 7.

1. **Phone as the desktop's camera.** The desktop virtual camera carries the processed feed into every meeting app.
2. **Record, export and live-stream** (RTMP, SRT, WHIP).
3. **Screen-share from the phone's meeting app** while God's Camera shows a clean full-screen output. Android can share a single app, which keeps notifications and the status bar out of the picture (to confirm on One UI and in each meeting app).
4. **Join meetings inside our app** through SDKs that accept custom video: Zoom Meeting SDK, Azure Communication Services for Teams.
5. **Our own rooms** over WebRTC, guests joining by browser link.
6. **In-app browser** that overrides the web camera API for web meeting clients. Experimental.

### 3.4 Non-goals

- Root, bootloader unlocking or hooking other apps to inject a camera on phones. The Android tools that replace the camera need root. Unlocking a Samsung bootloader permanently trips Knox, which disables Samsung Wallet and Secure Folder and can break banking apps.
- Any "undress" or nudity generation.
- Evading identity-verification, banking or exam-proctoring systems.
- Voice conversion (not requested).

## 4. Devices and system requirements

### 4.1 Reference devices

| Device | Role | What is known |
|---|---|---|
| MacBook Pro, M4 Pro, 24 GB, macOS 27.2.1 | Development machine and first desktop | Verified by probe |
| Windows desktop: Core i5-13400F, 32 GB, GeForce RTX 3050 8 GB, external camera, Windows 11 Pro | Second desktop, set up later | As you reported. It has no integrated GPU and no neural processor. The Windows version is recorded when Windows work starts |
| Galaxy S22+, 8 GB / 128 GB | Phone, once updated | Checked over USB. It is the US model (SM-S906U) with a Snapdragon 8 Gen 1 and an Adreno 730 GPU, 7.1 GB of usable memory and 17 GB of free storage. It runs Android 13 (One UI 5.1, June 2023 patch), which is below the Android floor. Steps to update it: [updating-the-galaxy-s22-plus.md](updating-the-galaxy-s22-plus.md) |
| Galaxy Tab S9+, 12 GB / 512 GB | Development device, paired with the Mac | Checked over USB. It is the 5G model (SM-X816B) with a Snapdragon 8 Gen 2 and an Adreno 740 GPU. It runs Android 16 (One UI 8.5, September 2026 patch), has 10.9 GB of usable memory and 377 GB of free storage, and supports ARCore |

### 4.2 Minimum requirements

A floor is what the app needs to run well on that platform. It is not lowered to fit a particular device: a device below a floor is not supported until it is updated. The desktop floors are the two machines you own, since they are the only desktops the targets can be verified on.

| Platform | Minimum | Notes |
|---|---|---|
| Mac | Apple silicon, 24 GB memory, macOS 27, 60 GB free disk | Tested on the M4 Pro only |
| Windows PC | Windows 11 64-bit, Core i5-13400F class CPU, 32 GB memory, GeForce RTX 3050 with 8 GB, a DirectX 12 graphics driver | Tested on Windows 11 Pro 25H2 (build 26200) with NVIDIA driver 616.92 |
| Android phone or tablet | Android 16 (API level 36), 64-bit ARM, 8 GB memory, Snapdragon 8 Gen 1 or newer | The tablet meets it. The phone does not until it is updated |
| iPhone / iPad | Not supported yet | Provisional floor when you have one: iPhone 16 or later, iOS 27 |
| Network | Phone and desktop on the same Wi-Fi 6 network, or joined by a USB cable | |

### 4.3 Which device does what

| Job | Mac (M4 Pro, 24 GB) | Windows PC (RTX 3050, 8 GB) | Galaxy S22+ | Galaxy Tab S9+ |
|---|---|---|---|---|
| Virtual camera and loop | Phase 1 | Phase 4 | — | — |
| Live swap | Phase 2 | Phase 4; speed to measure | Through a desktop; on-device unlikely | Through a desktop; best candidate for on-device |
| OCR | Apple Vision; Vietnamese verified | PP-OCRv5; Vietnamese accuracy to measure | Quick preview with ML Kit | Same |
| Local LLMs | Apple's on-device model and Ollama | Small models only | Through the hub | Through the hub |
| 3D reconstruction | Object Capture; verified available | Open pipeline | Capture | Capture |
| Local try-on | To test | Borderline: the model wants about 8 GB | — | — |
| MiniMax H3 clips | No | No | — | — |

### 4.4 How the floors are enforced

- The app refuses to install below the OS minimum.
- A first-run check measures the device against the targets in section 7.
- A feature that cannot meet its target on a device is switched off there, with the reason shown. It is never left to run badly.

## 5. Development setup

### 5.1 The Mac

Verified on 2026-11-11; details are in the research document, section 8.

- **Ready:** Xcode 27.2, Swift 6.4, Rust 1.99 with Windows targets, Python 3.14 with uv, Homebrew, ffmpeg, and all three kinds of Apple signing certificate.
- **Ready for Android:** Android Studio 2026.2, SDK platforms for API 34–37, NDK 28.2, and an emulator with an Android 16 image that can use the Mac's camera.
- **Ready for testing:** Zoom, Teams, Chrome, Slack and Discord are installed. FaceTime, QuickTime and Safari are built in.
- **Ready for language and 3D work:** the built-in OCR and the on-device language model both support Vietnamese. Object Capture reconstruction is available. Ollama is installed with `qwen2.5:7b`.
- **Missing:** cmake on the PATH, Rust's Android target, cargo-ndk, the UniFFI tools, cargo-xwin and LLVM, the `ANDROID_HOME` variable, and the InsightFace model files.
- **Tight:** 32 GB of free disk. Models, build products and local LLMs need about 60 GB.

### 5.2 The Android devices

- The Tab S9+ is the development device. It is checked: Android 16, all core tests pass on it, and it uploads through the cable at about 30 MB/s.
- The S22+ is on Android 13, below the floor, so the app will not install on it yet. It is a US model whose firmware no longer receives over-the-air updates. The update steps are in [updating-the-galaxy-s22-plus.md](updating-the-galaxy-s22-plus.md).
- `adb reverse` maps a port on the device to the Mac's loopback. The real phone then uses the development hub as localhost over the cable.

### 5.3 The Windows PC

- In use since 2026-11-12, when you moved the work onto it. Each platform's own work is done on its own machine, and the two share one GitHub repository; `CLAUDE.md` at the top of the repository says how.
- Found ready: Visual Studio Build Tools 2022 with the C++ tools, Windows SDK 10.0.26100, Rust, CMake, Ninja, Python with uv, Git and the GitHub CLI, and a current NVIDIA driver. The .NET SDK is version 8, whose support has ended; the app shell needs .NET 10.
- Not needed after all: the CUDA and TensorRT libraries. The models run through DirectML, which comes with ONNX Runtime as one library (spike S3, in [phase-4-results.md](phase-4-results.md)).
- The model files and the parity tests' reference pictures are made on the PC by `tools/fetch-packs.py` and `tools/make-reference.py`.
- The Mac still cross-compiles the core for Windows on every merge. The PC now runs the core's tests natively as well.
- Work sessions on the PC run without administrator rights. Steps that need them, such as registering the camera driver, are yours, or mine when you say so.
- It has an external camera; none was attached on the first day, so the virtual camera is tested with a generated picture.

### 5.4 Localhost-first workflow

The hub has two modes.

| | Development mode | Paired mode |
|---|---|---|
| Listens on | 127.0.0.1 only | The local network interface |
| Authentication | A fixed token from a local config file | A per-device key issued at pairing |
| Encryption | None needed on loopback | TLS with a pinned certificate |
| Origin check, size and time limits | On | On |
| When | Phases 0–3b, and day-to-day development after that | From Phase 3c, over Wi-Fi |

Clients during development:

1. **A command-line client in the core.** It uploads a folder of images, starts jobs, streams a video file as if it were a phone camera, and prints results. It drives most of Phases 3, 5, 8 and 9.
2. **The Android emulator.** It reaches the Mac's hub at its host address and uses the Mac's camera.
3. **The Tab S9+ on a USB cable**, through `adb reverse`.
4. **The same devices over Wi-Fi**, in paired mode.

All heavy work (swap, OCR, 3D, LLMs) runs on a desktop. The phone app stays a thin client until Phase 6 tests on-device inference.

### 5.5 API keys

- No keys were set in the shell environment when I checked.
- Keys go into the macOS Keychain, and into Windows Credential Manager on the PC. You enter them in the app's settings screen, which Phase 5 builds. The core reads them at run time.
- Keys never go into the repository, logs, command history or this chat.
- Only the desktop calls cloud providers. Phones ask the hub, so no key is stored on a phone.

## 6. Architecture

### 6.1 Overview

```
┌───────────────────────── Desktop app (macOS / Windows) ─────────────────────────┐
│  Native shell: UI, capture, GPU compositing, virtual camera driver              │
│  ┌──────────── Live engine ───────────────────────────┐  ┌───── Hub ─────────┐  │
│  │ capture → track → detect → swap → composite → out  │  │ HTTP + WebSocket  │  │
│  └───────┬───────────────────────────┬────────────────┘  │ + video streams   │  │
│   Virtual camera driver        Loop store / recorder     │ jobs: OCR, 3D, LLM│  │
│                                                          └─────────┬─────────┘  │
│  ── Shared Rust core: infer · track · align · link · models · jobs · doc · llm ─│
└────────────────────────────────────────────────────────────────────┼────────────┘
                              USB cable (localhost) in development; paired Wi-Fi later
┌───────────────────────── Mobile app (Android first, iOS later) ────┼────────────┐
│  Native shell: UI, camera, scanner, 3D capture, try-on capture, remote control  │
│  Thin client of the hub first; on-device swap only if Phase 6 measures it works │
│  ── Shared Rust core ────────────────────────────────────────────────────────── │
└─────────────────────────────────────────────────────────────────────────────────┘
```

The split of responsibilities:

- **Native shell**, per platform: UI, camera capture, GPU work on full frames, the virtual camera driver, OS services such as OCR.
- **Rust core**, shared: model inference, detector decoding, tracking, alignment maths, the hub and its clients, jobs, document merging and the LLM gateway.

### 6.2 Technology per platform

| Layer | macOS | Windows | Android | iOS / iPadOS (later) |
|---|---|---|---|---|
| UI | SwiftUI + AppKit | WinUI 3, C# | Jetpack Compose, Kotlin | SwiftUI, sharing packages with macOS |
| Capture | AVFoundation | Media Foundation | CameraX | AVFoundation |
| GPU | Metal | Direct3D 11 | OpenGL ES 3.1 | Metal |
| Inference | ONNX Runtime, Core ML provider | ONNX Runtime, DirectML provider | ONNX Runtime on the CPU for the detector; swap runtime chosen by bake-off | ONNX Runtime, Core ML provider |
| Video codec | VideoToolbox | Media Foundation with NVIDIA hardware codecs | MediaCodec | VideoToolbox |
| Virtual camera | Camera Extension (Swift) | Media Foundation media source (C++) | — | — |
| Scanner and OCR | Vision, accurate level | PP-OCRv5 on ONNX Runtime | ML Kit document scanner and text recognition | VisionKit, Vision |
| 3D | Object Capture | Open pipeline (Gaussian splats) | Capture; ARCore where supported | Guided Object Capture |
| System LLM | Foundation Models | None on this PC | None on these devices, as far as published lists show | Foundation Models |
| Shared core | Rust via UniFFI | Rust via C ABI + C# bindings | Rust via UniFFI, built with the NDK | Rust via UniFFI |

- Shaders are written once in WGSL and cross-compiled at build time to Metal, HLSL and GLSL ES.
- Offline tooling is Python in a pinned environment: reference runs, model checks, pack building.

### 6.3 Shared core modules

| Module | Responsibility |
|---|---|
| `gc-infer` | ONNX Runtime sessions; detector pre-processing, decoding and suppression; identity encoder; swap models behind one contract |
| `gc-live` | The live engine: tracking on the frame thread, detector and swap workers, late reprojection, fades; still-image swap |
| `gc-ffi` | The interface the apps see, turned into Swift and Kotlin by UniFFI |
| `gc-track` | Sparse optical-flow tracker, One Euro filter, detection scheduling, reprojection maths |
| `gc-align` | Alignment templates, similarity transforms, mask parameters, head pose |
| `gc-link` | The hub server and its clients: discovery, pairing, authenticated HTTP and WebSocket, resumable transfer, video streams |
| `gc-models` | Pack manifests, hash verification, device capability check |
| `gc-jobs` | Job queue with persistence and a video-memory budget; live video takes priority over batch work |
| `gc-doc` | OCR merge, reading order, overlap removal, exporters |
| `gc-llm` | Provider gateway, local model bridge, schemas, usage metering |
| `gc-safety` | Consent ledger, indicator and watermark policy, content credentials |
| `gc-cli` | The command-line client used for development and automation |

### 6.4 Live video engine

| Stage | Runs | Work |
|---|---|---|
| 1. Capture | Every frame | Native camera into GPU-backed buffers; one-slot hand-off that drops the oldest frame |
| 2. Track | Every frame, CPU, 1–2 ms | Optical flow on the half-resolution luma plane; smoothing; yields the current face transform |
| 3. Detect | 4–12 times a second, plus forced | Detector with 5 keypoints. Forced on fast motion, tracker disagreement, dropped frames or a settings change |
| 4. Swap | As fast as the model allows | Aligned crop cut by the core from the frame's memory, swapped; latest-wins queue |
| 5. Masks | Every N frames | Occlusion and hair segmentation, cached |
| 6. Composite | Every frame, GPU | Newest transform re-warps the latest patch; mask × occlusion × fade; colour match; optional sharpen |
| 7. Output | Every frame | Virtual camera, preview, recorder, video stream, loop store |

Two properties matter:

- **Stage 7 never waits for stage 4.** A slow model lowers how often the face texture refreshes, not the frame rate or the head tracking.
- **The models only see small images.** The detector sees a downscaled frame and the swap model a 128 or 256 px crop. The core reads the frame's memory in place for the tracker, the crop and the detector's copy; the full frame is drawn only on the GPU.

### 6.5 Model stack

| Role | Desktop | Android | Source |
|---|---|---|---|
| Detector | SCRFD `det_10g` | SCRFD `det_500m` at 320 px, on the CPU | InsightFace `buffalo_l` and `buffalo_s` packs |
| Identity encoder | `w600k_r50`, run once per source photo | Not needed: the desktop computes and syncs the identity vector | `buffalo_l` |
| Swap, baseline | `inswapper_128`. On the Mac, the half-precision variant on the Neural Engine | On a desktop first; on the device only if the bake-off passes | InsightFace model-zoo release |
| Swap, higher resolution | HyperSwap 256. On the Mac, on the GPU | No | FaceFusion assets |
| Landmarks | `2d106det`, only when mouth or eye features are on | Later | `buffalo_l` |
| Head pose | `1k3d68` on detection frames | Carried by the tracker | `buffalo_l` |
| Occlusion mask | XSeg, every N frames | On the desktop, with the swap | FaceFusion assets |
| Enhancer | GPEN-BFR-256, optional | No | Face-Upscalers-ONNX |
| Hair mask | MediaPipe segmentation | Same | Google |

Rules that come from the reference code:

- Every swap model sits behind one **swapper contract**: crop size, alignment template, input range, identity input, output range. The research document, section 4.3, lists the values for each model.
- `inswapper_128` accepts only the `w600k_r50` embedding. No smaller encoder can replace it.
- The 128 px crop uses the alignment template shifted 8 px in x. The research document, section 4.8, has the exact constants.
- Model files are downloaded once into a local pack store outside git, and each file's SHA-256 is pinned in its manifest. The upstream downloads have no integrity check.

Measured on the Mac in spike S3:

- HyperSwap takes 34 ms a swap on the GPU. `inswapper_128` takes 37 ms on the Neural Engine, once its reflect padding is rewritten and it is converted to half precision; without those two steps it takes 54 ms.
- Both are slower than the camera's 33 ms frame, so late reprojection is required, not optional.
- HyperSwap also returns its own face mask.

Measured in Phase 2 with the whole engine running at 1280×720 and 30 frames a second:

- 27 swaps a second with `inswapper_128` and 30 with HyperSwap. The swapped face on screen is 2 frames old at the median and 3 at the 95th percentile.
- The frame thread needs about 1 ms a frame for tracking, smoothing and cutting the crop.
- Both detectors are faster on the CPU than through Core ML: about 11 ms for the large one. They run there, which leaves the GPU and the Neural Engine to the swap model.
- The tracker is the steady part and the detector the noisy part. A detection therefore only nudges the tracked keypoints, a tenth of the way, unless the two disagree clearly.

Measured on the Windows PC in spike S3, on the RTX 3050 through DirectML:

- A wholly half-precision swap model is fast and gives a wrong picture: 18 ms a swap and 14 levels off for `inswapper_128`, and 7 levels off for HyperSwap, which is stored that way. A mixed-precision copy, with the normalisation in full precision, takes 21 to 22 ms and is right. Windows uses those copies; the manifests pin them.
- With the whole engine running: 25 swaps a second with the occlusion mask and 30 without. The swapped face on screen is 2 frames old at the median.
- The PC has one accelerator, so the swap model, the detector and the occlusion mask share the card. The large detector still belongs there: 10 ms against 48 ms on the processor.
- NVIDIA's CUDA provider was no faster than DirectML for any model on this card.

### 6.6 Virtual camera drivers

**macOS**

- A sandboxed Camera Extension with one device and two streams: a source stream that client apps read and a sink stream the app writes.
- It offers 1280×720 and 1920×1080, at any rate from 30 to 60 frames a second. The app renders at the size the reading apps ask for.
- Frames are IOSurface-backed, so nothing is copied.
- The extension shows a placeholder slate when no frames arrive.
- The extension runs as a system user, not as you. It cannot share preferences or a folder with the app, so everything passes through three custom device properties: counters, blocked processes, and the update hand-over.
- macOS keeps a copy of each property on the reading side. The extension therefore announces its counters about once a second, and at once when an app starts or stops reading.
- The extension reports how many apps are reading, so the app opens the real camera only while something is watching.
- macOS 27 does not tell the extension which app a client is; it gives a process number only. The app works out which app each process belongs to. It sends the extension the process numbers to refuse, and it sends no pictures at all while a blocked app is reading, because every reader receives the same pictures.
- **Updates.** macOS starts a new build of the extension at the instant it stops the old one, and never starts it if the old one is still running. The app therefore asks the old build to stand down first, and the old build refuses to come back for 12 seconds. An app that was running during the update cannot see the new camera, so it reopens itself once. Measured: 0.66 seconds for the hand-over, about 14 seconds for the whole update, no restart of the Mac.
- The extension cannot confirm that pictures come from the God's Camera app, for the same reason. This is a known gap, listed for the security review in Phase 12.

**Windows**

- A custom media source DLL registered machine-wide and created with `MFCreateVirtualCamera`.
- The Frame Server service loads it in a different session from the app, so frames cross through shared memory or a shared texture. Spike S2 decides which.
- Session lifetime by default: the camera disappears when the app exits.

### 6.7 Hub

- **Interface.** HTTP with JSON for control and uploads, a WebSocket for events, and a framed binary stream for live video. One OpenAPI description is the contract for every client.
- **Modes.** Development and paired, as in section 5.4. The hub is off until the user turns it on.
- **Pairing.** As built: the desktop shows a six-digit code, good for five minutes, one device and five wrong tries. The device and the hub run SPAKE2 on it, so the code itself never crosses the network, and each proves to the other that it knows the code and saw the same TLS certificate; a relay in the middle with another certificate fails. The device then pins the hub's certificate and receives a token of its own, which the hub keeps only as a hash. The hub's certificate and key live in the Keychain. A QR code holding the same things can be added later as a quicker way in.
- **Two listeners.** Loopback, plain HTTP, the development token: for a device attached for debugging. The local network, TLS, device tokens only: for paired devices. Neither accepts the other's token.
- **Discovery.** mDNS only finds the address. Identity always comes from the pinned keys.
- **Live video.** Hardware-encoded frames, each with a small header: stream, frame ID, timestamp, face transform. The codec is negotiated per pair: HEVC where both ends decode it in hardware, H.264 otherwise. The sender drops frames when the stream backs up.
- **Remote control.** The same interface exposes the desktop's live controls (loop, source face, preset), so a paired phone or tablet works as a remote during a call.
- **Every route** needs a token and passes an origin check. Bodies are size-limited while streaming, decoded images have a pixel-dimension cap, and requests time out.
- **Borrowed from InsightFace Server:** keys stored as hashes, one public health endpoint and nothing else public, a request ID on every response, one error format.
- **Jobs** are persisted and resumable. The scheduler tracks video memory, so on the 8 GB Windows GPU heavy jobs run one at a time. Live video always goes first.

### 6.8 Split rendering for phones

This is how a phone gets desktop-quality swap, and it is the first swap mode the Android app gets.

1. The phone detects and tracks locally and sends only the aligned face crop with a frame ID.
2. The desktop swaps and enhances, then returns the swapped patch with its alpha mask.
3. The phone composites the patch onto its newest camera frame using its current tracker transform.

Network delay then affects how fresh the face texture is, not how the picture follows the head. Bandwidth is a small square video, not a full frame.

### 6.9 Packs

Each pack manifest records the source URL, SHA-256, licence text and one flag, `commercial_ok`.

- The personal build accepts every pack.
- A future release build refuses packs without the flag. That is the whole mechanism behind the Vietnam gate.

Included with the desktop app, so it works offline after install:

- Virtual camera driver and engine.
- Detector, identity encoder, landmarks and `inswapper_128` (about 1 GB together).
- OCR baseline: built into macOS; PP-OCRv5 files on Windows.

Added on demand, per device:

- HyperSwap, occlusion mask and enhancer.
- Local LLMs beyond what the system provides.
- Try-on, document-vision and 3D generation packs.

### 6.10 Feature pipelines in brief

**Documents**

1. The phone, or the command-line client, uploads pages, overlapping shots or screenshots with their order.
2. The desktop pre-processes, runs OCR, finds layout and reading order, and removes text duplicated by overlaps.
3. Text is normalised to Unicode NFC, and overlap matching is aware of Vietnamese diacritics.
4. An optional LLM pass cleans up, constrained to the OCR text and shown as a diff.
5. The user reviews image and text side by side, then exports.

**3D objects**

1. Capture on the phone: many photos around the object, with coverage hints.
2. Reconstruction on the desktop: Apple's Object Capture on the Mac, which accepts ordinary photos; a splat-based open pipeline on Windows.
3. Viewing, measuring, export, and an LLM description of the object.

**Try-on**

1. Capture the body, garments, accessories and hair.
2. Generate with a cloud image model or a local pack.
3. Live preview only where it is genuinely real-time: hair recolour and face-anchored accessories.

**LLMs**

- One gateway on the desktop speaks to Claude, Gemini, OpenAI and any OpenAI-compatible local server.
- Local models start with what the Mac already has: Apple's on-device model and Ollama. An embedded runtime follows, so the app carries its own.
- Phones use the desktop's local models and cloud providers through the hub.
- Claude specifics: default model `claude-opus-5-5`, with the model picker filled from the Models API. Streaming for long outputs, structured outputs for extraction, prompt caching for repeated instructions, Message Batches for large scan jobs, and explicit handling of the `refusal` stop reason with server-side fallbacks. Claude has no official Rust SDK, so the gateway uses raw HTTPS.

### 6.11 Trust and safety layer

- **Consent.** Every face source carries an attestation. "This is me" is established by a short self-enrolment. Synthetic faces are the default choice.
- **Disclosure.** A visible AI indicator on the output, on by default. An invisible watermark. Content credentials (C2PA) on recorded exports.
- **Loop honesty.** A local "looping" indicator the user always sees; an optional on-stream badge.
- **Client policy.** On macOS the driver can refuse named client apps.
- **Generation limits.** No nudity or undress output; an output classifier backs this up.
- **Data.** Face embeddings, body photos and measurements are encrypted at rest, stay on the user's devices, and have a delete-everything control.
- **Cloud use** is a switch per feature, off until turned on, with a notice of what leaves the device.

## 7. Targets

Targets are goals to validate on the reference devices, not measurements. The swap targets are confirmed or reset by spikes S3 and S6.

| Area | Target |
|---|---|
| Mac output | 720p30 held with swap on; 1080p30 as the stretch |
| Windows output | 720p30 held with swap on the RTX 3050 |
| Added latency, capture to virtual camera | Mac: ≤ 12 ms median, ≤ 25 ms at the 95th percentile. Windows: set by spike S2 |
| Age of the swapped face when it is shown | Mac: ≤ 80 ms median, ≤ 110 ms at the 95th percentile, reset after spike S3. Windows: the same; spike S3 measured 60 ms at the median on the RTX 3050 |
| Tracker | ≤ 2 ms per frame; ≤ 0.3 px wander at rest |
| Parity with InsightFace's Python output on the same inputs | Mean absolute difference ≤ 2 levels out of 255 on the swapped crop |
| Android through a desktop | 30 fps output; swap patch age ≤ 150 ms median over Wi-Fi 6 or USB |
| Android on the device | ≥ 15 swaps a second with 30 fps output, sustained 10 minutes. Below that, the mode stays off on that device |
| Phone as camera, glass to glass | ≤ 150 ms median |
| Scan transfer | 100 photos resumable; no re-upload after a drop |
| OCR on clean print, Vietnamese and English | ≥ 98 % character accuracy with diacritics intact; correct column order; no duplicated overlap text |
| Virtual camera soak | 60 minutes in Zoom and Teams without drift or drop-out |

## 8. Repository layout

```
gods-camera/
├─ core/            Rust workspace: gc-infer, gc-track, gc-align, gc-link,
│                   gc-models, gc-jobs, gc-doc, gc-llm, gc-safety, gc-cli, gc-ffi
├─ shaders/         WGSL sources, compiled to Metal / HLSL / GLSL ES at build
├─ api/             OpenAPI description of the hub
├─ apps/
│  ├─ apple/        macOS app, Camera Extension, shared Swift packages; iOS app later
│  ├─ android/      Compose app, native engine, bindings
│  └─ windows/      WinUI 3 app, engine DLL, media source DLL, installer
├─ tools/
│  ├─ modelkit/     Reference runs, model checks, pack building (Python)
│  ├─ bench/        Replay benchmark and latency rig
│  └─ corpus/       Manifests for test media (media kept outside git)
├─ packs/           Pack manifests (no weights in git)
└─ docs/
```

## 9. Phases

Sizes are rough and assume one developer working with an AI coding assistant. Testing on real hardware, not writing code, sets the pace. Every phase ends with tests, updated docs, and the safety checks for the features it adds.

### Phase 0 — Setup and spikes (1–2 weeks)

Status on 2026-11-11: complete. All four spikes pass and both Android devices are checked. Results are in [phase-0-results.md](phase-0-results.md).

Goal: a working environment, the models on disk, and the four riskiest assumptions tested on the Mac.

Setup on the Mac

- [x] Initialise git in this directory with the layout in section 8.
- [x] Free disk space to at least 60 GB.
- [x] Install cmake, Rust's Android target and cargo-ndk. The scripts set `ANDROID_HOME` themselves. The UniFFI tools arrive with the first bindings in Phase 3a.
- [x] Install cargo-xwin and LLVM, which the Windows cross-compile check needs.
- [x] Create a pinned Python environment with uv for `modelkit`.
- [x] Download `buffalo_l`, `buffalo_s` and `inswapper_128.onnx` from the InsightFace model-zoo release, and HyperSwap and XSeg from the FaceFusion assets. Record each SHA-256 in a pack manifest.
- [x] Reference run: InsightFace's Python package swaps a still image. The outputs are kept as golden files for parity tests.
- [x] Rust workspace skeleton that builds for macOS, Windows and Android; Xcode workspace with app and Camera Extension targets, signed with the existing certificates.

Device check-in

- [x] Check in the S22+ over USB: model number, chip, Android version, memory and ARCore support.
- [x] Check in the Tab S9+ the same way.
- [x] Smoke test: the core's unit tests pass on one Android device.
- [x] Cross-compile check: the core builds for Windows from the Mac. It runs on every merge from here on.

Spikes, each ending in a short written result and a go/no-go

- [x] **S1 macOS driver.** A test pattern fed through a Camera Extension appears in Zoom and in Google Meet in a browser, and the slate replaces it when the feed stops. A development-signed build activates with System Integrity Protection on. The wider app matrix and the latency measurement are part of Phase 1.
- [x] **S3 Swap speed on the Mac.** Run `inswapper_128` and HyperSwap 256 through ONNX Runtime's Core ML provider, at full and half precision, with the detector on a different compute unit. Record milliseconds, check half-precision output against full precision, and compare by eye with Picsi's Mac app.
- [x] **S4 Tracker.** Rust tracker and reprojection maths, meeting the tracker targets on synthetic sequences. The run against real clips follows once the corpus exists.
- [x] **S5 Hub on loopback.** Development-mode hub plus the command-line client: upload 100 images, run a job, and repeat an upload from the phone through `adb reverse`. Streaming video through the swap pipeline moves to Phase 2, where that pipeline is built.

Spikes that open later phases

- **S7 Phone link over Wi-Fi**, at the start of Phase 3c.
- **S2 Windows driver**, **S3 on the RTX 3050** and **S8 Shared renderer**, at the start of Phase 4.
- **S6 Android on-device runtime**, at the start of Phase 6b.

Exit: S1, S3, S4 and S5 pass; golden outputs exist; the Android device facts in section 4.1 are confirmed; the model stack in 6.5 is confirmed or adjusted. All of these are met as of 2026-11-11. S1's latency measurement moves to Phase 1, which adds the counters it needs.

### Phase 1 — macOS virtual camera and loop (3–5 weeks)

Goal: a Mac app that replaces the camera in any meeting app with a live feed, an image, a video or a loop. No machine learning yet.

Driver

- [x] Camera Extension with source and sink streams; 720p and 1080p at 30 to 60 fps.
- [x] Activation flow: Applications-folder check, button to the approval pane, upgrade and replace handling, uninstall, diagnostics export.
- [x] Placeholder slate when no frames arrive.
- [x] Start capture only while a client is streaming; per-client policy: block an app from the list of readers.

Engine without ML

- [x] AVFoundation capture from built-in, external and Continuity cameras into GPU-backed buffers; measured frame rate. Written; it has not run yet, because the app has no camera permission until you grant it.
- [x] Metal render graph: input, scale, crop, mirror, output pool, sink stream, no CPU copies.
- [x] Sources: live camera, still image, video file.

Loop

- [x] Record the output into a bounded clip store, up to 30 seconds, hardware-encoded.
- [x] Playback modes: forward with a cross-fade at the seam, ping-pong, automatic loop-point finder. Avoiding blinks at the seam needs eye landmarks and moves to Phase 2. Revised after your test: the finder keeps nearly the whole recording, a clip plays at the rate its frames arrived, and the way of repeating is chosen from how well the loop's end matches its start.
- [x] Controls: global hotkey and menu-bar toggle.
- [x] Return-to-live triggers: manual and optional voice activity. A face returning to the frame needs the detector and moves to Phase 2.
- [x] Local "looping" indicator; optional on-stream badge.
- [x] Named loop library with a hint when a saved loop's lighting no longer matches the room.

App shell

- [x] Menu-bar app and main window, preview, device picker, settings. Since version 0.4.0 the window has a sidebar with separate screens: Camera, Loop, Face, Scanner and Settings. Onboarding is the guidance on the Camera screen, not a separate screen.
- [x] Interface text in Vietnamese and English from the start.
- [x] File logging and a diagnostics report.

Checks only you can do, listed in [phase-1-results.md](phase-1-results.md)

- [x] Grant camera access, and confirm the live camera in the preview. Done in your first test.
- [ ] Meeting apps: live picture, loop and return to live in Zoom and in Google Meet; the hot keys; the menu-bar menu with the window closed. In your first test Zoom and Meet listed only the default camera: both apps had been open while the camera was updated. Quit and reopen them, then repeat this check.
- [ ] 60-minute soak with a meeting app reading the camera.

Exit: passthrough and loop pass the 60-minute soak in the app matrix; added latency within target. The automated part is met; the three checks above are open.

### Phase 2 — Live face swap on macOS (5–8 weeks)

Goal: the pipeline of section 6.4 at target quality and speed, with the safety layer in place.

State: the pipeline runs in the Mac app and meets its speed targets on a moving-face clip made from a photo. What is open needs your webcam clips, your eyes, or a later pack. Details are in [phase-2-results.md](phase-2-results.md).

Start

- [ ] Record the webcam clips with the app's own recorder from Phase 1: sitting still, fast head turns, a hand across the face, low light, glasses. This is the next thing I need from you.
- [x] Replay benchmark: `gc replay` runs the pipeline over a clip, paced or unpaced, and reports frame rate, latency percentiles, keypoint wobble and the largest keypoint step. So far it runs on a clip made from a photo.
- [x] Run the tracker on real face texture and compare with its synthetic results. It stays within 1 px for 3 seconds with no detector. The run on your clips follows.
- [ ] Add a recorded clip to the InsightFace reference run.

Perception, in the core

- [x] SCRFD pre-processing, decoding and suppression ported from InsightFace; parity-tested against the golden outputs.
- [x] Detection on the adaptive schedule with forced triggers.
- [x] Pin the detector's input shape. Both detectors now run on the CPU, where they are faster than Core ML ran them. The large detector at 640 px is the default; the choice is confirmed on your clips.
- [x] Tracker on the luma plane every frame, with smoothing and a hold-and-fade when the face is lost.
- [ ] Loop extras that need the detector, moved from Phase 1: keep blinks away from the loop seam, and return to live when a face comes back into the frame.
- [x] Fade to the real face at extreme angles. The angle is judged from the five keypoints rather than from the head-pose model, which is not needed for this.

Swap

- [x] Source face library: import photos, compute and store the identity vector. A face can be made from up to 20 photos of one person; their identity vectors are averaged, and photos that show somebody else are left out.
- [x] Aligned crop with InsightFace's template and transform. It is cut on the CPU in the core, which takes 0.1 ms; a GPU path is not needed.
- [x] Swap sessions behind the swapper contract: `inswapper_128` and HyperSwap 256.
- [x] Latest-wins queue with a face-jump guard.

Compositing

- [x] Inverse-warp paste with a soft mask in crop space, in the Metal shader.
- [x] Oval mask multiplied by the model's own mask and by track and pose fades.
- [x] Occlusion mask from the XSeg model, on its own thread beside the swap, so a hand or an object stays in front of the swapped face. Tuning on your clips is open.
- [x] Colour match and sharpening on the patch.
- [x] Option to show the real mouth through the swap. The same for the eyes is open.
- [x] Late reprojection on every output frame.

Quality and speed

- [ ] Optional enhancer on every second frame, re-pasting the cached result.
- [x] Half precision only for models that pass the comparison in S3.
- [x] Two swap sessions side by side: not needed. One session already delivers 27 to 30 swaps a second.
- [ ] First-run check that measures the device and sets a preset: Smooth, Balanced or Quality.
- [x] Benchmark thresholds kept in the repository and run before each merge, as tests on the moving-face clip.
- [x] The core's parity tests also pass on an Android device, including the real models; the core cross-compiles for Windows with its inference library.

Safety, required before daily use

- [x] First-use notice of the use policy.
- [x] Consent statement per face. A liveness check for "this is me" is open.
- [ ] A set of synthetic source faces generated for the app. It needs an image model, which arrives with Phase 5.
- [x] Visible AI indicator, on by default. The invisible watermark is open; content credentials wait for the first export feature.
- [x] Face data encrypted at rest; delete-everything control.
- [x] Option to blur the whole picture whenever the swap is not fully on the face, so the real face never shows. Off by default.

Checks only you can do

- [ ] Add your own face from a photo, turn the swap on, and judge the picture: lag during head motion, edges, colour. Your first verdict: it works, and the result could be better. Judge again with several photos of the face, the sharpness control and the occlusion mask.
- [ ] 30-minute soak in Zoom and in Teams with the swap on.

Exit: the Mac meets its targets in section 7; the face region shows no visible lag during head motion; 30-minute soak in Zoom and Teams. The measured targets are met; the two checks above are open.

### Phase 3 — Hub and Android companion (6–9 weeks)

Goal: scan documents into consolidated Vietnamese or English text, and use the phone as a camera and a remote.

State: the scanner works from end to end. Pictures sent from the Android app are read on the Mac and merged into one document. A phone or tablet connects to the Mac over Wi-Fi with a six-digit code. Phone-as-camera and the remote are open. Details are in [phase-3-results.md](phase-3-results.md).

3a. On localhost, no phone needed

- [x] Hub in development mode: HTTP and resumable uploads. The WebSocket for events and the OpenAPI description are open; clients poll for now.
- [x] Token, origin check, file-size cap, a time limit on the text reader, and generic errors on every route. The pixel-dimension cap is open.
- [x] Jobs run in the background with progress, are kept on disk, and run one heavy job at a time. Real priority for live video arrives with the video stream.
- [x] Command-line client covering every hub operation so far: `gc upload`, `gc scan`.
- [x] Document pipeline: OCR with Vision at the accurate level; layout and reading order; overlap removal by text alignment. Deskew, dewarp and contrast are left to Vision and to the phone's scanner for now.
- [ ] Review screen on the Mac: image region beside text, confidence highlighting, quick fixes. The app lists received documents and opens, copies or reveals them; the review screen itself is open.
- [x] Export to Markdown, plain text and JSON with geometry. DOCX and searchable PDF are open.
- [x] Test documents with a character-error measure: `make_document.py` renders a page with known text and overlapping, skewed shots of it. Photos of real newspapers are still to be added.
- [x] Android app in the emulator: Compose shell and hub client. It uploads pictures and follows a job. Loading the Rust core in the app waits for Phase 6, which is the first to need it; the core already builds for Android.

3b. With the Tab S9+ connected for debugging, by USB cable or Wi-Fi

- [x] A script that sets up `adb reverse`; a debug-only setting that allows unencrypted traffic to localhost.
- [x] Single documents with the ML Kit document scanner: edge detection, clean-up, gallery import. The scanner opens on the tablet; a real page has not been photographed with it yet.
- [ ] Long-document mode with CameraX: guided overlapping shots for newspapers and scrolls.
- [x] Import through the system photo picker; a share target so screenshots can be sent from any app. Batch auto-capture is open.
- [ ] On-device OCR preview with ML Kit, whose Latin model covers Vietnamese, so a page can be checked before moving on.
- [x] Uploads that resume after a dropped connection. Carrying on in the background is open.
- [ ] Phone as camera: CameraX into the hardware encoder into the hub stream, with lens, zoom and exposure controls. A camera foreground service keeps it running. The desktop lists it as an input source.
- [ ] Remote control screen: loop on and off, source face, preset.
- [x] Adaptive layout for the tablet. Redesigned for version 0.4.0: a connection card, large actions, numbered page thumbnails, two panes on a tablet.

Checks only you can do

- [ ] Connect the tablet to the Mac app with the six-digit code, then run the scanner with real pages: the document scanner, the photo picker, and a share from another app. I ran it on the tablet with test pages, over Wi-Fi, against a temporary hub.

3c. Paired mode over Wi-Fi

- [x] Pairing with a six-digit code shown on the Mac (SPAKE2, so the code is never sent), TLS with the hub's certificate pinned by the device, a token per device, a device list on the Mac, and removal that takes effect at once. Pairing by QR code and per-device permissions are open.
- [x] Discovery: the Mac announces the hub by Bonjour, and the Android app finds it with network service discovery, with typing an address as the fallback. macOS asks once for local-network access.
- [x] Limits on the network listener: 64 connections, 10 seconds for a handshake, two minutes of silence.
- [ ] Before use on a network you do not control: fuzzing of the hub's parsers (section 10.2).
- [ ] The app's minimum and build target are both API 36. Add Android 17's local-network permission flow before raising the target to 37.
- [ ] Codec negotiation and frame dropping tuned on real Wi-Fi.

Exit: 50 overlapping photos of a multi-column Vietnamese newspaper page, taken with the tablet, become one text that meets the OCR targets. Phone-as-camera meets the latency target over USB and over Wi-Fi.

**Phases 0–3 are the first usable product.** The exit of this phase was meant to be the Windows start point. You brought the PC in on 2026-11-12, before that, so Phase 4 now runs beside the rest of this phase (section 12).

### Phase 4 — Windows desktop (6–10 weeks; started 2026-11-12)

Goal: parity with Phases 1–3 on the Windows PC.

State: the toolchain is in place, the core passes its tests on the PC, and the models run on the RTX 3050 at camera speed. The virtual camera's driver is built and passes every check that needs no administrator; it waits for you to register it. The engine that feeds it is built in Rust and sends the face swap, a recording and a loop through the driver at 30 frames a second. The app's window does not exist yet. Details are in [phase-4-results.md](phase-4-results.md).

Start

- [x] Toolchain on the PC, as in section 5.3. Windows 11 Pro 25H2 build 26200.8655, NVIDIA driver 616.92. .NET 10 is still to be installed for the app shell.
- [x] The core's unit and parity tests pass on the PC: 208 in a full run.
- [ ] **S2 Windows driver.** Custom media source under Frame Server. Built, and measured with the driver loaded into a test tool: 1 to 3 ms from the app to the reader through a named pipe, so neither shared memory nor a shared texture was built. Open: registration, which needs an administrator, and with it the check under Windows' own camera service and in Zoom, Teams, Chrome, Edge, Camera and OBS.
- [x] **S3 on the RTX 3050.** `inswapper_128` and HyperSwap 256 at full, half and mixed precision, through DirectML and CUDA, with milliseconds, video memory and the error of each picture. DirectML with mixed-precision copies was chosen. TensorRT did not load in the comparison and was not pursued.
- [x] **S8 Shared renderer.** Decided without the wgpu test: on Windows the outgoing picture is drawn on the processor (`gc-render`), 3 to 10 ms a frame. The camera wants its pictures in ordinary memory, and the one graphics card is busy with the models. To be looked at again for Android.

Build

- [ ] Media Foundation virtual camera: media source DLL, machine-wide registration, session and system lifetime, NV12 and YUY2, placeholder slate, in-use detection. Built: the DLL, both formats at two sizes, the slate, in-use detection, the lifetime of the app's session, and a script for the registration. Open: the registration itself, and a camera that stays after the app closes.
- [x] Frame hand-off into Frame Server per the S2 result: a named pipe that the app owns, with its Rust end in `gc-win`. The real media source received every picture from it in the tests.
- [ ] Inputs: Media Foundation capture from the external camera, and the phone-as-camera link as a second source.
- [x] The outgoing picture, per the S8 result: `gc-render` draws it, and `gc-camera` is the pipeline around it, with the face swap, the "AI" badge, recording, and the loop with the Mac's rules.
- [ ] The app's model in `gc-app`: saved faces are done, encrypted as on the Mac. Open: the key in Credential Manager, settings, saved loops, and the interface the window calls.
- [x] Core inference on the card through DirectML, with separate sessions and queues per stage and no global inference lock. A model the card refuses runs on the processor instead.
- [ ] A fixed video-memory budget for the live pipeline. S3 measured about 0.7 GB for `inswapper_128` with the detector and the mask, and 1 GB with HyperSwap, of the card's 8 GB.
- [ ] OCR with PP-OCRv5; Vietnamese accuracy measured against the corpus.
- [ ] Hub on the shared core, WinUI shell, tray icon, hotkeys. The installer adds the firewall rule for the hub's network port, for private networks; Windows must never have to ask about it.
- [ ] Installer that registers the camera; file logging; single-instance guard.
- [ ] App matrix: Zoom, Teams, Meet in Chrome and Edge, Slack, Discord, OBS, Camera.

Exit: the exit criteria of Phases 1–3 are met on the Windows PC, against the Windows targets in section 7.

### Phase 5 — LLM layer and document intelligence (3–5 weeks, can overlap Phase 3)

Goal: local and cloud models behind one interface, used first to improve scanned documents.

- [ ] A settings screen where you enter each API key. The app stores it in the Keychain and never shows it again. The Terminal commands in [phase-0-results.md](phase-0-results.md) remain as an alternative.
- [ ] Provider gateway: Anthropic, Google, OpenAI and OpenAI-compatible endpoints. Keys read from the Keychain.
- [ ] Claude integration as described in 6.10; model picker from each provider's model list.
- [ ] Local models on the Mac: Apple's on-device model and the existing Ollama install.
- [ ] Embedded local runtime, so the app no longer depends on a separately installed Ollama.
- [ ] Phones and the command-line client use the desktop's local models and cloud providers through the hub.
- [ ] Document workflows: clean-up constrained to OCR text with a diff view, summary, translation between Vietnamese and English, extraction to a schema, questions about the document.
- [ ] A Vietnamese evaluation set that compares the local models and the three cloud providers; pick defaults by the results.
- [ ] Hard pages: cloud vision as an option; a local document-vision model if it measures well.
- [ ] Usage and cost meter; a local-only switch per feature.

Exit: a 100-image scan becomes a consolidated document with page and region references, fully offline on the Mac, or through any of the three cloud providers when switched on.

### Phase 6 — Face swap on Android (5–8 weeks)

Goal: live swap on the phone and tablet screens, first through a desktop, then on the device if it measures well.

6a. Through a desktop

- [ ] Detector and tracker running on the device from the shared core.
- [ ] Split rendering as in section 6.8, with frame IDs in the stream header.
- [ ] OpenGL ES compositing from the shared shaders; preview, photo and video modes; save to the gallery.
- [ ] Routes 1–3 of section 3.3: desktop camera, record and stream, clean full-screen output for screen-share.
- [ ] Check single-app screen sharing in Meet, Zoom and Teams on One UI.
- [ ] Frame rate that follows Android's thermal headroom.

6b. On the device (spike S6 first)

- [ ] **S6 bake-off** on the Tab S9+, and on the S22+ once it is updated: ONNX Runtime with Qualcomm's QNN provider on the neural processor, MNN and LiteRT on the GPU, and a CPU baseline. Record swaps per second, output parity and heat over 10 minutes.
- [ ] Turn on-device mode on only for a device that meets the target. Otherwise it stays with the desktop.
- [ ] Battery and thermal soak tests.

Exit: swap through a desktop works on both devices over Wi-Fi and USB. On-device mode either meets the target on a device or is off there, with the measurement recorded.

### Phase 7 — Meetings on the phone (optional, spikes first)

Goal: put the processed feed into real meetings from a phone, where vendors allow it.

- [ ] Spike: Zoom Meeting SDK for Android with a custom video source.
- [ ] Spike: Azure Communication Services joining Teams meetings with raw outgoing video.
- [ ] Spike: our own WebRTC rooms with browser guests.
- [ ] Spike: in-app browser overriding the web camera API.
- [ ] Build only the routes that work reliably.

### Phase 8 — 3D object scanning (4–6 weeks)

Goal: scan a small object with the phone and get a textured model on the desktop.

- [ ] Reconstruction on the Mac with `PhotogrammetrySession` in object mode, driven first from a folder of photos through the command-line client.
- [ ] Android capture: many photos with coverage hints, locked exposure and focus.
- [ ] ARCore poses and depth where the device supports them (to confirm for both devices).
- [ ] Per-image background removal, passed to the reconstruction as object masks.
- [ ] Real-world scale from ARCore depth where available; otherwise from one typed measurement or a reference card in the shot.
- [ ] Viewer with orbit and measure; export to USDZ, GLB, OBJ and PLY; LLM naming and description.
- [ ] Open pipeline on the Windows PC: camera poses, Gaussian-splat training, mesh extraction, texture bake.
- [ ] Optional pack: generative 3D from one or a few images, for incomplete captures.

Exit: a coffee cup photographed 40–80 times with the tablet yields a usable model on the Mac; time-to-model recorded.

### Phase 9 — Body, wardrobe and hairstyle try-on (5–8 weeks)

Goal: capture a person and their items, then generate try-on images.

- [ ] Capture flows with pose guidance for body (front and side), garments (worn, flat or on a hanger), accessories (photos or a Phase 8 scan), and hair.
- [ ] Wardrobe library with automatic tags for category, colour and material.
- [ ] One generation interface over several backends. Cloud image models from Gemini and OpenAI come first, since the keys exist.
- [ ] Local FASHN VTON v1.5 as the private path. Test it on the RTX 3050, where its stated 8 GB need is borderline, and on the Mac; keep whichever works.
- [ ] Hairstyle studio: style, length and colour presets, generated previews, side-by-side comparison.
- [ ] Live hair recolour by segmentation shader; face-anchored glasses and hats from scanned or stock 3D assets.
- [ ] Experimental body measurements from photos plus height, shown with uncertainty.
- [ ] Safety: output classifier against nudity, consent for photos of other people, encrypted storage, the per-feature cloud switch.

### Phase 10 — Generative studio and pro features (ongoing, optional)

- [ ] MiniMax H3 Ref2VA with the FaceSwap LoRA, through ComfyUI on a rented GPU or a hosted endpoint that accepts custom LoRAs: template workflow, 24 fps clip preparation, up to 4 reference faces, results into the loop library.
- [ ] A small distilled swap model for phones, trained from the larger models' outputs. InsightFace's own 512 px live model shows this is achievable.
- [ ] Personal trained model on desktop: our own training code, your own data.
- [ ] 512 px swap, multi-face mapping, per-user calibration, multi-photo identity.
- [ ] Automation: Shortcuts, command line, and an MCP server so assistants can run scans and exports.

### Phase 11 — iPhone and iPad (when you have one, 4–6 weeks)

- [ ] iOS app built from the Mac app's Swift packages: pairing, scanner with VisionKit, phone as camera, remote control.
- [ ] Swap through a desktop, then on the device through the Core ML provider if it meets the target.
- [ ] Guided Object Capture on LiDAR devices.
- [ ] Apple's on-device language model for quick tasks.
- [ ] Set the iPhone and iPad floors from the device you buy.

### Phase 12 — Vietnam release gate (only if you decide to release)

Nothing here is needed for personal use. All of it is needed before anyone else installs the app.

- [ ] Replace or license every pack without `commercial_ok`: detector, identity encoder, swap model, landmarks, occlusion mask, enhancer.
- [ ] Legal review against the primary texts: the AI law's marking and notice duties, its labelling decree, and the personal data law's rules for biometric data.
- [ ] Confirm that no code in the repository derives from the AGPL or GPL references.
- [ ] Security review of the hub: pairing, authentication, parsing, pack integrity.
- [ ] Security review of the Camera Extension: it accepts pictures from any local process, because macOS 27 does not tell a sandboxed extension who its clients are. Find a way to authenticate the app, or accept the risk in writing.
- [ ] Privacy review: data inventory, retention, deletion, consent records.
- [ ] Device coverage for the Vietnamese market: revisit the floors in section 4 and test on popular phones.
- [ ] Android store requirements: target API 37 with the local-network permission, and Google Play's policy on altered media.
- [ ] Distribution: stores or direct, notarisation, signed installers, update channels.
- [ ] Vietnamese help content, support process, abuse-report channel, pack revocation.

## 10. Cross-cutting checklists

### 10.1 Models and packs

- [ ] Every model file has a manifest with source URL, SHA-256, licence text and the `commercial_ok` flag.
- [ ] The app verifies hashes before loading a model.
- [ ] No weights in git; the pack store lives outside the repository.
- [ ] Local copies of every downloaded model are kept, since several upstream links are fragile.

### 10.2 Security and privacy

- [ ] The hub requires a token and passes an origin check in every mode, including development.
- [ ] Development mode binds to loopback only and refuses any other address.
- [ ] Cloud API keys live only in the desktop's Keychain or Credential Manager. They are never on a phone, in the repository or in logs.
- [ ] Face data, body photos and scans encrypted at rest.
- [ ] No telemetry.
- [ ] Hub fuzzing for the protocol and image decoders before paired mode is used on a shared network.

### 10.3 Quality and performance

- [ ] Parity tests against InsightFace's golden outputs for detector, alignment and swap.
- [ ] Core tests run on macOS and Android from Phase 0, and on Windows from Phase 4. Until then the core is cross-compiled for Windows on every merge.
- [ ] Replay benchmark with fixed thresholds on each reference device.
- [ ] Virtual-camera app matrix re-run after each OS update and each meeting-app major release.
- [ ] OCR corpus metrics tracked, Vietnamese first.
- [ ] Thermal and battery tests for the phone and tablet.

### 10.4 Safety

- [ ] AI indicator on by default wherever swapped or generated video leaves the app.
- [ ] Consent attestation recorded for every face source.
- [ ] Loop indicator always visible to the user.
- [ ] No generation of nudity; classifier on generated images.

## 11. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Swap model too slow on the RTX 3050 or on the Samsung devices | Low face refresh rate | Spikes S3 and S6; late reprojection hides low rates; phones offload; HyperSwap; a distilled model later |
| 8 GB of video memory on the Windows PC | Swap, a local LLM and try-on cannot share the GPU | Video-memory budget in the scheduler; one heavy job at a time; the Mac hosts the LLMs |
| Camera Extension activation problems on macOS 27 | Driver fails to load | Spike S1 first; diagnostics and guided recovery |
| macOS does not start an updated Camera Extension | Camera missing until the Mac restarts | Solved in Phase 1 by the hand-over in 6.6; the app shows restart advice if it ever fails |
| macOS 27 hides client identity from the extension | The extension cannot check who reads or writes | The app identifies processes for blocking; the unverified writer is on the Phase 12 security list |
| One developer, three platforms | Slow progress; platforms drift apart | Shared core and shaders; Windows cross-compile check from Phase 0; Windows begins only at its start point; renderer decision before Phase 4 |
| The S22+ is below the Android floor | Only one Android device for testing until it is updated | The tablet covers development; the update steps are written out |
| Disk space on the Mac | Builds and model downloads fail | Free 60 GB in Phase 0; keep packs on an external disk if needed |
| Model download links disappear | Cannot rebuild the pack store | Keep local copies with pinned hashes |
| Scope: about a year of solo work through Phase 9 | Never finishes | Phases 0–3 defined as the first product; later phases independent and optional |
| Mobile OS limits disappoint | Expectation gap | Routes 1–3 ship early; SDK routes are spikes |
| Cloud image models decline or alter edits of real people | Try-on and hairstyle previews fail | Several providers; the local FASHN path |
| Misuse for impersonation | Harm to others | Consent attestation, default indicator, watermark, client policy |
| A later release in Vietnam | Model licences and legal duties apply | Phase 12 gate; `commercial_ok` flag kept from day one |
| MiniMax H3 needs hardware neither desktop has | Feature unavailable locally | Rented GPU or hosted endpoint; optional phase |
| Python 3.14 too new for model tooling | Reference runs fail | Pinned older Python environment with uv |
| No iPhone or iPad | The Apple mobile app cannot be tested | Phase 11 waits; the Mac app's Swift packages are kept reusable |

## 12. Checkpoints

These are the points where I come back to you for a decision.

| Checkpoint | When | What you decide |
|---|---|---|
| Updating the S22+ | Whenever you choose | Nothing to decide in the plan. The phone joins testing once it is on Android 16 |
| Freeing disk space on the Mac | Before anything is deleted | What goes. The Xcode build cache (10 GB) is the obvious candidate |
| Windows start point | Decided on 2026-11-12: you brought the PC in, and Phase 4 began | Nothing further to decide |
| On-device swap on Android | After spike S6 in Phase 6 | Keep it for the devices that pass, or stay with the desktop |
| iPhone and iPad | When you own one | Start Phase 11 and set its floors |
| Release in Vietnam | When you decide to release | Start Phase 12 |

Why the plan put the Windows start point at the exit of Phase 3, and what starting early changes. The first two reasons were about work done twice. They still hold for the hub's video stream, which Phase 3 has not built yet: Windows takes that up only once the Mac has it. The engine and the pairing are settled, so the camera driver, the models on the card and the app shell can go ahead now.

- **Not earlier.** Before that, Windows would port an engine and a hub that are still changing, so the work would be done twice. Problems in the swap pipeline would also have to be debugged on two systems at once.
- **Ready by then.** The three things Windows reuses are settled: the live engine in the core and the shared shaders (Phase 2), and the hub interface (Phase 3).
- **Not much later.** Phases 8 and 9 have Windows parts: the open 3D pipeline, and local try-on on the NVIDIA GPU. Features built on the Mac alone have to be retrofitted, so Windows should start before Phase 8.
- **Nothing to do on the PC until then.** A cross-compile check on the Mac keeps the core portable in the meantime.

## 13. Sequencing

```
Phase 0 ─► Phase 1 ─► Phase 2 ─► Phase 3 ─►  first product (Mac + Android)
                                    │
                                    ├─► Phase 5 (LLMs)
                                    ├─► Phase 6 (swap on Android) ─► Phase 7 (optional)
                                    ├─► Phase 4 (Windows; the start point opens here)
                                    └─► Phase 8 (3D) ─► Phase 9 (try-on)

Optional, any time:         Phase 10 (generative, pro)
When you have an iPhone:    Phase 11
Only for a release:         Phase 12 (Vietnam gate)
```

- Recommended order: 0, 1, 2, 3, 5, 6, 4, 8, 9.
- Windows started on 2026-11-12, beside the end of Phase 3. Its camera driver, inference and app shell do not wait for anything; its phone-as-camera input waits for Phase 3 to build the video stream.
- Phases 5, 8 and 9 can each start from the command-line client, before the phone work they depend on is finished.
- First product: Phases 0–3, roughly four to six months of solo work. Phases 0–9: about a year.
