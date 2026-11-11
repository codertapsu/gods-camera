# Phase 0 results

Date: 2026-11-11 · Plan: [implementation-plan.md](implementation-plan.md)

Phase 0 is complete. All four spikes pass, and both Android devices are checked. Nothing is committed to git yet.

## Status of each item

| Item | Status | Notes |
|---|---|---|
| Repository and layout | Done | `git init` on branch `main`, no commits |
| Tools | Done | cmake, LLVM, cargo-ndk, cargo-xwin, XcodeGen, Rust's Android target |
| Python environment | Done | `tools/modelkit`, Python 3.12 through uv, ONNX Runtime 1.31.1, InsightFace 2.1 |
| Models | Done | In `../packs`, each with a manifest and a pinned SHA-256; `gc verify-packs` passes |
| Reference run | Done | Golden files from InsightFace's own code on its sample photo |
| Shared core | Done | Six crates, 32 tests, lint clean. Builds for macOS, Android arm64 and Windows x64 |
| Pre-merge check | Done | `tools/check.sh` runs tests, lint and both cross-builds |
| S1 macOS driver | Passes | Installed and enabled. You saw the test pattern and the slate in Zoom and in Google Meet in a browser |
| S3 Swap speed on the Mac | Passes | 34 to 37 ms a swap. One target reset. See below |
| S4 Tracker | Passes on synthetic sequences | 0.7 ms a frame on the Mac, 1.3 ms on the tablet, 1.8 ms on the phone |
| S5 Hub on loopback | Passes | Including uploads from the tablet and the phone through the cable |
| Galaxy Tab S9+ check-in | Done | Android 16. It is the development device, paired with the Mac |
| Galaxy S22+ check-in | Done | It runs Android 13, below the Android 16 minimum, until it is updated |
| Disk space | No action needed | 68 GB is free. Nothing was deleted |

Moved out of Phase 0, because nothing needs them yet:

- **Test clips and the replay benchmark** move to the start of Phase 2. The app's own recorder, built in Phase 1, records the clips.
- **API keys in the Keychain** move to the start of Phase 5.

## S1: the Camera Extension

- macOS reports the extension as activated and enabled, and "God's Camera" appears in the system's camera list next to the MacBook Pro camera.
- A build signed with the development certificate installs with System Integrity Protection on. Developer ID signing and notarisation are not needed for personal use.
- The app and the extension process were both running when I checked.

- You confirmed the colour bars, and the grey slate after **Stop test pattern**, in the Zoom app and in Google Meet in a browser.

One measurement moves to Phase 1: the added latency. It needs counters inside the extension, which Phase 1 adds.

## S3: swap speed on the M4 Pro

Measured with `tools/modelkit/check_swap.py` on a real aligned face crop: ONNX Runtime 1.31.1, 40 to 60 runs after warm-up. "Difference" is against the full-precision CPU output, in 8-bit levels. Raw numbers are in `spikes-s3-mac-real-crop.json`.

| Model | Core ML unit | Time per swap | Mean difference | Largest difference |
|---|---|---|---|---|
| `inswapper_128`, as downloaded | GPU | 54 ms | 0.0 | 0.0 |
| `inswapper_128`, as downloaded | Neural Engine | 210 ms | 0.0 | 0.0 |
| `inswapper_128`, half precision | Neural Engine | 126 ms | 0.45 | 2.0 |
| `inswapper_128`, padding rewritten, half precision | Neural Engine | 37 ms | 0.45 | 2.0 |
| `inswapper_128`, padding rewritten, half precision | GPU | 50 ms | 0.05 | 0.3 |
| HyperSwap 1b, 256 px | GPU | 34 ms | 0.02 | 0.5 |
| HyperSwap 1b, 256 px | Neural Engine | 57 ms | 0.05 | 0.8 |

On the CPU alone, `inswapper_128` takes 210 ms and HyperSwap 114 ms.

What this means:

