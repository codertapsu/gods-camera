# Phase 2 results

Date: 2026-11-11 · Plan: [implementation-plan.md](implementation-plan.md)

The live face swap works in the Mac app and meets its speed targets. The measurements below come from a moving-face clip made from a photo, not from a webcam, because the app has no camera access yet. What is still open needs your clips, your eyes, or a later pack.

## What exists now

| Area | State |
|---|---|
| Core | Detector, identity encoder and both swap models run through ONNX Runtime in the Rust core, on the Mac, on Android and in the Windows cross-build |
| Live engine | Tracking and smoothing on every frame; detector and swap model on their own threads; the newest swapped face is re-placed on every output frame |
| Mac app | **Face** section: swap switch, saved faces, add a face from a photo, quality choice, skin-tone match, AI badge |
| Safety | First-use notice, a consent statement for every face, "AI" badge on by default, faces encrypted on disk, **Delete all my data…** |
| Models in the app | The detector, the identity encoder and `inswapper_128` are inside the app, 470 MB, each checked against its manifest when the app is built |
| Tools | `gc swap` for still pictures, `gc replay` for the live benchmark |

The app is version 0.3.0 build 7. The Camera Extension is unchanged at build 6, so installing this version does not replace it.

## Measurements on the M4 Pro

`gc replay`, 1280×720 at 30 frames a second for 10 seconds, face moving as in ordinary talking, eyes 128 px apart.

| Measure | `inswapper_128` | HyperSwap 256 | Target |
|---|---|---|---|
| Time for one swap | 37 ms | 33 ms | — |
| Swaps a second | 27 | 30 | — |
| From a frame's arrival to its swapped face | 56 ms | 54 ms | ≤ 80 ms median |
| Age of the face on screen, median and 95th percentile | 2 and 3 frames | 2 and 2 frames | ≤ 80 and ≤ 110 ms |
| Frame thread, median and worst | 1.05 and 1.39 ms | 1.33 and 4.75 ms | tracker ≤ 2 ms |
| Face followed | every frame | every frame | — |
| Keypoint wobble | 1.2 px, 0.95 % of the eye distance | the same | — |

Other results:

- **Match with InsightFace.** On its sample photo the Rust pipeline finds the same six faces, and the swapped crop differs by 1.1 levels out of 255 on average. The target was 2.
- **Tracker alone.** Started once on the true keypoints and given no detections, it stays within 0.2 px on a still face, 0.6 px on a talking one and 1.0 px on a fast one, over 3 seconds.
- **In the installed app.** The self-test swaps a face through the app's own engine and the virtual camera link: 37 ms a swap, 26 a second, 0.9 ms on the frame thread. The first run of a model is slower, because Core ML compiles it once.
- **Tests.** 35 in the Swift package and 50 in the core, all passing. The core's check also builds it for Android and Windows with the inference library linked.

## On the tablet

The core's tests ran on the Galaxy Tab S9+. All pass except the one that needs live speed.

- The still-image pipeline with the real models matches InsightFace's output there too.
- The tracker gives the same results as on the Mac.
- Speed is the gap: with ONNX Runtime's plain CPU path, one detection takes 0.3 s and the first swap 2.7 s.

This confirms the plan's order. Phones use the desktop for the swap first, and the on-device runtime comparison stays in Phase 6.

## Decisions taken from measurements

| Question | Decision | Why |
|---|---|---|
| Where detectors run | On the CPU | The large detector takes 11 ms there, less than through Core ML, and the GPU and Neural Engine stay free for the swap |
| Which detector | Large, at 640 px | Half the keypoint wobble of the small one. To confirm on your clips |
| How detections are used | They nudge the tracked keypoints a tenth of the way; a clear disagreement replaces them | The tracker is steady and drifts slowly; the detector does not drift but is a pixel or two off each time |
| Where the crop is cut | In the core, from the frame's memory | It takes 0.1 ms, so a GPU path would add work for no gain |
| Head angle | Judged from the five keypoints | Enough for fading out at profile angles; the head-pose model is not needed |
| A second swap session | Not added | One already keeps up with the camera |

## One fix worth knowing about

The tracker treated motion under 0.05 px a frame as none, so a face drifting very slowly was not followed and ended up 2 px off. It now keeps its reference frame while at rest, so slow drift adds up until it is followed. Error on a still face fell from 2.1 px to 0.2 px.

## Open

**Needs your clips or your eyes**

- Record the webcam clips: sitting still, fast head turns, a hand across the face, low light, glasses.
- Tune the detector choice, the smoothing and the pull on those clips.
- The occlusion mask, so that a hand in front of the face stays in front of the swap.
- Judge the live picture, and run the 30-minute soak in Zoom and Teams.

**Not built yet**

- Keeping blinks away from the loop seam, and returning to live when a face comes back.
- Showing the real mouth or eyes through the swap; sharpening; the enhancer.
- A first-run check that picks a preset.
- An invisible watermark. Content credentials wait until there is something to export.
- A liveness check for "this is me".
- The set of synthetic faces, which needs an image model from Phase 5.
- More than one face. The swap follows one face; a second person in the picture stays as they are.

**Worth deciding together**

- When the swap is on and the face is lost or turned to profile, the real face shows. An option to blur the picture instead would hide it. It is not in the plan; say if you want it.

## Checks for you

These follow the Phase 1 checks, which grant the camera access the app needs.

1. In **Face**, click **Add a face…** and choose a clear photo of a face you may use. Choose the statement that applies and click **Save**.
2. Turn on **Swap my face**. Read the notice and click **I understand**.
3. Move your head: side to side, closer and further, a turn to profile. Tell me what you see at the edges of the face and whether the face lags behind the head.
4. Pass a hand in front of your face. The swap will draw over the hand for now; that is the open occlusion item.
5. Open Zoom with **God's Camera** selected and confirm the swapped face and the "AI" badge there.
6. For the sharper model, the app needs `hyperswap_1b_256.onnx` in `~/Library/Application Support/GodsCamera/Packs`. A link to your copy is already there on this Mac, so **Quality** offers **Sharper**.
7. Record the clips for me: with the swap off, click **Record a loop** and record each of the five situations for 10 to 20 seconds, then **Save** each with a name.
