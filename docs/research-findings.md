# God's Camera — Research Findings

Date: 2026-11-11, last revised after the owner's device list and follow-up answers · Companion to [implementation-plan.md](implementation-plan.md)

This document records what the reference repositories and web resources contain, what each platform allows, what the owner's four devices provide, and which facts still need confirmation. The plan builds on it.

## 1. What was reviewed

| Source | What it is | How it was checked |
|---|---|---|
| `../insightface` | deepinsight/insightface, HEAD 2026-10-19. The primary reference for the personal build | Python package, InspireFace SDK, server and model zoo read; key claims re-checked with grep |
| `../Deep-Live-Cam` | hacksider/Deep-Live-Cam (upstream), HEAD 2026-11-08 | Core modules read in full; key claims re-checked with grep |
| `../DeepLiveCam` | beekamai fork "3.2.0", HEAD 2026-09-16 | New and changed modules and `docs/` read |
| `../Deep-Live-Cam-Web` | tianxuanai fork with a browser front end, HEAD 2026-08-08 | `web_service/` read in full |
| `../tsaiface` | netexist's Windows re-skin, single commit 2026-11-09 | Every changed file diffed against upstream |
| `../faceswap` | deepfakes/faceswap, HEAD 2026-08-05 | `lib/align`, `lib/convert.py`, `plugins/`, docs |
| HackerNoon guide, Hugging Face `UntMods/FaceSwap_MiniMaxH3_REF2VA`, Civitai model 2940916 | A face-swap LoRA for the MiniMax H3 video model | Read in the browser through the Mother Claude plugin, then followed to the H3 model card, its licence and the shared workflow file |
| The development Mac | MacBook Pro, M4 Pro, 24 GB, macOS 27.2.1 | Read-only probes of OS frameworks and the toolchain; see section 8 |
| The owner's other devices | Windows 11 desktop with an RTX 3050, Galaxy S22+, Galaxy Tab S9+ | As reported by the owner. None was connected, so nothing was probed; public facts were checked by web search. See section 9 |
| Vendor documentation and 2026 reporting | Virtual cameras, model licences, regulation in Vietnam and the EU | Web search; secondary sources are marked |

Nothing from the repositories was executed. No file contained instructions aimed at an AI agent.

Labels used below:

- **Verified**: read in code, on the primary page, or measured on the development Mac.
- **Reported**: from a secondary source.
- **To confirm**: a working assumption that needs checking before we rely on it.

## 2. Headline conclusions

1. **Desktop camera replacement is supported by the operating systems.** macOS has the Core Media I/O Camera Extension and Windows 11 has `MFCreateVirtualCamera`. None of the reference repositories implements either. Upstream only shows a preview window and tells users to capture it with OBS (`Deep-Live-Cam/README.md:360`). The beekamai fork goes through `pyvirtualcam` to third-party drivers.
2. **Phones cannot replace the camera for other apps.** iOS, iPadOS and Android have no public API for it. I found no sign of this changing in iOS 27 or Android 17. Android 17 instead lets apps tell whether a camera is built-in, external or virtual (Reported). The Android tools that do replace the camera need root (Reported). The plan substitutes routes that do work on phones.
3. **The live face-swap pipeline is small and fully specified by InsightFace's own code.** Detect the face, align a 128 px crop from 5 keypoints, run an identity-conditioned swap model, paste the result back through the inverse transform with a soft mask. Section 4.8 has the exact constants.
4. **InsightFace's code is MIT and can be ported directly.** The Deep-Live-Cam variants are AGPL-3.0 and faceswap is GPL-3.0 (Verified). Those licences do not restrict private use, but porting their code would complicate any later release. We port from InsightFace and re-implement the ideas from the others.
5. **Model licences allow personal use and block a commercial release.** InsightFace's models (`inswapper_128`, `buffalo_l`) are for non-commercial research only (`insightface/README.md:42-44`). That fits a personal build. A release in Vietnam would need InsightFace's commercial licence or different models.
6. **The best engineering ideas are in the beekamai fork.** Optical-flow tracking between detections, late reprojection of the last swap onto the newest frame, one-slot queues, validated fp16, and a record-and-replay benchmark, all with measured gains.
7. **The Web fork shows what a desktop server must avoid.** It has no authentication, no origin check, one global source face, and JPEG-over-WebSocket transport. InsightFace's own server shows better patterns.
8. **The MiniMax H3 FaceSwap LoRA is an offline generative tool, not a live one.** It rewrites 4–15 second clips at 24 fps on a 33-billion-parameter video model. Its licence excludes the EU, UK, South Korea and the USA; Vietnam is inside the licensed territory. It fits pre-rendered clips, such as a loop. It will not run on the 24 GB Mac or on the 8 GB Windows GPU.
9. **Vietnam already requires labelling of AI-generated media.** The Law on Artificial Intelligence has applied since 1 March 2026, and the personal data law treats biometric data as sensitive. Neither binds a private, personal tool, but both apply to a release in Vietnam.
10. **Vietnamese is covered on the development Mac today.** The built-in OCR and the on-device language model both list Vietnamese (Verified, section 8).
11. **A 512 px real-time swap model exists but is not obtainable.** InsightFace's `inswapper-512-live` ships only inside its own macOS and iOS apps; the repository holds no weights (Verified).
12. **The owned devices decide where things run.** The Mac is the stronger machine for OCR and language models. The Windows PC adds CUDA with 8 GB of video memory. The two Samsung devices capture and offload; whether they can swap on the device is uncertain. Section 9 has the details.

## 3. Replacing the camera feed: what each platform allows

| Platform | Mechanism | Status |
|---|---|---|
| macOS | Core Media I/O Camera Extension (a system extension inside the app). The app feeds frames to the extension, which presents a camera to every client app. | Supported since macOS 12.3. Reports of activation failures on macOS 26.5 and a "process never launched" case on 27.0 mean activation needs careful handling and testing (Reported). |
| Windows | `MFCreateVirtualCamera` with a custom media source loaded by the Frame Server service. | Windows 11 (build 22000) and later. Only the software camera source type is supported. All-user registration needs administrator rights. Windows appends "Windows Virtual Camera" to the name (Verified in Microsoft docs). |
| iOS / iPadOS | None. Camera Extensions are macOS-only. | The only system-wide examples are jailbreak tweaks. |
| Android | None for ordinary apps. The virtual-device camera APIs are for privileged system apps (To confirm). | Android 17 adds vendor camera extensions and camera-type identification, not third-party injection (Reported). |