- **Two usable configurations.** HyperSwap on the GPU at 34 ms (29 swaps a second, 256 px), and `inswapper_128` on the Neural Engine at 37 ms (27 swaps a second, 128 px).
- **Neither reaches the camera's 30 frames a second.** The output stays at 30 fps because the latest swapped face is re-warped onto every new frame. That step is required.
- **Half precision is safe for `inswapper_128` here.** The output differs from full precision by under half a level on average. I looked at the images: clean faces, no visible difference.
- **The padding rewrite matters only on the Neural Engine.** It changes nothing on the GPU, and cuts 126 ms to 37 ms on the Neural Engine.
- **HyperSwap on the GPU matches its CPU output** within half a level on a real face. My earlier worry came from testing with random noise.
- **HyperSwap is visibly sharper**, since it works at 256 px, and it returns its own face mask.
- **The two models use different units**, so running one of each kind side by side, or two sessions of one model, may lift the rate above the camera's. Phase 2 tries that.

One target is reset. The plan asked for a swapped face no older than 60 ms at the 95th percentile. With a 37 ms model, the age of the face when shown works out to about 80 ms on average: up to one frame of waiting, the swap itself, then display until the next swap lands. That is a calculation, not yet a measurement. The target is now 80 ms median and 110 ms at the 95th percentile. Head position is not affected, because the tracker corrects it on every frame; the lag shows only in expression.

Two earlier statements of mine were wrong:

- I said upstream's padding rewrite would halve `inswapper_128`'s time on the GPU. It does not. Their 27 ms was measured on an M3 Max, a chip with a much larger GPU than this M4 Pro, which is the likelier reason for the gap.
- I said the small detector fails under Core ML. Both detectors fail there at 320 px and work at 640 px. The stock models carry shapes fixed for 640 px.

### Detectors

| Detector | Input | CPU | Core ML |
|---|---|---|---|
| `det_10g` | 640 px | 38 ms | 18 ms |
| `det_10g` | 320 px | 8.4 ms | fails until the shapes are pinned |
| `det_500m` | 640 px | 4.7 ms | 7 ms |
| `det_500m` | 320 px | 1.6 ms | fails until the shapes are pinned |

The detector runs about four times a second, so any of these is affordable. Phase 2 pins the input shapes and picks the detector by how steady its keypoints are on real clips.

### Rebuilding the Neural Engine variant

```sh
cd tools/modelkit
uv run python rewrite_reflect_pad.py ../../../packs/inswapper_128.onnx /tmp/rf.onnx
uv run python convert_fp16.py /tmp/rf.onnx ../../../packs/inswapper_128.reflectfree.fp16.onnx
```

## S4: the tracker

The tracker follows a face between detector runs. It tracks the five keypoints and up to 40 textured points with pyramidal Lucas-Kanade flow, drops points that fail a forward-backward check, and fits one similarity transform with RANSAC.

Measured with `cargo run --release -p gc-track --example track_bench`: 300 frames of 640 x 360, a face moving up to 6 px a frame with rotation and zoom.

| Device | Time per frame | Mean error | Worst error | Lost |
|---|---|---|---|---|
| M4 Pro | 0.66 ms (0.75 ms at the 95th percentile) | 0.03 px | 0.07 px | 0 |
| Galaxy Tab S9+ | 1.3 ms (1.4 ms at the 95th percentile) | 0.03 px | 0.07 px | 0 |
| Galaxy S22+ | 1.8 ms (1.9 ms at the 95th percentile) | 0.03 px | 0.07 px | 0 |

- All three are inside the 2 ms target, and the error is far inside the 0.3 px target, with image noise added and without any re-anchoring across the 300 frames.
- These are synthetic scenes with exactly known motion. They measure accuracy, not robustness. Real clips, with blur, lighting changes and a hand across the face, are the Phase 2 test.
- The unit tests also cover a large jump, a still face under noise, part of the face moving differently, and a blank frame.

## S5: hub on loopback

The development hub is `gc hub`. It binds to 127.0.0.1 only and refuses any other address.

