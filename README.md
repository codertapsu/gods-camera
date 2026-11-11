# God's Camera

Personal-use virtual camera, face swap, scanner and capture tools for macOS, Windows and Android.

- Plan: [docs/implementation-plan.md](docs/implementation-plan.md)
- Research: [docs/research-findings.md](docs/research-findings.md)
- Phase 0 results: [docs/phase-0-results.md](docs/phase-0-results.md)
- Phase 1 results: [docs/phase-1-results.md](docs/phase-1-results.md)

## Layout

| Path | What it holds |
|---|---|
| `core/` | Shared Rust core: alignment, tracking, model inference, the live face-swap engine, pack manifests, the hub, the `gc` command-line tool |
| `apps/apple/` | macOS app and Camera Extension (generate the Xcode project with `xcodegen generate`) |
| `tools/modelkit/` | Python reference runs and model benchmarks (`uv run python ...`) |
| `packs/` | Model manifests with pinned SHA-256 hashes. Model files live outside the repository, in `../packs` |

## Common commands

```sh
tools/check.sh                                   # tests, lint, Android and Windows cross-builds
cd core && cargo run -p gc-cli -- verify-packs --manifests ../packs --store ../../packs
GC_HUB_TOKEN=<16+ characters> cargo run -p gc-cli -- hub
tools/build-core-apple.sh                        # the core for the Mac app: static library and Swift bindings
cd apps/apple && xcodegen generate && xcodebuild -scheme GodsCamera -derivedDataPath build -allowProvisioningUpdates build
cd apps/apple/GodsCameraKit && swift test       # the Mac engine's tests
open -a /Applications/GodsCamera.app --args --selftest /tmp/gc-selftest.json   # the app checks itself and writes a report
```

Face swap from the command line, on a still and on a moving-face clip made from a photo:

```sh
cd core
cargo run --release -p gc-cli -- swap --packs ../../packs --source face.jpg --target photo.jpg --out out.png
cargo run --release -p gc-cli -- replay --packs ../../packs --photo gc-infer/tests/golden/input.png
```

The Mac app carries its base models inside the bundle; the build copies them from `../packs` and checks each against its manifest. Changing anything under `core/` means running `tools/build-core-apple.sh` again before building the app.

Changing the Camera Extension means the extension's `CURRENT_PROJECT_VERSION` in `apps/apple/project.yml`; macOS replaces an installed extension only when its version or build number differs.

The models are licensed for non-commercial research only. See the manifests in `packs/`.
