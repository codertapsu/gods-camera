# Updating the Galaxy S22+ to Android 16

Date: 2026-11-11 · A side task to the [implementation plan](implementation-plan.md)

There are three routes. Try them in order: the first two are Samsung's own and carry almost no risk, and the third is a manual install that I expect you will need. Back up the phone before the third.

Each step has its own label (A1, B2, C5 and so on), so you can tell me exactly where you are.

## Your phone today

| Fact | Value |
|---|---|
| Model | SM-S906U, the US carrier variant |
| Software | Android 13, One UI 5.1, build `S906USQU2CWE6`, security patch of June 2023 |
| Sales code | `GCF` |
| Knox | Not tripped |
| Free storage | 17 GB |

Why it stopped updating, as far as I can tell: a US carrier model looks to its original carrier for updates. With a non-US SIM it falls back to the generic `GCF` code, and phones in that state stop receiving over-the-air updates. This is the consistent explanation in user forums; Samsung does not document it.

The newest official software I found for this model is build `S906USQSAGZH3`: Android 16, built in August 2026. The same build is published for every US carrier.

## What to expect

- **Android 16 is the last major version** Samsung released for the S22 series.
- **You cannot go back.** Samsung blocks downgrades after an update.
- **The `GCF` code probably stays.** Later security patches would then need the same manual install again.
- **Time:** routes A and B take a few minutes each to try. Route C takes one to two hours, most of it downloading.

## Route A: over the air

- **A1.** Connect the phone to Wi-Fi and charge it above 20 %.
- **A2.** Open **Settings**, then **Software update**, then **Download and install**.
- **A3.** If an update is offered, install it and repeat A2 until none is left. Going from Android 13 to 16 this way takes several rounds.
- **A4.** If it says your software is up to date, go to route B.

## Route B: Smart Switch on the Windows PC

Smart Switch is Samsung's own computer program. It can install updates through the computer's internet connection. Reports on US carrier phones are mixed: some users say it works where the phone's own update check does not, and others still see "latest version installed".

- **B1.** Install Smart Switch for Windows from samsung.com.
- **B2.** Connect the phone with its USB cable. On the phone, allow the connection and choose file transfer.
- **B3.** Open Smart Switch and let it detect the phone.
- **B4.** Click **Backup** and let it finish. You want this backup whichever route succeeds.
- **B5.** If an **Update** button appears, click it and follow the prompts. Do not unplug or use the phone until it finishes. Repeat until no update is offered.
- **B6.** If no **Update** button appears, go to route C.

## Route C: manual install with Odin

Odin is the Windows program Samsung's service centres use to write software onto a phone. Samsung does not publish it, so it comes from third-party sites. The software package itself is Samsung's and is signed; a US model like yours has a locked bootloader that refuses anything Samsung did not sign. The realistic risks are a bad download of the Windows tool, and losing your data if the install does not keep it.

Your phone's One UI 5.1 predates Auto Blocker, the Samsung setting that on newer versions can block installs over USB, so there is nothing to switch off first.

### Before you start

- **C1.** Make sure the backup from B4 exists. Also check that your photos are in Google Photos or copied to a computer.
- **C2.** Know the password of the Google account on the phone. After a factory reset the phone asks for it before it can be used.
- **C3.** Charge the phone above 60 %.
- **C4.** Close Smart Switch completely. It interferes with Odin. Its installer has already put Samsung's USB driver on the PC, which Odin needs.

### Get the files

- **C5.** Download the software package for model **SM-S906U**. SamMobile lists it at https://www.sammobile.com/samsung/galaxy-s22-plus/firmware/SM-S906U/TMB/. Pick the newest entry: `S906USQSAGZH3` or later, marked Android 16. Any US carrier's list gives the same package. Do not take a package for SM-S906U1, SM-S906B or SM-S906E; those are different variants.
- **C6.** Unzip it. You get five files whose names begin with `BL`, `AP`, `CP`, `CSC` and `HOME_CSC`.
- **C7.** Download Odin. The SamMobile page for each package links a copy. Scan the download with your antivirus, then unzip it.

### Put the phone in Download mode

- **C8.** Switch the phone off completely.
- **C9.** Hold **Volume Up** and **Volume Down** together. While holding them, plug the USB cable from the PC into the phone.
- **C10.** A warning screen appears. Release the buttons and press **Volume Up** once to continue. The screen then says it is downloading and not to turn off the target.

### Install