- On the Mac: 100 files of 3 MB uploaded and counted by a job. After one stored file was cut short, a second run sent only the missing part, and the result was byte-identical.
- From the tablet and the phone: with `adb reverse`, each uploaded 20 files (60 MB) through the cable to the loopback-only hub in about 2 s, which is 30 to 34 MB/s. All files arrived byte-identical. A wrong token was refused.
- Enforced and tested: a bearer token on every route but health, rejection of any browser origin, file-name checks, a per-file size limit applied while streaming, one error format with a request ID.

Not yet built: the live video stream, the WebSocket for events, persistent jobs and the OpenAPI description. Those belong to Phases 2 and 3.

## Parity with InsightFace

- The detector decoding finds the same 6 faces as InsightFace's Python code. Boxes and keypoints agree within 0.01 px, scores within 0.00001.
- The alignment matrix for the 128 px crop agrees within 0.0001 for rotation and scale, and 0.02 px for translation.
- All 32 tests also pass on the Tab S9+ and on the S22+.

The swap output itself is not yet compared in Rust, because the core does not run models yet.

## The Android devices

| Fact | Galaxy Tab S9+ | Galaxy S22+ |
|---|---|---|
| Model | SM-X816B, the 5G variant | SM-S906U, the US variant |
| Chip and GPU | Snapdragon 8 Gen 2, Adreno 740 | Snapdragon 8 Gen 1, Adreno 730 |
| Android | 16 (One UI 8.5), patch of September 2026 | 13 (One UI 5.1), patch of June 2023 |
| Memory | 10.9 GB usable | 7.1 GB usable |
| Free storage | 377 GB of 462 GB | 17 GB of 104 GB |
| Graphics | OpenGL ES 3.2, Vulkan 1.3 | OpenGL ES 3.2, Vulkan 1.1 |
| AR | Google Play Services for AR installed | Same |
| Knox | Not tripped | Not tripped |

- **The tablet is the development device**, paired with the Mac. It meets the Android 16 minimum.
- **The phone is below the minimum** until it is updated, so the app will not install on it yet. The minimum stays at Android 16 (API level 36), as you decided. The update steps are in [updating-the-galaxy-s22-plus.md](updating-the-galaxy-s22-plus.md).
- Neither device has Google's on-device model service installed, so on-device Gemini is not available on them.

## What I need from you

Nothing blocks the next phase. Phase 1, the real virtual camera app with the loop feature, can start when you say so.

## Adding the Claude API key

The key is not needed until Phase 5, and by then the app will have a settings screen for it. These Terminal steps are the alternative.

1. Get the key. Sign in to the Claude Console at https://platform.claude.com/settings/keys and create a key there, or use one you saved earlier. A key starts with `sk-ant-`. The Console shows a key only once, at creation, so if you no longer have the text, create a new one. A Claude.ai subscription does not include an API key; it comes from the Console.
2. Copy the key.
3. Open Terminal (press Command-Space, type `Terminal`, press Return).
4. Paste this line and press Return:
   ```sh
   security add-generic-password -a "$USER" -s godscamera.anthropic -U -w
   ```
5. Terminal shows `password data for new item:`. Paste the key with Command-V and press Return. Nothing appears as you paste; that is normal.
6. Terminal shows `retype password for new item:`. Paste the key again and press Return.
7. Terminal returns to the prompt without a message. That means it worked.
8. Check it, without showing the key:
   ```sh
   security find-generic-password -s godscamera.anthropic >/dev/null && echo stored
   ```
   It prints `stored`.

Notes:

- Running the line in step 4 again replaces the key. `security delete-generic-password -s godscamera.anthropic` removes it.
- The first time God's Camera reads the key, macOS asks whether to allow it. Choose **Always Allow**.
- The other two keys work the same way, with `godscamera.google` and `godscamera.openai` in place of `godscamera.anthropic`.
- Do not paste a key into a chat, a file in the repository, or a command line. The line in step 4 keeps it out of all three.
