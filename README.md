# God's Camera

Personal-use virtual camera, face swap, scanner and capture tools for macOS, Windows and Android.

- Plan: [docs/implementation-plan.md](docs/implementation-plan.md)
- Research: [docs/research-findings.md](docs/research-findings.md)
- Phase 0 results: [docs/phase-0-results.md](docs/phase-0-results.md)

## Layout

| Path | What it holds |
|---|---|
| `core/` | Shared Rust core: alignment, tracking, detector decoding, pack manifests, the hub, the `gc` command-line tool |
| `apps/apple/` | macOS app and Camera Extension (generate the Xcode project with `xcodegen generate`) |
| `tools/modelkit/` | Python reference runs and model benchmarks (`uv run python ...`) |
| `packs/` | Model manifests with pinned SHA-256 hashes. Model files live outside the repository, in `../packs` |

## Common commands

```sh
tools/check.sh                                   # tests, lint, Android and Windows cross-builds
cd core && cargo run -p gc-cli -- verify-packs --manifests ../packs --store ../../packs
GC_HUB_TOKEN=<16+ characters> cargo run -p gc-cli -- hub
cd apps/apple && xcodegen generate && xcodebuild -scheme GodsCamera -derivedDataPath build -allowProvisioningUpdates build
```

The models are licensed for non-commercial research only. See the manifests in `packs/`.
