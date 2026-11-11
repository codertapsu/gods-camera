# Phase 1 results

Date: 2026-11-11 · Plan: [implementation-plan.md](implementation-plan.md)

Phase 1 is built and its automated checks pass. You tested it on 2026-11-11 and found two problems, which are fixed; see "After your first test" below. The checks at the end still need you.

## What exists now

The Mac app is now version 0.4.0 build 8. Its Camera Extension is unchanged at build 6, so installing the app does not replace the camera.

| Area | State |
|---|---|
| Virtual camera | 1280×720 and 1920×1080, 30 to 60 frames a second, with a slate when the app sends nothing |
| Picture sources | Camera, still image, video file (looped), test pattern |
| Loop | Record up to 30 seconds, play forward with a cross-fade or back and forth, automatic loop points and automatic choice between the two, saved loops with names |
| Controls | A window with separate screens (Camera, Loop, Face, Scanner, Settings), menu-bar menu, ⌃⌥⌘L for the loop, ⌃⌥⌘R for recording |
| Return to live | By hand, or when the microphone hears you speak (off by default) |
| Apps reading the camera | Listed by name; each can be blocked |
| Languages | English and Vietnamese, 71 strings and both permission texts |
| Diagnostics | Log file in `~/Library/Logs/GodsCamera`, and a report from the window |

## Measurements

All from the app's self-test, which sends the test pattern through the real engine and the installed extension.

| Measure | Result |
|---|---|
| Render time for one frame | 0.7 to 1.0 ms |
| Frames delivered in a 10-minute run | 18,001 of 18,000 expected, none dropped |
| Frames the extension counted in that run | 18,181 of 18,210 sent; the rest fall in its last unreported second |
| Memory after 3 seconds, and after 10 minutes | 158 MB, 160 MB |
| Update from one extension build to the next | 14 seconds in total, no restart of the Mac |
| Old extension standing down during an update | 0.66 seconds |
| From the engine's output to an app reading the camera, median and 95th percentile | 6.3 ms and 8.8 ms. Targets: 12 and 25 ms |
| From capture to the extension handing the frame on | 2.9 ms, at most 4.8 ms |

Tests: 47 in the Swift package now, all passing. For this phase they cover the renderer, the sources, clips and their real frame rate, loop points, the counters, app identification and the voice detector.

The last two rows were measured on 2026-11-11, after you granted camera access: the app read its own virtual camera back at 1920 pixels wide, the way a meeting app does, and received 598 of 598 frames.

## After your first test

**Zoom and Google Meet listed only the default camera.** Both apps had been running while I updated the camera extension, five times that afternoon. An app asks macOS for its cameras when it starts; one that is running during an update is left with a dead entry and does not get the new camera until it is quit and opened again. An app started afterwards lists God's Camera, which is why the app's own read-back and my checks passed.

- What to do: quit Zoom and Chrome completely (⌘Q, not only the window) and open them again.
- In the app: the Camera screen now names the running apps that were opened before the camera was last installed or updated, and offers to quit each.
- The camera's version changes only when its own code changes, so ordinary app updates do not cause this.

**A five-second loop flashed at its start.** The loop finder looked for the two most similar moments anywhere in the recording, which were always close together: it looped one second out of five, so the picture jumped back every second. A second fault made it worse on a dim camera: a clip was labelled with the expected 30 pictures a second even if the camera delivered 15, and then played at double speed.

- The loop now keeps nearly the whole recording: it starts within the first sixth and ends within the last sixth.
- A clip plays at the rate its frames arrived, and the seam fades over about a third of a second at any rate.
- **Repeat › Automatic** plays start to end when the loop's end matches its start, and back and forth when you moved in between. Back and forth has no jump at all.

## Three things macOS 27 does differently from what the plan assumed

**1. An updated extension is not started.** When macOS replaced build 1 with build 2, it tried to start the new one while the old one was still stopping, received "Operation already in progress", and did not try again. The camera vanished until a restart.

- The fix: the app asks the old build to stand down before the update, and the old build refuses to come back for 12 seconds.
- An app that was running during the update cannot see the new camera, so the app reopens itself once.
- Tested three times, from build 3 to 6. Each update finished without a restart.
- If it ever fails, the window says so and advises a restart.

**2. Properties of the camera are cached.** The app reads the extension's counters through a custom property. macOS keeps a copy on the reading side and does not ask again. The extension now announces its counters once a second, and at once when an app starts or stops reading.

**3. The extension is not told which app is reading.** It receives a process number and "unknown" as the name. Its sandbox also stops it from looking the process up.

- Blocking therefore works from the app's side. The app can see which app a process belongs to, including a browser's helper processes. It sends the extension the process numbers to refuse.
- While a blocked app is already reading, the app sends no pictures to anyone, because all readers receive the same pictures.
- For the same reason, the extension cannot confirm that pictures come from the God's Camera app. Any program on the Mac could write to the camera. This is acceptable for personal use and is on the security list for a release.

The extension also runs as a system user, so it cannot share settings with the app. Everything passes through the camera's properties.

## Differences from the plan

| Planned | Done instead | Why |
|---|---|---|
| Shaders in WGSL, cross-compiled | Written directly for Metal | One platform so far. The shared form is decided before Phase 4 |
| Loop point avoids blinks | Loop point by picture similarity | Blinks need eye landmarks. Moved to Phase 2 |
| Return to live when a face comes back | Not yet | Needs the detector. Moved to Phase 2 |
| A separate onboarding screen | Guidance inside the window | Enough for one user |
| Camera at 60 frames a second | The camera's own default rate | Format selection is untested without camera access. Video files and the test pattern do reach 60 |

## Not exercised by me

These are written but I could not run them without you:

- The real camera as a source, and the camera permission prompt.
- The microphone trigger. Its detector is tested; the microphone path is not.
- The hot keys and the menu-bar menu.
- "Remove the camera", which asks for an administrator password.
- The button that opens System Settings at the approval pane.
- The new slate with its two lines of text, which only a reading app can show.

## Checks for you

1. Open God's Camera. On the **Camera** screen, choose **Camera** under the picture. Confirm that you see yourself. (Done in your first test.)
2. Quit Zoom and open it again. Choose **God's Camera** as the camera. Confirm the live picture.
3. On the **Loop** screen, click **Record a loop**, wait five seconds, and click **Stop recording**. Click **Play the loop**. Confirm that Zoom shows the loop without a flash where it starts again, and that **Back to live** returns to the live picture.
4. Try ⌃⌥⌘L and ⌃⌥⌘R while Zoom is in front.
5. Close the God's Camera window. Confirm that the menu-bar icon stays and that Zoom still shows your picture.
6. Repeat step 2 in Google Meet, after quitting the browser and opening it again.
7. Quit God's Camera while Zoom is open. Confirm that Zoom shows the slate with the words "No picture from the app yet".
8. For the soak: leave God's Camera and a Zoom meeting running for 60 minutes, then use **Save a diagnostics report…** and send me the file.

After step 1, I can also run the full self-test, which reads the camera back the way a meeting app does and measures the delay.
