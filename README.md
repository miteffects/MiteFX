# MiteFX

Free, open-source audio plugins: 19 of them, VST3 and CLAP, for Windows, macOS and Linux.

**Website and downloads:** https://miteffects.github.io/MiteFX/ · **Releases:** [latest](https://github.com/miteffects/MiteFX/releases/latest)

| Mixing | Dynamics | Space and time | Tools |
|---|---|---|---|
| Mite EQ | Mite Comp | Mite Verb | Mite Pitch |
| Mite Sat | Mite Limit | Mite Delay | Mite Tuner |
| Mite Shape | Mite Multiband | Mite Mod | Mite Analyzer |
| Mite Double | Mite Gate | | Mite Rig (guitar and bass) |
| Mite Tape | Mite DeEss | | Mite Gain |
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

Every push to `main` builds every plugin on Windows, macOS (universal) and Linux, packs installers
(Windows Setup.exe, macOS .pkg, Linux install.sh and .deb; `packaging/`) and replaces the downloads
of the current version's GitHub Release (`v` + the workspace version in `Cargo.toml`);
a newer push cancels a build still running (`.github/workflows/release.yml`). Bump the version
for a new release. The website in `site/` is deployed with GitHub
Pages (`.github/workflows/pages.yml`).

## License

GPL-3.0-or-later (see [`LICENSE`](LICENSE)). The Manrope font is under the SIL Open Font License.
VST is a trademark of Steinberg Media Technologies GmbH.
