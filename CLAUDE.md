# MiteFX: open-source pro audio effects suite (Rust)

**Read `BLUEPRINT.md` first.** It is the source of truth for vision, stack, crate layout,
plugin roster, UX system and roadmap. When a decision changes, update `BLUEPRINT.md` in
the same change.

Stack: Rust workspace · nih-plug (CLAP/VST3, isolated in `crates/mite-plugin`) ·
clap-wrapper (AU) · baseview + wgpu + Vello UI · GPL-3.0-or-later.

## Build

Package names use underscores (`mite_gain`); bundle names come from `bundler.toml`.

```bash
cargo test --workspace                          # DSP, core, UI and plugin tests
cargo clippy --workspace --all-targets
cargo xtask bundle mite_gain --release          # CLAP + VST3 into target/bundled/
cargo xtask bundle mite_gain                    # debug build: allocations in process() abort
cargo xtask install mite_gain                   # copy bundles to ~/.clap, ~/.vst3 (or OS equivalent)
cargo xtask validate mite_gain                  # clap-validator + pluginval, if on PATH
cargo run -p mite_gain --example preview        # editor in a standalone window, fake meter
cargo run -p mite_gain --example snapshot -- out.png 2   # editor → PNG (GPU, no window)
```

## Layout

- `crates/mite-dsp`: DSP primitives (smoother, meter, dB).
- `crates/mite-core`: `ParamSpec`, telemetry atomics, golden manifest.
- `crates/mite-ui`: tokens (from `docs/design/tokens.json`), text (embedded Manrope), widgets, window, offscreen render.
- `crates/mite-plugin`: the only nih-plug glue.
- `plugins/<name>`: the plugin crates.
- `tests/golden/`: frozen parameter manifests.

## Hard rules (full list: BLUEPRINT.md §5)

- The audio thread never allocates, locks, does I/O, logs or panics. `assert_no_alloc` guards `process()` and `reset()` in debug and CI builds.
- Buffers are sized only in `prepare()`. Expensive work (FIR design, IR or model load) runs on a worker thread and arrives via `handover` with a crossfade.
- `mite-dsp` and `mite-core` never import nih-plug, baseview or wgpu. Only `mite-plugin` touches the framework.
- Parameter IDs are frozen once released. State is versioned and migrated, with golden tests for every version.
- Reported latency equals measured group delay (tested). Latency changes are signalled to the host.
- Every continuous parameter is click-free (ramp test). Bypass, mix 0 % and neutral settings null.
- The UI reads parameters and telemetry and writes parameters only via begin/set/end gestures. It never owns DSP state.

## Reference code

Nova's C++ engine (`~/Nova/engine/src/plugin/native/`) is the algorithm reference: port
ideas, not code. See BLUEPRINT.md §4 for what to port and what to replace.
