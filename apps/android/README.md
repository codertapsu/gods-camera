# God's Camera for Android

The scanner on a phone or tablet: photograph pages, send them to the Mac, get the text back.
It needs Android 16 and a 64-bit ARM device.

## Build

```sh
tools/build-core-android.sh        # once, and again after core/gc-mobile or core/gc-pair changes
cd apps/android
JAVA_HOME=/Library/Java/JavaVirtualMachines/temurin-17.jdk/Contents/Home \
  ./gradlew assembleDebug testDebugUnitTest lintDebug
```

- The first command builds the shared core for Android and generates its Kotlin interface. Both are build products and are not in git; Gradle stops with a pointer to the script while they are missing.
- Lint passes without warnings, and the build fails on a new one.
- Release builds are not shrunk. If shrinking is turned on, the generated interface and JNA need keep rules, because they are reached by name.
- One unit test pairs with a real hub: the `gc` tool from `core/target/release`. Build it with `cargo build --release -p gc-cli` in `core/`. Without it that test is skipped.

## How the app connects to the Mac

1. On the Mac, **Scanner › Connect a phone or tablet…** shows a six-digit code.
2. In the app, **Connect to a Mac** lists the Macs that announce themselves on the Wi-Fi network (`_godscamera._tcp`). An address can be typed instead.
3. The code is entered. The exchange proves to each side that the other knows the code, and ties the Mac's certificate to it.

Afterwards the app accepts only that certificate and presents its own token with every request. It finds the Mac again by its announcement, so a new address on the network does not matter.

What is stored, in the app's private preferences (`pairing`): the Mac's name, the fingerprint of its certificate, its last address, this device's id, and the token. The token is encrypted with a key that lives in the Android Keystore. Nothing is included in backups or device transfers.

## Driving a debug build from a computer

A debug build takes these extras on its launch intent. A release build ignores all of them.

| Extra | Effect |
|---|---|
| `--es pair_address <host:port> --es pair_code <six digits>` | Connect with a code, as if the Mac had been chosen and the code typed |
| `--ez forget_mac true` | Forget the connected Mac |
| `--es debug_pages <folder>` | Take the pictures in `files/<folder>` of the app's own storage as pages |
| `--ez debug_send true` | Send the pages as soon as a Mac is connected |
| `--es hub_token <token>` | Use the hub through `adb reverse` on `127.0.0.1:47800`, for this run of the app |
| `--es debug_screen <name>` | Show a screen for pictures: `home`, `connect`, `connect-sample`, `address`, `code`, `wrong-code`, `connected`, `settings` |

Example, with the hub's code in `$CODE`:

```sh
adb shell am start -n dev.godscamera.android/.MainActivity \
  --es pair_address 192.168.1.20:47801 --es pair_code "$CODE" \
  --es debug_pages e2e --ez debug_send true
```

A Mac connected with a code comes before the debugging connection. The debugging connection is used when no Mac is connected, or after `hub_token` arrived, until the app is closed.