What works on phones instead, most reliable first:

1. Use the phone as a camera for the desktop app. The desktop's virtual camera carries the result into Zoom, Teams and the rest.
2. Record and export, or stream out over RTMP, SRT or WHIP.
3. Screen-share the app's clean full-screen output from the phone's own meeting app.
4. Join meetings inside our app through vendor SDKs that accept custom video (Zoom Meeting SDK, Azure Communication Services for Teams). Both need a spike on terms and review rules (To confirm).
5. Host our own rooms with WebRTC, with guests joining from a browser link.
6. An in-app browser that overrides `getUserMedia` for web meeting clients. Expect breakage.

Android specifics, since both of the owner's mobile devices run Android:

- The apps that replace the camera system-wide need root or Xposed-style hooks (Reported). Unlocking a Samsung bootloader permanently trips Knox.
- Since Android 14, an app that uses the standard screen-capture API can share a single app instead of the whole screen. Google Meet supports it; Zoom, Teams and Samsung's One UI are unconfirmed (Reported).
- Android 17 restricts local-network access. An app that targets API level 37 needs the `ACCESS_LOCAL_NETWORK` permission to reach devices on the LAN, including mDNS discovery (Reported, from the Android reference).

## 4. Reference repositories

### 4.1 Summary

| Repository | Code licence | UI | Virtual camera | What it offers | Use to us |
|---|---|---|---|---|---|
| insightface | MIT (models non-commercial) | Python library, C SDK, server | None | The original detector, aligner, identity encoder and swap code; a cross-platform C SDK; a recognition server | Port directly; source of models |
| Deep-Live-Cam (upstream) | AGPL-3.0 | PySide6 | None | The live pipeline around InsightFace, Apple Silicon work, async swap | Algorithm reference |
| DeepLiveCam (beekamai) | AGPL-3.0 | PySide6 | `pyvirtualcam` (Unity Capture, then OBS) | Tracking, reprojection, TensorRT, calibration, benchmark tools | Main source of performance ideas |
| Deep-Live-Cam-Web | AGPL-3.0 | Browser page | None | FastAPI + WebSocket wrapper, about 900 lines | Lessons for the hub server |
| tsaiface | AGPL-3.0 | PySide6, Chinese | None | Re-skin and packaging; no pipeline changes | UX and packaging ideas |
| faceswap | GPL-3.0 | Tkinter + CLI | None, offline only | Extraction, masking, compositing toolbox; per-identity training | Compositing and mask reference |

### 4.2 The baseline live pipeline (upstream Deep-Live-Cam)

| Step | How upstream does it | Where |
|---|---|---|
| Threads | Capture thread → queue of 2 (drop oldest) → processing thread → queue of 2 → UI timer | `modules/ui.py:1164-1431` |
| Capture | OpenCV: DirectShow then MSMF on Windows, AVFoundation on macOS; MJPG; real fps measured over a 30-frame burst | `modules/video_capture.py` |
| Detection | `det_10g` only, every `round(fps × 0.08)` frames (about 12 Hz); cached in between; leftmost face chosen | `modules/ui.py:1216`, `modules/face_analyser.py:193-216` |
| Identity | ArcFace `w600k_r50`, 512-d, computed once per source image | `modules/ui.py:1238-1259` |
| Alignment and swap | InsightFace's `INSwapper`; details in section 4.8 | `face_swapper.py:782-812` |
| Paste-back | Inverse affine; warp the face and a cached soft ellipse (axes 0.44 × size, Gaussian blur 31/σ12) into the face's bounding box only; alpha blend | `face_swapper.py:322-345, 452-512` |
| Async option | One-slot latest-wins swap queue; the newest result is pasted with the current frame's alignment; dropped if the face jumps | `face_swapper.py:639-766` |
| Enhancer | GFPGAN or GPEN, every 2nd frame, cached output re-pasted | `modules/processors/frame/face_enhancer.py` |
| Output | A Qt preview window | `modules/ui.py:1433` |

Apple Silicon details worth keeping (Verified):

- The swap model runs through ONNX Runtime's Core ML provider with compute units `ALL`; the detector is pinned to `CPUAndGPU` so the two overlap instead of queueing for the Neural Engine (`modules/face_analyser.py:100-117`, `face_swapper.py:266-287`).
- A reflect-`Pad` rewrite to Slice + Concat turns 14 Core ML partitions into 1 (`modules/onnx_optimize.py`).
- Author's numbers on an M3 Max, from a deleted doc in git history, not independently measured: swap model 55 → 27 ms, detector 21 → 4 ms, GFPGAN 155 → 89 ms.
- On this project's ONNX Runtime (1.31.1) and the M4 Pro, the padding rewrite changes nothing on the GPU: 54 ms before and after. It matters on the Neural Engine, where a half-precision `inswapper_128` takes 126 ms with the padding and 37 ms after the rewrite (Verified, [phase-0-results.md](phase-0-results.md)).

Weak points a native build should fix:

- Every frame makes several CPU copies; warps and blends run on the CPU.
- DirectML inference is serialised behind one global lock.
- Models download without hash verification.
- LAB colour transfer exists but is not wired into the live path.
- Tests are unit tests with mocks; there is no end-to-end or visual regression test.

### 4.3 Performance techniques (beekamai fork)

All figures were measured by the fork's author on one RTX 5060 Ti 16 GB (`DeepLiveCam/docs/performance.md`). Thresholds need re-tuning per device.

