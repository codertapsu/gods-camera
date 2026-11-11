# Phase 2 results

Date: 2026-11-11 · Plan: [implementation-plan.md](implementation-plan.md)

The live face swap works in the Mac app and meets its speed targets. You tried it on 2026-11-11: it works, and the result could be better. What was built in answer is in "After your first test" below. The measurements come from a moving-face clip made from a photo, not from a webcam. What is still open needs your clips, your eyes, or a later pack.

## What exists now

| Area | State |
|---|---|
| Core | Detector, identity encoder and both swap models run through ONNX Runtime in the Rust core, on the Mac, on Android and in the Windows cross-build |
| Live engine | Tracking and smoothing on every frame; detector and swap model on their own threads; the newest swapped face is re-placed on every output frame |
| Mac app | **Face** screen: swap switch, saved faces made from one or several photos, quality choice, sharpness, skin-tone match, hands and objects kept in front, real mouth, blur when the swap is off the face, AI badge |
| Safety | First-use notice, a consent statement for every face, "AI" badge on by default, faces encrypted on disk, **Delete all my data…** |
| Models in the app | The detector, the identity encoder, `inswapper_128` and the occlusion model are inside the app, 540 MB, each checked against its manifest when the app is built |
| Tools | `gc swap` for still pictures, `gc replay` for the live benchmark |

The app is version 0.4.0 build 8. The Camera Extension is unchanged at build 6, so installing this version does not replace it.

## After your first test

**Several photos for one face.** You can choose many photos of a person at once, or drop them on the Faces list, and add more to a saved face later.

- Each photo gives an identity vector. The vectors are averaged with equal weight, which keeps what the photos share, the person, and loses what differs: expression, angle, light.
- A photo that shows somebody else would pull the average away, so photos that disagree with most of the others are left out, and the sheet shows which.
- Measured on the sample photo: three versions of one person (as it is, mirrored, darkened) score 0.98 to 0.99 against the combined identity; another person scores −0.01. The line between "same" and "different" is 0.3.
- A face keeps a small picture of each photo, never the photo, and holds at most 20.

**Hands and objects stay in front.** A second model (XSeg) marks which pixels of the face crop are face. Whatever covers the face, a hand, a cup, a microphone, is cut out of the swapped face's mask.

- It runs on its own thread beside the swap model, on the processor the swap model leaves free, so the swap does not wait for it.
- Alone it takes 15 ms on the GPU, 30 ms on the Neural Engine and 36 ms on the CPU.
- In the test a bar across the mouth drops out of the mask while the eyes stay swapped.
- Cost, measured back to back on a quiet Mac with `gc replay`: with `inswapper_128` the swap takes 38 ms instead of 37 and stays at 26 a second. With HyperSwap 256 it takes 35 to 36 ms instead of 33, 25 to 28 a second instead of 30. The age of the face on screen does not change.

**Sharper.** The swapped crop is sharpened before it is drawn. The **Sharpness** slider sets how much; the middle is a mild default.

**Real mouth.** With **Show my real mouth**, your own lips and teeth show through the swap. Speech looks more natural; the mouth is then yours.

**Blur instead of your real face.** With **Blur the picture when the swap is not on my face**, the whole picture is blurred beyond recognition whenever a face is chosen and the swap is not fully on it: no face found, the head turned to profile, the face lost, or the first swapped picture still on its way. It is off by default.

- The blur leads: it is complete by the time the swapped face is half faded, and it starts the moment the face is lost, not after the short hold.
- In the test the picture was never sharp while the swap was under half strength, at the start and when the face left.
- The swapped face itself is drawn sharp on top of the blurred picture.

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
- **In the installed app.** The self-test swaps a face through the app's own engine and the virtual camera link. In version 0.4.0, with the occlusion mask on: 40 ms a swap, 25.5 a second, 1.3 ms on the frame thread, while an Android emulator was running on the same Mac. The first run of a model is slower, because Core ML compiles it once.
- **Tests.** 47 in the Swift package and 141 in the core, all passing. The core's check also builds it for Android and Windows with the inference library linked.

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
- Tune the detector choice, the smoothing, the pull and the occlusion mask on those clips.
- Judge the live picture again, and run the 30-minute soak in Zoom and Teams.

**Not built yet**

- Keeping blinks away from the loop seam, and returning to live when a face comes back.
- Showing the real eyes through the swap; the enhancer.
- A first-run check that picks a preset.
- An invisible watermark. Content credentials wait until there is something to export.
- A liveness check for "this is me".
- The set of synthetic faces, which needs an image model from Phase 5.
- More than one face. The swap follows one face; a second person in the picture stays as they are.

## Checks for you

These follow the Phase 1 checks, which grant the camera access the app needs.

1. On the **Face** screen, click **Add a face…** and choose several clear photos of one face you may use: different angles, expressions and light. Check which photos the sheet marks as used, choose the statement that applies and click **Save**.
2. Turn on **Swap my face**. Read the notice and click **I understand**.
3. Move your head: side to side, closer and further, a turn to profile. Tell me what you see at the edges of the face and whether the face lags behind the head.
4. Pass a hand in front of your face, and hold a cup in front of your mouth. Both should stay in front of the swapped face. Tell me where the edge looks wrong.
5. Turn on **Blur the picture when the swap is not on my face**, then turn to profile and cover your face. Others should see a blurred picture, never your face.
6. Open Zoom with **God's Camera** selected and confirm the swapped face and the "AI" badge there.
7. For the sharper model, the app needs `hyperswap_1b_256.onnx` in `~/Library/Application Support/GodsCamera/Packs`. A link to your copy is already there on this Mac, so **Quality** offers **Sharper**. Compare it with **Standard** and the **Sharpness** slider.
8. Record the clips for me: with the swap off, on the **Loop** screen, click **Record a loop** and record each of the five situations for 10 to 20 seconds, then **Save** each with a name.