- **C11.** Start Odin on the PC. One of the boxes near the top should show a port name such as `COM3`, which means it sees the phone. If no box lights up, try another USB port or cable.
- **C12.** Click **BL** and choose the file that begins with `BL`.
- **C13.** Click **AP** and choose the file that begins with `AP`. This file is large; Odin may look frozen for a minute while it checks it.
- **C14.** Click **CP** and choose the file that begins with `CP`.
- **C15.** Click **CSC** and choose the file that begins with `HOME_CSC`. This keeps your apps and data. The plain `CSC` file erases the phone instead.
- **C16.** Leave **USERDATA** empty.
- **C17.** On the **Options** tab, **Auto Reboot** and **F. Reset Time** should be ticked and **Re-Partition** unticked. Change nothing else.
- **C18.** Click **Start**. Leave the cable and the phone alone. It takes several minutes.
- **C19.** Odin shows **PASS!** and the phone restarts by itself. The first start can take ten minutes or more.
- **C20.** Check the result in **Settings**, **About phone**, **Software information**. It should say Android 16.

### If something goes wrong

| What you see | What to do |
|---|---|
| Odin shows **FAIL** straight away | Nothing was written. Check that Smart Switch is closed and that the files are for SM-S906U. Hold **Volume Down** and the **Side** key for more than seven seconds to restart the phone, then start again at C8 |
| Odin fails part-way, and the phone shows a software update error | The phone is not lost. Enter Download mode again (C8 to C10) and repeat the install |
| The phone restarts over and over after **PASS!** | The kept data did not survive the jump from Android 13. Erase the phone: switch it off, keep the USB cable connected to the PC, hold **Volume Up** and the **Side** key until the recovery menu appears, and choose **Wipe data/factory reset**. Then restore the backup from B4 |
| After an erase, the phone asks for a Google account | Sign in with the account that was on the phone before (C2) |

## Afterwards

- Turn USB debugging back on if it was reset, and connect the phone to the Mac.
- Tell me, and I will check it in again. It then meets the Android 16 minimum and joins testing next to the tablet.

## What I could not confirm

- **Whether route B offers the update on this phone.** Reports differ.
- **Whether your data survives route C.** Installing with `HOME_CSC` is meant to keep data, but your phone jumps three Android versions at once and I found no source that covers that case. That is why the backup comes first.
- **The newest build on the day you download.** `S906USQSAGZH3` was the newest I found; take whatever is newest then.
- **Whether official software leaves Knox untouched.** To my knowledge it does, since Knox trips on unofficial software. The pages I read do not say so.
- **The recovery key combination in the table above.** It is what I know of recent Galaxy phones, not something the pages I read state for the S22+.
- **Whether a Samsung service centre in Vietnam would do this for a US model.** If you would rather not do route C yourself, it is worth asking one.
- **Details from two sources.** A firmware site and a developer forum blocked my reader, so what they say reached me only through search summaries.

One alternative I do not recommend: installing the software of the unlocked variant (SM-S906U1) would bring automatic updates back, but it needs a modified copy of Odin and erases the phone. With the S22's major updates finished, that is not worth it.

## Sources

- [How to update your Galaxy phone or tablet (Samsung US)](https://www.samsung.com/us/support/answer/ANS00077582/)
- [How to update your Samsung Galaxy phone or tablet (Samsung)](https://www.samsung.com/latin_en/support/mobile-devices/how-to-update-your-samsung-galaxy-phone-or-tablet/)
- [Galaxy S22+ SM-S906U firmware list (SamMobile)](https://www.sammobile.com/samsung/galaxy-s22-plus/firmware/SM-S906U/TMB/)
- [Galaxy S22+ SM-S906U build S906USQS9GZB4, with install notes (SamMobile)](https://www.sammobile.com/samsung/galaxy-s22-plus/firmware/SM-S906U/TMB/download/S906USQS9GZB4/1988477/)
- [Flashing firmware guide (Samsung Discord community)](https://samsungdiscord.vercel.app/guides/firmware/flashing)
- [Install Samsung stock firmware using Odin (The Custom Droid)](https://thecustomdroid.com/install-samsung-stock-firmware-galaxy-devices-using-odin)
- [Unable to update Android on unlocked phone (Samsung Community)](https://us.community.samsung.com/t5/Galaxy-S22/Unable-to-update-Android-on-unlocked-phone/td-p/3440503/page/3)
- [S21 Ultra GCF/GCF/VZW OTA updates error (XDA Forums)](https://xdaforums.com/t/s21-ultra-gcf-gcf-vzw-ota-updates-error.4392607/)
- [OTA update issue (XDA Forums)](https://xdaforums.com/t/ota-update-issue.4555237/)
- [Galaxy S22+ SM-S906U firmware (Samfw)](https://samfw.com/firmware/SM-S906U)
- [Galaxy S22 gets One UI 8 on Verizon (Android Authority)](https://www.androidauthority.com/samsung-galaxy-s22-one-ui-8-us-verizon-3611797)
- [Protect your Galaxy device with the Auto Blocker feature (Samsung UK)](https://www.samsung.com/uk/support/mobile-devices/protect-your-galaxy-device-with-the-new-auto-blocker-feature/)