| Technique | Measured effect | Where |
|---|---|---|
| Whole-model TensorRT engines with static shapes | HyperSwap 9.5 → 6.2 ms (fp32) → 4.4 ms (fp16); InSwapper 19 → 6.2 ms; full pipeline 25.2 → 33.7 fps | `modules/providers.py`, `docs/performance.md:62-75` |
| fp16 only for models validated against fp32 | fp16 turned InSwapper, GPEN and XSeg output to "mush" without NaNs | `modules/providers.py:37` |
| Detect about 4 times a second, track in between; force detection on fast motion, a flow-versus-detection gap over 8 px, or 3+ dropped frames | Re-anchoring snap 12.9 → 8.7 px | `modules/ui.py:2376-2447` |
| Lucas–Kanade flow on half-resolution high-passed grey: 5 keypoints + 40 corners, forward-backward check, RANSAC similarity | About 1.5 ms; hand-occlusion drag 13 → 5.9 px mean | `modules/face_tracker.py` |
| One Euro filter with a 0.6 px deadband | Wander at rest 0.39 → 0.25 px | `modules/face_tracker.py:63-141` |
| Late reprojection: the display thread warps the last finished swap onto the newest camera frame | Position stays current even when the swap runs at 5–7 fps | `modules/reprojection.py`, `docs/reprojection.md` |
| One-slot drop-oldest capture queue | Camera-to-result latency 122 → 64 ms, same fps | `docs/performance.md:137` |
| Head pose computed on detection frames only and carried by the tracker | 3.85 → 1.49 ms per frame | `modules/head_geometry.py:49-55` |
| Poisson blend at quarter resolution | Swap stage 32 → 24.5 ms | `modules/processors/frame/face_swapper.py:73-118` |
| Occlusion mask every N frames, cached; model rewritten so every node runs on the GPU | 60 → 15 ms | `modules/face_occluder.py` |
| Skip the model when the swap alpha is near zero | Free on extreme head turns | `face_swapper.py:699-702` |
| Record a session, replay it through the real worker, unpaced and paced | Deterministic fps, latency and jitter numbers | `tools/record_session.py`, `tools/replay_bench.py` |

Quality features in the same fork: XSeg occlusion mask, a landmark-hull outline in place of the fixed ellipse, an "expression-proof" crop that keeps the eyes and nose fixed, blink-based eye reveal, lips-only mouth reveal, a fade to the real face at extreme angles, and several source photos blended by head pose.

How the fork feeds each swap model (Verified, `modules/swapper_registry.py`, `modules/processors/frame/swapper_models.py`):

| Model | Crop | Input range | Identity input | Output |
|---|---|---|---|---|
| `inswapper_128` | 128 px, ArcFace template | 0 to 1 | `w600k_r50` embedding × `emap`, normalised | 0 to 1 |
| HyperSwap 1a / 1b / 1c | 256 px, ArcFace template | −1 to 1 | Unit-length `w600k_r50` embedding | −1 to 1 |
| AlphaFace | 256 px, ArcFace template | 0 to 1 | Raw `w600k_r50` embedding | 0 to 1 |
| DeepFaceLive `.dfm` | Whole-face template, size from the model | 0 to 1 | None; identity is baked in | Image plus its own masks |

Things not to carry over:

- The global GPU lock. It works around an onnxruntime CUDA-graph limitation.
- CPU round trips for full frames.
- Sending frames to the virtual camera from the UI thread.
- The prebuilt `.pyd` inside the bundled insightface wheel.

### 4.4 Server lessons (Web fork)

How it works (Verified, `web_service/app.py` and `index.html`): the browser captures the camera, sends one JPEG per WebSocket message, waits for the swapped JPEG, then sends the next. One frame is in flight at a time.

| Observation | Consequence for our hub |
|---|---|
| No authentication, no TLS, no WebSocket origin check | Any web page could reach a localhost service. Every hub route needs a token and an origin check, in development too. |
| One global source face and one inference lock | State must live in sessions, with a scheduler deciding who gets the GPU. |
| JPEG in both directions, no frame IDs, metrics in a separate message | Use a hardware video codec for live video, and tag frames with IDs and timestamps. |
| No timeout: a lost reply stalls the stream forever | Heartbeats, timeouts and session resume. |
| Size limit checked after the body is read; no pixel-dimension cap | Check size while streaming; cap decoded dimensions. |
| Exception text returned to the client | Generic errors outward, details in local logs. |
| Fails fast if the accelerator silently fell back to CPU | Keep this, and report the active backend in the handshake. |

### 4.5 Productisation ideas (tsaiface)

The fork states that its swap algorithm is identical to upstream; the diff confirms it. Useful ideas:

- Separate Image, Video and Live pages with a shared source-face picker.
- Drop zones that show shooting advice: frontal face, even light, at least 720p.
- Enhancer choices described by trade-off ("natural, recommended", "sharpest, slower", "fast").
- One progress bar with the stage name and frame count; Start locked and quit guarded while a job runs.
- Models stored inside the app folder; log to a file when there is no console; single-instance guard; the detected accelerator shown in the UI.
- A check that the bundled ffmpeg matches the flags the app uses. ffmpeg 9 removed `-vsync`, and the failure was silent.

### 4.6 Compositing and masking toolbox (faceswap)

faceswap is an offline tool with three stages: extract, train a model per identity pair, convert. Training now runs on Keras 3 with a PyTorch backend. It has no live path and no model export.

Algorithms worth re-implementing, all cheap enough for a GPU shader because they run on the face patch:

| Algorithm | Where to read it |
|---|---|
| Similarity alignment (Umeyama) of 51 landmarks to a mean face | `lib/align/aligned_face.py:346-360, 522` |
| "Face" and "head" centring from a 3D pose fit, for stable crops under yaw and pitch | `lib/align/pose.py:165-225` |
| Landmark-hull masks, including an extended forehead | `lib/align/aligned_mask.py:426-632` |
| Feathered box × mask, erosion scaled by mask area, multi-pass box blur | `plugins/convert/mask/mask_blend.py:57-83, 206-266` |
| Reinhard LAB colour transfer and masked mean-colour shift | `plugins/convert/color/color_transfer.py`, `avg_color.py` |
| Quality filters that reject bad alignments (roll, scale, distance from mean face) | `lib/infer/align.py:859-990` |
| Order of operations at convert time | `lib/convert.py:280-313, 408-501` |

A "train a personal model" feature is realistic only as a later, desktop-only option. The docs quote 12–48 GPU hours and 500–5,000 images per identity, and we would have to write our own training and export code.

### 4.7 Safety mechanisms in the references

There are almost none:

- Upstream has an NSFW filter that is off by default (`modules/globals.py:35`), has no UI switch, and never runs in the live loop.
- None of the repositories watermarks output, records consent or marks provenance.
- The README disclaimers and faceswap's manifesto are text only.

### 4.8 InsightFace: the primary source for the personal build

**Licences (Verified)**

