# Phase 3 results so far

Date: 2026-11-11 · Plan: [implementation-plan.md](implementation-plan.md)

Phase 3 is under way, not finished. The scanner, the second idea on your list, works from end to end: pictures go from the Android app to the Mac, the Mac reads the text and merges overlapping shots into one document. Phone as camera, the remote control, the long-document camera mode and Wi-Fi pairing are not built yet.

**Windows work cannot start yet.** Its start point is the exit of this phase.

## What exists now

| Part | State |
|---|---|
| Hub | Runs inside the Mac app and from `gc hub`. Uploads resume. Document jobs run in the background with progress, are kept on disk, and serve their result files |
| Text reader | `gc-ocr`, a small tool that reads text with Apple's Vision at the accurate level, in Vietnamese and English. It is bundled inside the Mac app |
| Merging | `gc-doc` in the core: places overlapping photos in one frame, joins cut lines, removes repeats, finds columns and reading order |
| Output | Markdown, plain text, and JSON with every line's position |
| Mac app | **Scanner** section: a switch for the hub, the token for the device, and the documents received, each with open, copy, show in Finder and delete |
| Android app | Connects to the hub, takes pages from the document scanner, the photo picker or another app's share sheet, sends them, follows the job and shows the text with copy and share. English and Vietnamese |
| Tools | `gc scan <folder>`, `gc merge-ocr`, `tools/connect-android.sh`, `tools/modelkit/make_document.py` |

## How overlapping photos become one document

No picture processing is involved; the text itself does the work.

1. **Placing the photos.** A line that two photos both show is a landmark. A few shared lines fix how one photo sits against another. Where two photos share only a piece of a line, the place where their words overlap is used instead, so it also works when every photo is narrower than the text it shows.
2. **Keeping each line once.** The readings of one line are gathered from all photos. A whole reading settles the line; several whole readings are compared word by word, so one photo's misread letter is outvoted. Where no photo shows the line whole, it is built from its pieces, left to right, joined where their words overlap.
3. **Reading order.** The page is cut along its empty strips: first into bands across, then into columns, and so on. A headline comes before the columns under it, and columns are read left to right.
4. **Paragraphs.** A wider gap, an indent, or a short line that ends a sentence starts a new paragraph. A word split by a hyphen at a line's end is put back together.

## Measurements

Test pages are made by `make_document.py`: a newspaper-like page with known text, cut into overlapping shots that are each slightly turned, skewed and unevenly lit. The Vietnamese page has 398 words and the English page 248.

| Page | Shots | Words wrong | Characters right |
|---|---|---|---|
| Vietnamese, three columns | 6, in a 3 × 2 grid | 1 | 99.94 % |
| Vietnamese, two columns | 8, in a 4 × 2 grid | 1 | 99.94 % |
| Vietnamese, three columns | 9, in a 3 × 3 grid | 5 | 99.72 % |
| Vietnamese, one wide column | 5, in a strip | 5 | 99.72 % |
| English, two columns | 4, in a 2 × 2 grid | 0 | 100 % |
| English, three columns | 6, in a 3 × 2 grid | 0 | 100 % |
| English, two columns, every shot narrower than a column | 9, in a 3 × 3 grid | 0 | 100 % |

The target is 98 %. Every page came out as one document with the right reading order.

- **What is still wrong.** Every remaining slip is one Vietnamese letter that Vision misread: a tone mark lost or swapped, or `đ` read as `d`. A language model can fix these; that is part of Phase 5.
- **What the first version got wrong.** A wider test found two failures: a stray photo on a 3 × 3 grid (97.7 %), and 39 % when no photo showed any line whole. Both are fixed, and both layouts are in the table above.
- **Speed.** About 1 second for six photos once Vision is warm, and about 29 seconds the first time in a newly installed app, while Vision loads.

The six-shot Vietnamese document was also sent along each path. The merger was improved part-way through, and not every path has been run again since:

| Path | Merger | Result |
|---|---|---|
| `gc merge-ocr`, without the hub | Current | 1 word wrong |
| The Android app on the Galaxy Tab S9+, over Wi-Fi debugging, through the hub | Current | 1 word wrong, in 2 seconds; the app showed the text in Vietnamese |
| `gc scan` through the hub | Earlier | 3 words wrong |
| The Android app in the emulator | Earlier | 3 words wrong |
| The installed Mac app's own hub and bundled reader | Earlier | 3 words wrong. Not run again, because you were using the app |

Tests: 72 in the core and 36 in the Swift package. The core's check also builds for Android and Windows.

These pages are rendered, not photographed. Real newspaper photos will be harder: curved paper, shadows, smaller print. The measure on real photos is still to come.

## The tablet

On 2026-11-11 you switched the tablet to Wi-Fi debugging and unlocked it, so I could run the app on it.

- The port forward the hub needs works over Wi-Fi debugging as it does over the cable.
- The app connected, sent six test pages and showed the merged text. The pages came from a folder in the app's own storage, through a debug-only switch, so your photo library was not opened.
- **Scan a document** opens Google's document scanner. I did not take a picture with it or capture that screen, since it shows the camera's view.

Still unchecked: a real page photographed with the scanner, the photo picker with your own pictures, and a share from another app.

## Open in this phase

- The review screen on the Mac, with the picture beside the text.
- DOCX and searchable PDF.
- Long-document camera mode, batch capture, and a text preview on the device.
- Uploads that carry on in the background.
- Phone as camera, and the remote control.
- Events over a WebSocket instead of polling, the OpenAPI description, and a cap on picture dimensions.
- Wi-Fi pairing: all of 3c.

## Checks for you

1. On the Mac, open God's Camera. In **Scanner**, turn on **Receive scans from my phone or tablet**.
2. With the tablet unlocked and connected for debugging, by cable or Wi-Fi, run `tools/connect-android.sh` in a terminal on the Mac. It sets up the connection, opens the app on the tablet and gives it the hub's token. macOS asks once for permission to read the token from the Keychain; click **Always Allow**.
3. On the tablet, the app should say **Connected to the Mac** (**Đã kết nối với máy Mac**).
4. Tap **Scan a document** and photograph a printed page, or tap **Add pictures** and choose several overlapping photos of a newspaper page.
5. Tap **Send to the Mac**. The text should appear on the tablet, and the document in the Mac app's **Scanner** list.
6. Tell me how the text compares with the page, especially the reading order and the Vietnamese tone marks.

The token hand-over in step 2 works in debug builds only. Pairing by QR code, in part 3c, replaces it.
