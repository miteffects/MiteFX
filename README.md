# MiteFX

Free, open-source audio plugins: 18 of them, VST3 and CLAP, for Windows, macOS and Linux.

**Website and downloads:** https://OWNER.github.io/MiteFX/ · **Releases:** [latest](https://github.com/OWNER/MiteFX/releases/latest)

| Mixing | Dynamics | Space and time | Tools |
|---|---|---|---|
| Mite EQ | Mite Comp | Mite Verb | Mite Pitch |
| Mite Sat | Mite Limit | Mite Delay | Mite Tuner |
| Mite Shape | Mite Multiband | Mite Mod | Mite Analyzer |
| Mite Double | Mite Gate | | Mite Rig (guitar and bass) |
| | Mite DeEss | | Mite Gain |
| | Mite DeNoise | | |

This is an early release (0.1).

## Build

Rust (stable). Every plugin is a crate under `plugins/`; package names use underscores.

```bash
cargo test --workspace                       # every test
cargo xtask bundle mite_sat --release        # CLAP + VST3 into target/bundled/
cargo xtask install mite_sat                 # release bundle, copied into ~/.clap and ~/.vst3
cargo run -p mite_sat --example preview      # the editor in a standalone window
```

On Linux the editors need the X11/XCB development packages (see `.github/workflows/ci.yml`).

How the project is put together: [`BLUEPRINT.md`](BLUEPRINT.md); measurements against commercial
plugins: [`docs/compare/`](docs/compare/).

## Releases

Pushing a tag `v*` builds every plugin on Windows, macOS (universal) and Linux and publishes a
GitHub Release (`.github/workflows/release.yml`). The website in `site/` is deployed with GitHub
Pages (`.github/workflows/pages.yml`).

## License

GPL-3.0-or-later (see [`LICENSE`](LICENSE)). The Manrope font is under the SIL Open Font License.
VST is a trademark of Steinberg Media Technologies GmbH.