- Code: "released under the MIT License. There is no limitation for both academic and commercial usage" (`README.md:40`). This is stated in prose; there is no top-level LICENSE file.
- Models: "available for non-commercial research purposes only", whether downloaded by hand or by the library (`README.md:42-44`).
- Since 2025-11-24, the inswapper series and the `buffalo_l` pack each have a contact address for licensing (`README.md:48-49`).
- InspireFace's models are for academic use only (`cpp-package/inspireface/README.md:75`). That directory declares no licence for its code.

**Code to port (MIT)**

| What | Where |
|---|---|
| Alignment template and transform | `python-package/insightface/utils/face_align.py:6-35` |
| Swap pre-processing, latent and paste-back | `python-package/insightface/model_zoo/inswapper.py:19-108` |
| Detector decode, letterbox and non-maximum suppression | `python-package/insightface/model_zoo/scrfd.py:464-517, 671-743, 809-840` |
| Identity-encoder pre-processing | `python-package/insightface/model_zoo/arcface_onnx.py:41-42, 87-96` |
| 106-point landmark crop and 3D pose | `python-package/insightface/model_zoo/landmark.py:84-116` |

**Exact constants (Verified)**

- Alignment template on a 112 px canvas, in the order left eye, right eye, nose, left mouth corner, right mouth corner: (38.2946, 51.6963), (73.5318, 51.5014), (56.0252, 71.7366), (41.5493, 92.3655), (70.7299, 92.2041).
- Crop sizes that are multiples of 112 scale the template by size ÷ 112. Other sizes must be multiples of 128: scale by size ÷ 128, then shift x by 8 × that scale. So 128 px uses +8, 256 px uses ×2 and +16, 512 px uses ×4 and +32. Using the 112 px crop for the swap model is a known mistake.
- The transform is a least-squares similarity (rotation, uniform scale, translation).
- Swap input: RGB, 0 to 1, 128 × 128. Latent: the unit-length `w600k_r50` embedding multiplied by `emap` (the last initialiser stored in the model file), then normalised again. Output: RGB, 0 to 1.
- Paste-back: warp a white square through the inverse transform; erode with a square kernel of max(√area ÷ 10, 10) pixels; Gaussian-blur with a kernel of 2 × max(√area ÷ 20, 5) + 1; alpha-blend. The code also computes a difference mask but never uses it.
- Detector: input mean 127.5, scale 1/128, RGB. The image is resized into the top-left of the canvas and padded. Strides 8, 16, 32 with 2 anchors each. Score threshold 0.5, suppression threshold 0.4. Keypoints are anchor centre + prediction × stride.
- Identity encoder: 112 px crop, mean and scale 127.5, 512-d output.

**Model packs** (`python-package/docs/model_zoo.md`)

| Pack | Detector | Identity encoder | Landmarks | Size |
|---|---|---|---|---|
| `buffalo_l` (default) | SCRFD-10GF | ResNet-50, WebFace600K | 106-point 2D, 68-point 3D | 326 MB |
| `buffalo_m` | SCRFD-2.5GF | ResNet-50, WebFace600K | Both | 313 MB |
| `buffalo_s` | SCRFD-500MF | MobileFaceNet, WebFace600K | Both | 159 MB |
| `buffalo_sc` | SCRFD-500MF | MobileFaceNet, WebFace600K | None | 16 MB |
| `antelopev2` | SCRFD-10GF | ResNet-100, Glint360K | Both | 407 MB |

- Only the ResNet-50 encoder from `buffalo_l` or `buffalo_m` drives `inswapper_128` (`examples/in_swapper/README.md:26`). The small packs' MobileFaceNet embedding does not.
- Detector accuracy on WIDER FACE (easy / medium / hard): 500M with keypoints 90.97 / 88.44 / 69.49; 2.5G with keypoints 93.80 / 92.02 / 77.13; 10G with keypoints 95.40 / 94.01 / 82.80.

**Downloads**

- Packs: `https://github.com/deepinsight/insightface/releases/download/model-zoo/<name>.zip` (`python-package/insightface/utils/storage.py:10`).
- `inswapper_128.onnx` comes from the same release as a single file.
- Pack downloads carry no hash check, so we pin SHA-256 values ourselves.
- Several other weights in the wider model zoo sit on Google Drive, OneDrive or Baidu links, which are fragile.

**Recent changes** (package 2.1, 2026-10-03)

- Automatic Core ML, CUDA or CPU provider selection, with persistent Core ML compile caches.
- A desktop "Evaluation Studio" that includes face-swap trials.
- Detection runs at 128 px and 640 px and merges the results by default.
- The example for `inswapper_128` is marked no longer maintained, pointing to the Picsi.ai service.

**`inswapper-512-live`** (Verified on its GitHub page)

- The repository contains one README and no weights, releases or code.
- Two variants are described: base (9.5 million parameters, 12.1 GFLOPs) and mini (4.7 million, 6.3 GFLOPs), with 512 × 512 output.
- It claims under one tenth of `inswapper_128`'s compute for 16 times the pixels.
- It ships only inside Picsi's macOS app (Apple silicon, macOS 15+) and iOS app, "strictly for academic use or personal testing".
- Two implications: `inswapper_128` is a heavy model for its resolution, and a small high-resolution swap model is achievable. Picsi's app is the benchmark to compare against.

**InspireFace 1.2.4** (the C SDK in `cpp-package/inspireface`)

- What it does: detection, tracking, 106-point landmarks, head pose, quality, recognition, liveness, attributes, behind one C API with Swift and Java bindings.
- Platforms: macOS, iOS, Android, Linux, and Windows x64 on CPU only.
- Backends: MNN by default; Core ML and the Neural Engine on Apple; TensorRT on Linux.
- Tracking: a light mode detects every N frames (default 20) and follows landmarks in between.
- Stated speed with the Neural Engine: on an M2, detection at 320 px 1.07 ms and ResNet-50 embedding 3.53 ms; on an iPhone 13, 1.32 ms and 3.86 ms.
- Limits for our use:
  - It takes CPU buffers only.
  - Windows has no GPU path.
  - In tracking mode its five points are picked from the 106 landmarks, so they may not match the detector keypoints the swap model was trained with.
  - Its packs are opaque archives, so we cannot confirm that its encoder is `w600k_r50`.
- Conclusion: run the detector, encoder and swap model ourselves. InspireFace remains an option later for the liveness check in "this is me" enrolment.

**InsightFace Server 0.3.1** (`server/`)

