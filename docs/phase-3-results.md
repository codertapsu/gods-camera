# Phase 3 results so far

Date: 2026-11-11 · Plan: [implementation-plan.md](implementation-plan.md)

Phase 3 is under way, not finished. The scanner, the second idea on your list, works from end to end: pictures go from the Android app to the Mac, the Mac reads the text and merges overlapping shots into one document. Since version 0.4.0 the phone or tablet connects to the Mac over Wi-Fi with a six-digit code, and both apps have a new design. Phone as camera, the remote control and the long-document camera mode are not built yet.

**Windows work cannot start yet.** Its start point is the exit of this phase.

## What exists now

| Part | State |
|---|---|
| Hub | Runs inside the Mac app and from `gc hub`. Uploads resume. Document jobs run in the background with progress, are kept on disk, and serve their result files. It listens on the Mac itself for a device attached for debugging, and on the local network, over TLS, for devices connected with a code |
| Text reader | `gc-ocr`, a small tool that reads text with Apple's Vision at the accurate level, in Vietnamese and English. It is bundled inside the Mac app |
| Merging | `gc-doc` in the core: places overlapping photos in one frame, joins cut lines, removes repeats, finds columns and reading order |
| Output | Markdown, plain text, and JSON with every line's position |
| Mac app | **Scanner** screen: a switch for the hub, **Connect a phone or tablet…** with a six-digit code, the connected devices with **Remove**, and the documents received, each with read, copy, open, show in Finder and delete. Connecting over debugging is under **For development** |
| Android app | Finds the Mac on the Wi-Fi network and connects with the code. Takes pages from the document scanner, the photo picker or another app's share sheet, sends them, follows the job and shows the text with copy and share. Two panes on a tablet, one column on a phone. English and Vietnamese |
| Tools | `gc scan <folder>`, `gc merge-ocr`, `gc hub --lan --pair`, `gc pair`, `tools/build-core-android.sh`, `tools/connect-android.sh`, `tools/modelkit/make_document.py` |

## How overlapping photos become one document

No picture processing is involved; the text itself does the work.

1. **Placing the photos.** A line that two photos both show is a landmark. A few shared lines fix how one photo sits against another. Where two photos share only a piece of a line, the place where their words overlap is used instead, so it also works when every photo is narrower than the text it shows.
2. **Keeping each line once.** The readings of one line are gathered from all photos. A whole reading settles the line; several whole readings are compared word by word, so one photo's misread letter is outvoted. Where no photo shows the line whole, it is built from its pieces, left to right, joined where their words overlap.
3. **Reading order.** The page is cut along its empty strips: first into bands across, then into columns, and so on. A headline comes before the columns under it, and columns are read left to right.
4. **Paragraphs.** A wider gap, an indent, or a short line that ends a sentence starts a new paragraph. A word split by a hyphen at a line's end is put back together.

## Connecting with a six-digit code

On the Mac, **Scanner › Connect a phone or tablet…** shows a code. On the device, **Connect to a Mac** lists the Macs on the Wi-Fi network; choose yours and enter the code.

- **The code is never sent.** Device and Mac each prove to the other that they know it (SPAKE2). The proof also covers the Mac's TLS certificate, so a program in the middle that relays the messages with a certificate of its own fails, even with the right code.
- **A code is good for** five minutes, one device and five wrong tries. After that the Mac shows a new one.
- **Afterwards** the device accepts only that Mac's certificate and presents a token of its own with every request. The Mac keeps only a hash of the token. **Remove** on the Mac takes effect at once.
- **Where the secrets are.** The Mac's certificate and key are in the Keychain. On the device the token is encrypted with a key in the Android Keystore and left out of backups.
- **Finding the Mac.** The Mac announces itself by Bonjour as `_godscamera._tcp`; the app finds it again by that announcement, so a new address on the network does not matter. An address can also be typed.
- **Two doors, two keys.** The network side takes device tokens only, and the debugging side on the Mac itself takes the development token only.
- **Limits on the network side:** 64 connections, 10 seconds for a handshake, two minutes of silence.

What I checked:

| Check | Result |
|---|---|
| Core tests of the exchange and the hub over real TLS | 25 and 33 pass: right code, wrong code, changed message, a relay with another certificate, five wrong tries, expiry, a removed device, the wrong token on each side |
| Mac hub with the `gc` tool as the device | A wrong code is refused and uses up a try; the right one connects; the device survives a restart of the hub; removal works |
| Android app on the emulator, against a hub of its own | Connected in 1.8 s from a cold start; six pages came back as a document in 3.7 s |
| **Your tablet, over your Wi-Fi** | The app's list showed this Mac, announced by the installed Mac app. Against a temporary hub on another port it connected with the code, sent six pages and showed the text 3 seconds later. I removed that connection and the test pages afterwards |

Not checked: connecting your tablet to the Mac app itself with the code on the Mac's screen. That needs a person at both; it is the first check below.

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
- **Paragraphs cut in two, fixed.** The text that came back to your tablet in my test had two paragraphs split in the middle. The reader marks an ordinary line as a title when a photo happens to begin with it, and one such mark made the line a heading. Whether a line is a title is now decided by all the photos that show it, and a title must be larger than body text. The layouts above give the same accuracy and now the right number of paragraphs.
- **A new weak spot.** A three-column Vietnamese page in a 3 × 3 grid with 30 % overlap (seed 7 of the generator) does not come out as one document: two photos are not placed against the others. The text is still 98.1 % right, because repeated text is dropped, but the order near those photos can be wrong. Not fixed yet.
- **What the first version got wrong.** A wider test found two failures: a stray photo on a 3 × 3 grid (97.7 %), and 39 % when no photo showed any line whole. Both are fixed, and both layouts are in the table above.
- **Speed.** About 1 second for six photos once Vision is warm, and about 29 seconds the first time in a newly installed app, while Vision loads.

The six-shot Vietnamese document was also sent along each path. The merger was improved part-way through, and not every path has been run again since:

| Path | Merger | Result |
|---|---|---|
| `gc merge-ocr`, without the hub | Current | 1 word wrong |
| The Android app on the Galaxy Tab S9+, over Wi-Fi debugging, through the hub | Current | 1 word wrong, in 2 seconds; the app showed the text in Vietnamese |
| `gc scan` through the hub | Earlier | 3 words wrong |
| The Android app in the emulator | Earlier | 3 words wrong |
| The installed Mac app's own hub and bundled reader, nine shots in a 3 × 3 grid | Current | 5 words wrong, as in the table above, in 1.3 seconds |

Tests: 145 in the core, 47 in the Swift package and 19 in the Android app. The core's check also builds for Android and Windows.

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
- Photos that cannot be placed against the others: the 3 × 3 case above.
- Pairing by QR code, per-device permissions, and fuzzing of the hub's parsers before use on a network you do not control.
- A release build of the Android app, and a check with TalkBack.

## Checks for you

The tablet now has the new app, and it is not connected to any Mac.

1. On the Mac, open God's Camera and go to **Scanner**. **Receive scans on this Mac** should be on.
2. Click **Connect a phone or tablet…**. A six-digit code appears.
3. On the tablet, open God's Camera, tap **Connect to a Mac** (**Kết nối với máy Mac**), choose this Mac in the list, and enter the code. Both should say they are connected. If macOS asks whether God's Camera may find devices on your local network, choose **Allow**.
4. Tap **Scan a document** and photograph a printed page, or tap **Choose photos** and pick several overlapping photos of a newspaper page.
5. Tap **Send to the Mac**. The text should appear on the tablet, and the document in the Mac app's **Scanner** list, where **Read** shows it.
6. Close the app on the tablet and open it again: it should find the Mac by itself. Then click **Remove** beside the tablet on the Mac: the tablet should say the Mac no longer accepts it.
7. Tell me how the text compares with the page, especially the reading order and the Vietnamese tone marks, and how the two apps feel to use.

For development, a device attached for debugging can still be connected without a code: **Scanner › For development › Connect over debugging** on the Mac, or `tools/connect-android.sh` for a hub started with `gc hub`. The debug build of the Android app can also be driven from a terminal; `apps/android/README.md` lists how.
