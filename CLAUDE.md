# MiteFX: open-source pro audio effects suite (Rust)

**Read `BLUEPRINT.md` first.** It is the source of truth for vision, stack, crate layout,
plugin roster, UX system and roadmap. When a decision changes, update `BLUEPRINT.md` in
the same change.

Stack: Rust workspace · nih-plug (CLAP/VST3, isolated in `crates/mite-plugin`) ·
clap-wrapper (AU) · baseview + wgpu + Vello UI · GPL-3.0-or-later.

## Build (from Phase 0 onward)

```bash
cargo test --workspace                 # DSP + core tests
cargo xtask bundle <plugin> --release  # CLAP / VST3 / AU bundles into target/bundled
cargo xtask install <plugin>           # copy to user plugin folders
cargo xtask validate <plugin>          # clap-validator + pluginval (+ auval on macOS)
```

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