- A self-hosted recognition server: FastAPI, SQLite with migrations, a REST API under `/v1`, no swap endpoints.
- Patterns worth borrowing for the hub: API keys stored as scrypt hashes and compared in constant time; a public health endpoint and nothing else public; a request ID on every response; one error format; origin checks; model installation that requires accepting the licence.
- Its own Compose files ship with authentication off, which we should not copy.

## 5. The MiniMax H3 FaceSwap LoRA

| Item | Finding | Source |
|---|---|---|
| What it is | A 62.6 MB LoRA (BF16 safetensors, trained 4,500 steps, published 2026-09-16) for the MiniMax H3 `ref2va` model. It replaces the face in a reference video with the identity from one or more reference images. | Civitai, Hugging Face |
| How it is used | In ComfyUI with CRT-Nodes or any Ref2VA workflow. Trigger word `Faceswap`, strength 1. The shared workflow loads the video at 24 fps capped at 192 frames (8 s), up to 4 reference images, and the clip's audio. | Workflow JSON |
| Base model | MiniMax H3: a 33-billion-parameter transformer with a Qwen3-VL-32B encoder. Output is 4–15 s at 24 fps, 768p by default, with stereo audio. The official serving example uses 4 GPUs. | H3 model card |
| Local hardware | Community guides put the quantised INT8 path at 16 GB VRAM minimum and 24 GB recommended; smaller cards rely on offloading. No measured generation times found. | Reported |
| Quality evidence | Example videos only. No identity or temporal-consistency metrics. One Civitai comment reports it sometimes changes only the hair. | HackerNoon, Civitai |
| Licence of the LoRA | Tagged Apache-2.0 on Hugging Face. Civitai states that downloaded weights fall under the MiniMax H3 Community License. | Both pages |
| Licence of the base model | The grant excludes the EU, UK, South Korea and the USA, so Vietnam is covered. Commercial products must show "MiniMax H3" in the UI and need written authorisation above USD 20 million yearly revenue. Outputs may not train other models. Anyone offering it to third parties must bind users to its use restrictions and run safeguards. | H3 licence text |
| Hosted option | MiniMax offers H3 through its API and applies automated moderation. | H3 model card |

Where it fits: an optional backend for offline clips, reached through a ComfyUI server on a rented GPU. Examples are a polished loop for the camera-loop feature or a swapped video for content. It cannot drive a live camera, and it cannot run on phones, on the 24 GB Mac or on the 8 GB Windows GPU.

Hosted routes are less certain. MiniMax's own API runs the base model and is unlikely to load a third-party LoRA. The HackerNoon guide names a hosted `h3/reference-to-video/lora` endpoint, which suggests some hosts accept custom LoRAs. Both points are to confirm.

## 6. Models and licences

| Need | Choice for the personal build | Licence | Personal use | Release in Vietnam |
|---|---|---|---|---|
| Detector | SCRFD `det_10g` on desktop, `det_500m` on phones | Non-commercial research | Yes | Licence from InsightFace or replace |
| Identity encoder | `w600k_r50` | Non-commercial research | Yes | Same |
| Swap, baseline | `inswapper_128` | Non-commercial research; licensing by contact | Yes | Licence or our own model |
| Swap, higher resolution | HyperSwap 1a / 1b / 1c at 256 px | ResearchRAIL (Reported) | Yes, as research use | Not without permission |
| Swap, 512 px live | `inswapper-512-live` | Not downloadable | No | No |
| Swap, baked identity | DeepFaceLive `.dfm` files | Per model | Yes | Per model |
| Landmarks and pose | `2d106det`, `1k3d68` | Non-commercial research | Yes | Replace with MediaPipe or Apple Vision |
| Occlusion mask | XSeg (`xseg_3`) | Not stated | Yes | Replace with MediaPipe segmentation |
| Enhancer | GPEN-BFR-256/512, GFPGAN 1.4 | Mixed; several non-commercial | Yes | Check each |
| Hair mask | MediaPipe segmentation | Apache-2.0 (To confirm) | Yes | Yes |
| Offline video swap | MiniMax H3 with the FaceSwap LoRA | Community licence; Vietnam inside the territory | Yes, on external hardware | Yes, under its terms |
| Garment try-on | FASHN VTON v1.5 | Apache-2.0 code and weights; about 2 GB on disk and about 8 GB of video memory | Yes | Yes |
| Garment try-on, others | IDM-VTON, OOTDiffusion, CatVTON, FitDiT | CC BY-NC-SA | Yes | No |
| Hair and outfit edits | Cloud image-edit models; local Qwen-Image-Edit | Provider terms; Apache-2.0 (Reported) | Yes | To confirm |
| 3D reconstruction | Apple Object Capture; Brush | OS-provided; Apache-2.0 (Reported) | Yes | To confirm |
| OCR | Apple Vision; on Windows PP-OCRv6's line detector with the AsterNet recogniser; ML Kit | OS-provided; Apache-2.0 and MIT; Google terms | Yes | Yes |
| LLMs | Apple's on-device model; open-weight models through Ollama or llama.cpp; cloud APIs | Varies | Yes | Per model |

Faceswap's extraction weights (S3FD, FAN, HRNet, BiSeNet-FP) carry no licence statement and are not needed.

PP-OCR's own recognisers cannot write Vietnamese, although their documentation lists the language (Verified 2026-11-12, from the character lists inside the published model files). `latin_PP-OCRv5_mobile_rec` has none of the 90 Vietnamese letters from U+1EA0 to U+1EF9; `PP-OCRv6_small_rec` and `PP-OCRv6_medium_rec` lack 88 of them. Their line detectors do not depend on the language and are used. Windows' built-in reader offers no Vietnamese on the PC. The comparison that led to AsterNet is in [phase-4-results.md](phase-4-results.md).

## 7. Regulation and store policy

None of this binds a private tool used only by its owner. It applies once the app is released in Vietnam. The EU items apply only if it is ever offered there.

