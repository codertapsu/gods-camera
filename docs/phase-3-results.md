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

1. **Placing the photos.** A line that two photos both show is a landmark. A few shared lines fix how one photo sits against another. Where two photos share only a cut-off piece of a line, the end both can see is used.
2. **Keeping each line once.** A whole reading of a line replaces a cut one. Where every photo cuts a line, the pieces are joined where their words overlap.
3. **Reading order.** The page is cut along its empty strips: first into bands across, then into columns, and so on. A headline comes before the columns under it, and columns are read left to right.
4. **Paragraphs.** A wider gap, an indent, or a short line that ends a sentence starts a new paragraph. A word split by a hyphen at a line's end is put back together.

## Measurements

Test pages are made by `make_document.py`: a newspaper-like page with known text, cut into overlapping shots that are each slightly turned, skewed and unevenly lit.

| Test | Shots | Result |
|---|---|---|
| Vietnamese page, three columns | 6 | 99.8 % of characters right; 3 of 398 words differ. Target: 98 % |
| English page, two columns | 4 | 100 % |
| Through the hub, from the command line | 6 | The same document, in about 1 second once Vision is warm |
| Through the installed Mac app's own hub and bundled reader | 6 | The same document; 29 seconds the first time, while Vision loads |
| From the Android app in the emulator | 6 | The same document, shown in the app |

The three Vietnamese slips: one `đ` read as `d`, and two words at a cut where neither photo had the letters whole. A language model can fix slips like these; that is part of Phase 5.

Tests: 69 in the core and 36 in the Swift package, all passing. The core's check also builds for Android and Windows.

These pages are rendered, not photographed. Real newspaper photos will be harder: curved paper, shadows, smaller print. The measure on real photos is still to come.

## What I could not check

- **The tablet.** It is locked with a screen lock, so I could not open the app on it. I checked the app in an Android 16 emulator instead. The document scanner needs Google Play services and a camera, so it has not run at all yet.
- **The share sheet** into the Android app.

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
2. Connect the tablet by cable and unlock it. In a terminal on the Mac, run `tools/connect-android.sh`. It sets up the connection, opens the app on the tablet and gives it the hub's token. macOS asks once for permission to read the token from the Keychain; click **Always Allow**.
3. On the tablet, the app should say **Connected to the Mac**. The app is already installed.
4. Tap **Scan a document** and photograph a printed page, or tap **Add pictures** and choose several overlapping photos of a newspaper page.
5. Tap **Send to the Mac**. The text should appear on the tablet, and the document in the Mac app's **Scanner** list.
6. Tell me how the text compares with the page, especially the reading order and the Vietnamese tone marks.

The token hand-over in step 2 works in debug builds only. Pairing by QR code, in part 3c, replaces it.