- **Vietnam, Law on Artificial Intelligence (No. 134/2025/QH15)**, in force since 1 March 2026. Providers must mark AI-generated audio, images and video in a machine-readable format. Deployers must give clear notice when publishing AI-generated or edited content that could confuse people about authenticity (Article 11). Decree 142/2026/ND-CP on labelling is reported in force from 1 May 2026, focused on content that could mislead about whether a person really appeared or spoke. Transition periods for existing systems are reported to run into 2027 (Reported; check the primary text).
- **Vietnam, Law on Personal Data Protection (No. 91/2025/QH15)**, in force since 1 January 2026, with Decree 356/2025/ND-CP. Biometric data is sensitive personal data. Consent must be specific to each purpose and can be withdrawn. Holders of biometric data have extra security duties and breach-notification deadlines, and sensitive-data processing needs an impact assessment (Reported, law-firm summaries).
- **EU AI Act Article 50** has applied since 2 August 2026, with a grace period to 2 December 2026 for the marking duty on systems already on the market (Reported).
- **Google Play** has required apps that alter media to disclose or watermark the result when the alteration is not obvious (Reported from 2020; current wording to confirm).
- **Apple's guidelines** do not name deepfakes. Enforcement in 2026 has centred on sexual content: one study found most face-swap apps in both stores allowed nude swaps, and both companies are under pressure to remove "nudify" apps (Reported).

## 8. The development Mac (Verified 2026-11-11)

**Hardware and system**

- MacBook Pro with M4 Pro (8 performance and 4 efficiency cores), 24 GB memory, macOS 27.2.1.
- 32 GB free of 460 GB. About 10 GB of that disk is Xcode build caches, which can be cleared.
- One camera: the built-in MacBook Pro camera.
- System Integrity Protection is on. No camera extension is installed yet.

**Toolchain present**

- Xcode 27.2, Swift 6.4.2, Rust 1.99.1, Python 3.14.6, uv, Homebrew, Node 24, Java 26, Gradle 9.6.1, ffmpeg 8.1.2.
- Rust targets for macOS and for Windows (MSVC). No Android or iOS targets.
- Android Studio 2026.2. The Android SDK sits under `/opt/homebrew/share/android-commandlinetools` (9.7 GB): platforms for API 34 to 37, build tools 35 to 37, NDK 28.2, its own cmake 3.22.1, adb, and the emulator with an Android 16 arm64 image and one virtual device.
- Signing certificates of all three kinds: Apple Development, Apple Distribution and Developer ID Application. The camera extension can therefore be signed properly.
- Ollama 0.32.1 with one model, `qwen2.5:7b` (4.7 GB).
- iOS 27.2 simulators only (iPhone 14 through iPhone 18 Pro, current iPads).
- Zoom, Microsoft Teams, Chrome, Slack and Discord are installed for the test matrix.

**Missing**

- cmake on the PATH.
- Rust's Android targets, cargo-ndk and the UniFFI tools.
- cargo-xwin and LLVM, which cross-compiling the core for Windows from the Mac needs.
- The `ANDROID_HOME` variable.
- OBS.
- No InsightFace models are on disk.
- No API keys are set in the shell environment (`ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `OPENAI_API_KEY`).
- No phone or tablet was connected at any check.

**Probes**

| Probe | Result |
|---|---|
| Vision text recognition, accurate level | 33 languages, Vietnamese included |
| Vision text recognition, fast level | 6 languages, no Vietnamese. We must always use the accurate level |
| Vision document recognition (`RecognizeDocumentsRequest`) | Available; 33 languages, Vietnamese included |
| Foundation Models on-device language model | Available on this Mac; 24 languages, Vietnamese included |
| Object Capture reconstruction (`PhotogrammetrySession.isSupported`) | Supported on this Mac |
| Android emulator camera | The emulator lists the MacBook Pro camera as `webcam0` |

This Mac is the reference device for the Mac targets in the plan.

## 9. The owner's other devices (reported 2026-11-11)

The phone and the tablet were later connected and checked over USB. The Windows PC has not been connected, so its hardware details are as the owner reported them, and the rest comes from web search.

**Windows desktop**

- Windows 11 Pro 64-bit, Intel Core i5-13400F, 32 GB memory, NVIDIA GeForce RTX 3050 with 8 GB, and an external camera (as reported).
- The i5-13400F has no integrated graphics, so the RTX 3050 also drives the display and its 8 GB is shared with the Windows desktop.
- It has no neural processor, so Windows' on-device AI features for Copilot+ PCs are unavailable.
- The RTX 3050 supports CUDA, TensorRT and hardware video encode and decode.
- Nothing in the references measures swap speed on this class of card. The only GPU figures are from an RTX 5060 Ti (section 4.3), a considerably faster card.
- To record when Windows work starts: the Windows 11 version and the GPU driver version.

**Galaxy S22+ (8 GB / 128 GB)**, checked over USB on 2026-11-11 (Verified unless marked)

- Model SM-S906U, the US variant, with a Snapdragon 8 Gen 1 (SM8450) and an Adreno 730 GPU.
- Android 13 (API level 33), One UI 5.1, security patch of June 2023. The firmware build dates from May 2023.
- The firmware's sales code is `GCF`, a generic code that US models fall back to without a matching carrier SIM. Phones in that state commonly stop receiving over-the-air updates, which would explain the old version (my inference).
- One UI 8 (Android 16) is the newest version Samsung released for the S22 series; One UI 8.5 was not delivered to it (Reported).
- 7.1 GB of usable memory. 17 GB of storage free out of 104 GB.
- OpenGL ES 3.2 and Vulkan 1.1. Google Play Services for AR is installed, so ARCore is supported. Google's on-device model service is not installed.
- Knox has not been tripped.
- The core's 25 tests pass on it, and it uploaded 60 MB to the Mac's loopback hub through `adb reverse` at 34 MB/s.

**Galaxy Tab S9+ (12 GB / 512 GB)**, checked over USB on 2026-11-11 (Verified unless marked)

- Model SM-X816B, the 5G variant, with a Snapdragon 8 Gen 2 (SM8550) and an Adreno 740 GPU. The owner confirmed the model after first writing "Galaxy S9 Plus tablet".
- Android 16 (API level 36), One UI 8.5, security patch of September 2026. Its sales code is `XXV`, for Vietnam.
- One UI 9 (Android 17, API level 37) is its final major upgrade and began rolling out in late 2026, in South Korea first (Reported). It has not reached this tablet.
- 10.9 GB of usable memory. 377 GB of storage free out of 462 GB. The display is 1752 x 2800.
- OpenGL ES 3.2 and Vulkan 1.3. Google Play Services for AR is installed, so ARCore is supported. Google's on-device model service is not installed.
- Knox has not been tripped.
- The core's 32 tests pass on it. The tracker takes 1.3 ms a frame. It uploaded 60 MB to the Mac's loopback hub through `adb reverse` at about 30 MB/s.

**What Android offers on these devices**

| Topic | Finding | Status |
|---|---|---|
| Document scanning | ML Kit's document scanner supplies the whole capture flow: viewfinder, edge detection, clean-up, gallery import, a configurable page limit, JPEG and PDF output. Its models arrive through Google Play services | Reported from Google's docs |
| On-device OCR | ML Kit text recognition v2 lists Vietnamese in its Latin-script model | Reported from Google's language table |
| Neural processor | ONNX Runtime's QNN provider runs floating-point models in half precision on Qualcomm's neural processor. A community report confirms the Snapdragon 8 Gen 2; the 8 Gen 1 is unconfirmed | Reported |
| System language model | Published device lists for Gemini Nano through ML Kit name the Pixel 9 and 10, the Galaxy S25 and other recent models. Neither owned device appears | Reported |
| ARCore and its Depth API | Support for these two models was not confirmed by search. Google's device list is the authority | To confirm |
| Camera for the emulator | The Android emulator on the Mac can use the MacBook's camera | Verified |

**Local try-on on this hardware**

- FASHN VTON v1.5 has 972 million parameters, needs about 8 GB of video memory, and takes about 5 seconds on an NVIDIA H100. It uses bfloat16 on RTX 30-series cards and newer (Reported from the model card).
- On the RTX 3050 that is the whole card, shared with the Windows desktop, so it is borderline.
- Nothing found mentions Apple silicon support.

## 10. Items to confirm

- Swap-model speed through Core ML on the M4 Pro, through CUDA and TensorRT on the RTX 3050, and on the two Samsung devices. Nothing in the references measures this for our hardware.
- Whether a development-signed camera extension activates with System Integrity Protection on, or whether Developer ID signing and notarisation are needed even for private use.
- The Windows version on the PC.
- Whether FASHN VTON v1.5 fits in the RTX 3050's 8 GB, and whether it runs on Apple silicon.
- Whether single-app screen sharing works in Zoom and Teams on One UI.
- Whether loopback traffic through `adb reverse` is exempt from Android 17's local-network restriction, as expected.
- Whether a clean Windows 11 install decodes HEVC in hardware without the store extension. The phone link falls back to H.264 otherwise.
- Which Python version the model-conversion tools support; Python 3.14 may be too new for some.
- The route from an ONNX file to a native Core ML model, if ONNX Runtime's Core ML provider proves too slow.
- Licence text for the MediaPipe models, Brush, HyperSwap and each enhancer, before any release.
- Vietnamese accuracy of PP-OCRv5 and of local vision-language models. PP-OCRv5's Latin model lists Vietnamese; one benchmark reports that open vision-language models trail proprietary ones on Vietnamese text.
- The Windows path for getting frames from a user-session app into the Frame Server process.
- Whether Zoom's Meeting SDK and Azure Communication Services accept our use of custom video.
- The primary text of Vietnam's AI law, its labelling decree and the personal data law, before a release.

## 11. Sources

Web resources named in the brief:

- [FaceSwap MiniMax H3 LoRA: A Practical Guide (HackerNoon)](https://hackernoon.com/faceswap-minimax-h3-lora-a-practical-guide-to-face-replacement)
- [UntMods/FaceSwap_MiniMaxH3_REF2VA (Hugging Face)](https://huggingface.co/UntMods/FaceSwap_MiniMaxH3_REF2VA)
- [FaceSwap – MiniMax REF2VA (Civitai)](https://civitai.com/models/2940916/faceswap-minimax-ref2va)
- [MiniMaxAI/MiniMax-H3 model card](https://huggingface.co/MiniMaxAI/MiniMax-H3) and [licence](https://huggingface.co/MiniMaxAI/MiniMax-H3/blob/42ed227ee7df40d41602854ae760620d6eb651fe/LICENSE)

Platforms:

- [Create camera extensions with Core Media IO (WWDC22)](https://developer.apple.com/videos/play/wwdc2022/10022/)
- [CMIO system extension activation on macOS 26.5 / 27 (Apple forums)](https://developer.apple.com/forums/thread/840312)
- [OBS virtual camera on macOS 27.0 (OBS forum)](https://obsproject.com/forum/threads/macos-27-0-%E2%80%93-could-not-find-the-virtual-camera-system-process-%E2%80%93-camera-extension-is-activated-enabled-but-never-launched.196433/)
- [MFCreateVirtualCamera (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nf-mfvirtualcamera-mfcreatevirtualcamera) and [IMFVirtualCamera](https://learn.microsoft.com/en-us/windows/win32/api/mfvirtualcamera/nn-mfvirtualcamera-imfvirtualcamera)
- [Android 17 vendor-defined camera extensions (Android Authority)](https://www.androidauthority.com/android-17-beta-3-vendor-defined-camera-extensions-3652506/)
- [Virtual-Cam-iOS16, a jailbreak-only system-wide virtual camera](https://github.com/xyzl1789/Virtual-Cam-iOS16)
- [Discover area mode for Object Capture (WWDC24)](https://developer.apple.com/videos/play/wwdc2024/10107/) and [PhotogrammetrySession errors with area-mode images (Apple forums)](https://developer.apple.com/forums/thread/794169)

Models and licences:

- [deepinsight/inswapper-512-live](https://github.com/deepinsight/inswapper-512-live)
- [InsightFace face swapping: licensing and API](https://www.insightface.ai/solutions/face-swapping)
- [FaceFusion mobile model sets, licence listing (Hugging Face)](https://huggingface.co/AbrahamPJ/facefusion-mobile-models)
- [faceswap-api, a list of swap models in use](https://github.com/ashleykleynhans/faceswap-api)
- [FASHN VTON v1.5 open-source release](https://fashn.ai/blog/fashn-vton-1-5-open-source-release)
- [Open virtual try-on models sorted by licence](https://github.com/XQ0820/open-virtual-try-on-models)
- [MiniMax H3 in ComfyUI (ComfyUI wiki)](https://comfyui-wiki.com/en/tutorial/advanced/video/minimax/minimax-h3), [Atlas Cloud guide](https://www.atlascloud.ai/ja/blog/tips/minimax-h3-comfyui-workflow), [Seedance local setup guide](https://www.seedance.tv/ja/blog/minimax-h3-local-setup-guide)
- [Brush brings Gaussian splatting to every GPU (AlphaSignal)](https://alphasignal.ai/news/brush-brings-3d-gaussian-splatting-to-every-gpu-without-cuda.md)
- [Bringing Gemini models to Apple developers (Google)](https://blog.google/innovation-and-ai/technology/developers-tools/bringing-gemini-models-to-apple-developers/)

Android, Samsung and local try-on:

- [Galaxy S22+ SM-S906E specifications (PhoneMore)](https://www.phonemore.com/specs/samsung/galaxy-s22-plus/sm-s906e-128gb/)
- [Galaxy S22+ specifications (GSMArena)](https://m.gsmarena.com/samsung_galaxy_s22+_5g-11252.php)
- [With One UI 8 out, what is next for the Galaxy S22 series (SamMobile)](https://www.sammobile.com/news/galaxy-s22-plus-ultra-one-ui-8-update-out-whats-next/)
- [Which Galaxy devices receive One UI 8.5 (Gizmochina)](https://www.gizmochina.com/2026/04/22/will-your-samsung-galaxy-receive-one-ui-8-5-update-check-here/)
- [Devices eligible for One UI 9 and Android 17 (SamMobile)](https://www.sammobile.com/news/samsung-galaxy-phone-tablet-one-ui-9-0-android-17-eligibility/)
- [One UI 9 stable rollout reaches the Galaxy Tab S9 (NPowerUser)](https://nokiapoweruser.com/samsung-one-ui-9-rollout-galaxy-tab-s9-watch-8-m15/)
- [Galaxy Tab S9 line-up reaches its final Android upgrade (HiTech Reports)](https://hitechreports.com/news/samsung-galaxy-tab-s9-lineup-reaches-its-final-android-upgrade)
- [ML Kit document scanner (Google)](https://developers.google.com/ml-kit/vision/doc-scanner)
- [ML Kit text recognition v2 languages (Google)](https://developers.google.com/ml-kit/vision/text-recognition/v2/languages)
- [ML Kit GenAI APIs overview (Google)](https://developers.google.com/ml-kit/genai)
- [ARCore supported devices (Google)](https://developers.google.com/ar/devices)
- [QNN execution provider (ONNX Runtime docs)](https://onnxruntime.ai/docs/execution-providers/QNN-ExecutionProvider.html)
- [QNN execution provider notes (onnxruntime-qnn on GitHub)](https://github.com/onnxruntime/onnxruntime-qnn/blob/main/docs/execution_providers/QNN-ExecutionProvider.md)
- [ONNX Runtime on Qualcomm devices, community tutorial](https://github.com/DakeQQ/Tutorial-ONNX-Runtime-Execution-Providers/blob/main/Qualcomm/README.md)
- [NsdManager reference, local-network restriction from API 37 (Android Developers)](https://developer.android.com/reference/android/net/nsd/NsdManager)
- [Android 17 local network permission, tested (salari.dev)](https://salari.dev/writing/android-17-local-network-permission-tested/)
- [Enhanced screen sharing in Android 14 (Android Developers Blog)](https://android-developers.googleblog.com/2024/03/enhanced-screen-sharing-capabilities-in-android-14.html)
- [Virtual camera on Android, maintainer discussion (Webcamoid)](https://github.com/webcamoid/webcamoid/discussions/746)
- [MYCAM, a root-based Android virtual camera](https://github.com/elementtime6969/mycam)
- [FASHN VTON v1.5 repository](https://github.com/fashn-AI/fashn-vton-1.5) and [project page](https://fashn.ai/research/vton-1-5)

Vietnamese text:

- [PP-OCRv5 multilingual text recognition (PaddleOCR docs)](https://www.paddleocr.ai/latest/en/version3.x/algorithm/PP-OCRv5/PP-OCRv5_multi_languages.html)
- [VMMU: a Vietnamese multimodal benchmark (arXiv)](https://arxiv.org/pdf/2508.13680)
- [A Survey on Vietnamese Document Analysis and Recognition (arXiv)](https://arxiv.org/pdf/2506.05061)
- [VietOCR](https://vietocr.com/)

Regulation and policy:

- [Vietnam: the first Law on Artificial Intelligence (Duane Morris)](https://blogs.duanemorris.com/vietnam/2026/03/03/vietnam-the-first-law-on-artificial-intelligence-what-you-must-know/)
- [Vietnam AI law takes effect (France 24)](https://www.france24.com/en/live-news/20260228-vietnam-ai-law-takes-effect-first-in-southeast-asia)
- [Vietnam Law on Artificial Intelligence, article summary (EveryAILaw)](https://everyailaw.com/regulation/vn-ai-law/index.html)
- [Vietnam mandates AI content labelling from March 2026 (Báo Nghệ An)](https://baonghean.vn/en/viet-nam-bat-buoc-gan-nhan-noi-dung-ai-tu-thang-3-nam-2026-theo-luat-tri-tue-nhan-tao-10325945.html)
- [Data protection in Vietnam (DLA Piper)](https://www.dlapiperdataprotection.com/?c=VN&t=law)
- [Vietnam's Law on Personal Data Protection (Vietnam Briefing)](https://www.vietnam-briefing.com/news/vietnam-law-on-personal-data-protection-latest-developments-and-insights.html/)
- [Law on Personal Data Protection No. 91/2025/QH15 (LuatVietnam)](https://english.luatvietnam.vn/dan-su/law-on-personal-data-protection-law-no-91-2025-qh15-405135-d1.html)
- [EU AI Act: what still applies on 2 August 2026 (Jones Walker)](https://www.joneswalker.com/en/insights/blogs/ai-law-blog/yes-august-2-still-matters-the-eu-approved-a-high-risk-ai-delay-but-most-trans.html)
- [EU AI Act on 2 August 2026 and the Omnibus (Legalithm)](https://www.legalithm.com/en/blog/eu-ai-act-what-applies-2-august-2026)
- [Google Play policies on deepfakes (TechCrunch, 2020)](https://techcrunch.com/2020/04/16/new-google-play-policies-to-cut-down-on-fleeceware-deepfakes-and-unnecessary-location-tracking)
- [Most face-swap apps in the stores could make deepfake nudes (Indicator)](https://indicator.media/p/most-faceswap-apps-in-the-apple-and-google-stores-could-be-used-to-make-deepfake-nudes)
- [Apple and Google hit with demand to pull "nudify" apps (MacRumors)](https://www.macrumors.com/2026/07/17/demand-to-pull-nudify-apps/)
