# MiteFX — Blueprint

> The backbone document for MiteFX. Every design and implementation decision either
> follows this file or changes this file first. Audience: Claude Design (visual
> system), implementation sessions, and future contributors.

**Status:** Phase 0 (not started) · design first pass in · **License:** GPL-3.0-or-later · **Last revised:** 2026-10-22

---

## 1. Vision

MiteFX is a free, open-source suite of studio effects that aims to be **better than FabFilter**:
better sound, more features, and a better-looking, faster-to-use interface. It should
never become something people pirate, because it costs nothing and nobody can lock it up.

### Principles

1. **Sound first, measured.** Every DSP claim (aliasing, THD, phase, latency) has a test that measures it. "It sounds good" is not a test.
2. **Real-time is sacred.** No allocation, locks, I/O, or unbounded work on the audio thread, ever. This is enforced by tooling, not by reviewers remembering.
3. **Null-test honesty.** Bypass, mix at 0 %, and neutral settings must null to the dry signal (aside from reported latency). Every dynamics or colour plugin has **Delta** listen.
4. **Every pixel is information.** Visuals show what the DSP is actually doing (real curves, real gain reduction, real spectra) at 60–120 fps. There is no decoration that lies.
5. **One gesture, one result.** Direct manipulation over panels of knobs. Everything is reachable with the mouse, the keyboard, and a screen reader.
6. **One system, many plugins.** All plugins share the same widgets, tokens, interactions, preset and undo model. Learn one, know all.
7. **Professional in every host.** CLAP, VST3, AU, sample-accurate automation, correct latency reporting, state that loads in 20 years.

---

## 2. Tech stack

| Layer | Choice | Why |
|---|---|---|
| Language | **Rust (stable, edition 2024)** | Memory safety, and the compiler plus `assert_no_alloc` catch whole classes of real-time bugs that hurt Nova. Modern tooling and a straightforward cross-compile story. |
| Plugin formats | **nih-plug** (pinned git rev) → **CLAP + VST3** | Mature Rust plugin framework, ISC licence, sample-accurate params, sidechain and bus layouts. VST3 SDK is MIT since 3.8, so licensing is clean under GPL. |
| AU (macOS) | **clap-wrapper** (CLAP → AUv2) | Official CLAP project. Also the path to AUv3 and standalone later. |
| Framework isolation | Our own **`mite-plugin`** adapter crate | DSP and UI crates **never import nih-plug**. If nih-plug stalls, we swap the adapter (for example to Coupler or raw `clap-sys` + `vst3`) without touching DSP. |
| Windowing | **baseview** | Parented plugin windows on Win32, Cocoa and X11. |
| GPU | **wgpu** (Vulkan / Metal / DX12, GL fallback) | One renderer on every platform. |
| 2D renderer | **Vello** | GPU compute vector renderer: analytic anti-aliasing, gradients, clipping, huge path counts (spectra, curves) at high refresh rates. |
| Effects | Custom **WGSL** passes | Bloom and glow, frosted blur, spectrum heat fills, noise and grain. This is how we beat FabFilter visually. |
| Fallback renderer | `vello_hybrid` / `vello_cpu` | For VMs, old iGPUs and remote desktops. Same scene code, so the UI is identical. |
| Text | `parley` + `skrifa` (Linebender stack) | Proper shaping and hinting; we ship our own fonts. |
| Accessibility | **AccessKit** | Screen readers on all three OSes. |
| FFT | `realfft` / `rustfft` | One wrapper in `mite-dsp`; no hand-rolled FFTs. |
| SIMD | `std::simd` once stable, `wide` until then | Biquad and SVF banks, convolution. |
| RT safety | `assert_no_alloc` (debug and CI), `rtrb` / `triple_buffer` for lock-free hand-off | Allocation on the audio thread **panics in debug builds**. |
| Build | `cargo xtask` | Bundle, sign, notarize, universal2 macOS, install to user plugin folders. |
| Validation | `clap-validator`, `pluginval` (VST3/AU), `auval` | Runs in CI on every pull request. |
| Testing | `cargo test`, golden files, `criterion` benchmarks, `proptest` for parameter fuzzing | |

**Platforms:** Windows 10+ (x64, arm64), macOS 11+ (universal2), Linux (x64, arm64; X11 and XWayland).

**Formats now:** CLAP, VST3, AUv2.

**Formats with an architectural slot but not built yet:**
- **AAX:** through clap-wrapper's AAX work or a dedicated adapter. It needs the Avid SDK and PACE signing.
- **AUv3 for iOS/iPadOS:** the UI must stay touch-capable; see §7.
- **Standalone:** via the clap-wrapper standalone target.
- **LV2:** optional, for Linux distros.
- **Native Nova host integration:** load the CLAP builds directly.

Nothing in `mite-dsp`, `mite-core` or `mite-ui` may assume a particular format or host.

### Why not C++/JUCE, and why not a WebView UI

- **JUCE:** its GPL path is fine, but we would inherit Nova's class of real-time bugs and gain nothing visually. JUCE's renderer is the bottleneck for a FabFilter-beating look.
- **WebView:** the design hand-off would be 1:1, but WebKitGTK embedding in Linux hosts is fragile, it costs a browser runtime per instance, and feeding 120 fps meters across IPC is wasteful. We keep HTML for **design mockups only** (see §7.4).

---

## 3. Repository layout

```
MiteFX/
├── BLUEPRINT.md            ← this file
├── CLAUDE.md               ← build commands + hard rules (short)
├── LICENSE                 ← GPL-3.0-or-later
├── Cargo.toml              ← workspace
├── crates/
│   ├── mite-dsp/           ← pure DSP primitives. No I/O, no framework, no alloc in process()
│   │   └── src/{filter/, dynamics/, oversample/, fft/, conv/, loudness/, delay/, mod/, util/}
│   ├── mite-core/          ← param model, smoothing, versioned state, telemetry rings,
│   │                         undo model, A/B/C/D snapshots, preset format
│   ├── mite-ui/            ← design tokens, Vello scene kit, widget library, shaders/,
│   │                         interaction model, AccessKit bridge, fonts/
│   ├── mite-plugin/        ← THE ONLY crate that imports nih-plug. Adapts MiteProcessor
│   │                         + MiteEditor to CLAP/VST3. clap-wrapper glue for AU.
│   └── mite-test/          ← offline render harness, null/THD/aliasing measurements, golden IO
├── plugins/
│   ├── mite-eq/            ← one crate per product: dsp.rs, params.rs, editor.rs, presets/
│   └── …
├── xtask/                  ← bundle / sign / notarize / install / validate
├── docs/
│   ├── design/             ← Claude Design deliverables: tokens.json, mockups/*.html, specs
│   └── dsp/                ← algorithm notes, measurement reports
└── tests/golden/           ← parameter golden files, reference renders
```

**What a new plugin reuses (as of 2026-10-22, extracted from Mite EQ before plugin 2):**

| Need | Where |
|---|---|
| Presets: file format, user folder per OS, factory + user list | `mite_core::presets` (`PresetLibrary`, `CurrentPreset`) |
| Snapshots A–D store | `mite_core::snapshots` |
| Undo/redo steps | `mite_core::History` |
| Header: ‹ preset ›, preset list with Save as…, undo/redo, A–D, their help | `mite_ui::session::Session` (the plugin gives `SessionConfig`: name, factory files, sound ids, snapshot ids) |
| Recording the editor's own edits for undo | `mite_ui::RecordingHost`, `Session::begin`/`end`/`follow` |
| Help strip, dropdown lists, icon buttons, glass panels, glow | `mite_ui::widgets::{Help, Dropdown}`, `mite_ui::paint::{icon_button, glass_panel, glow_stroke}` |
| Footer: toggles, dials with name and value, latency, dropdown buttons | `mite_ui::widgets::Footer`, `widgets::footer::draw_dropdown_button` |
| Per-slice history telemetry (levels, GR) | `mite_core::HistoryTap` (Comp records one per 5 ms) |
| Dynamics: stages, detectors, Characters, Smart | `mite_dsp::dynamics` (Limit, Multiband, Gate and DeEss can build on it) |
| State version, migrations, snapshot/preset fields | `mite_plugin::state` (`migrate` from `Plugin::filter_state`; `session_fields`/`load_session_fields` from `Params::serialize_fields`/`deserialize_fields`) |
| Golden state test through the real CLAP entry point | `mite_plugin::testing::check_golden_states` (`tests/golden/<plugin>.state.v<N>.json`) |
| Parameter golden manifest | `mite_core::manifest` (`tests/golden/<plugin>.params.txt`) |
| CI bundling and validation | add the package to `PACKAGES` and its bundle name to `BUNDLES` in `.github/workflows/ci.yml` |

`mite-test` does not exist yet; measurement helpers live in the crates' tests and examples.

**Dependency direction (enforced by the crate graph):**

```
plugins/* → mite-plugin → mite-ui → mite-core → mite-dsp
                                     ↑
                         mite-test ──┘
```

`mite-dsp` depends on nothing but `std`, `realfft` and SIMD helpers.

---

## 4. Reuse from Nova (`~/Nova`)

Nova's engine is C++20. We **port algorithms and philosophy, not code**. Every port gets
re-derived, re-tested in Rust, and improved where Nova was weak.

### 4.1 Port (strong in Nova)

| What | Nova source | Notes for the port |
|---|---|---|
| 4-node series compressor curve: never inverts; compress, expand, up-compress and up-expand quadrants | `engine/src/plugin/native/dsp/CompCurve.h`, `NovaComp.h` | Also port the 7 Characters table, the Smart modes (adaptive release, auto-threshold, sibilance guard), M/S with continuous link, Delta, and curve-aware auto-makeup. Vectorise the per-sample `log`/`pow`. |
| Overshoot-proof brickwall limiter: sliding-min, then boxcar, with attack equal to lookahead | `dsp/Limiter.h`, `NovaLimiter.h` | Keep the provable no-overshoot property and the 5 styles plus program-dependent release. Replace its oversampler. |
| True-peak detector, 4× polyphase sinc | `dsp/Limiter.h` (`TruePeak`) | Upgrade to a BS.1770-4-compliant interpolator; test it against the ITU test vectors. |
| Level-independent transient detector (envelope comparison, no threshold) | `NovaShaper.h` | Becomes Mite Shape, plus the Transient band mode in the EQ and multiband plugins. |
| LR4 crossover with allpass compensation, which sums flat | `dsp/Crossover.h` | Add a linear-phase and dynamic-phase variant (see Multiband). |
| `RtHandover<T>`: lock-free swap with retire slots | `dsp/NamSlot.h` | Becomes the `mite-core::handover` crate module, used for IRs, FIR kernels and models. |
| Parameter definition table: id, range, log scaling, step names, smoothing ms | `NativePluginInstance.h` | Becomes declarative `ParamSpec`, with **stable string IDs that never change**. |
| Versioned state with migration hooks | `NativePluginInstance.h`, `NovaComp.h` (v1→v2) | Becomes a binary or CBOR state with `version` and migration chain, plus a golden round-trip test per version. |
| Delta / listen / sidechain-listen everywhere | Comp, Limiter, Shaper, Gate | Required feature on every dynamics plugin. |
| Latency reported to the host | EQ, Limiter, Gate, Shaper, Amp | Latency changes are **signalled** to the host, not polled. |
| Test philosophy: 85 DSP tests, NaN/Inf recovery for every effect, parameter-ramp click tests, parameter golden file | `engine/test/NativeDspTest.cpp`, `engine/test/data/native_params.json` | Carry over as the minimum bar for every plugin (§10). |
| NAM neural amp hosting | `NovaAmp.h`, `dsp/NamSlot.h` | Later (Mite Amp), via FFI to NeuralAmpModelerCore (MIT) or a pure-Rust NAM implementation. |

### 4.2 Fix or replace (weak in Nova)

| Nova weakness | MiteFX replacement |
|---|---|
| `Oversampler2x` is a single 2nd-order Butterworth, roughly 12 dB/oct, so it barely suppresses aliasing | Polyphase **half-band FIR** (linear phase) and **elliptic IIR half-band** (min phase, low latency) cascades at 2/4/8/16×. Requirement: ≥ 100 dB stopband in HQ. |
| Naive radix-2 FFT copy-pasted in 5 places | One `mite-dsp::fft` wrapper over `realfft`, with plans cached at `prepare()`. |
| Linear-phase EQ as a direct 2049-tap FIR, redesigned **on the audio thread**, with a hard cut on mode switch | Uniformly partitioned **FFT convolution**. Kernels are designed on a worker thread and swapped via handover with a crossfade. Selectable latency: Low / Medium / High / Max. |
| RBJ biquads, which cramp near Nyquist | **ZDF/TPT state-variable filters** (stable under fast modulation) plus **Vicanek matched** magnitude designs, so analog-matched curves hold up to 20 kHz at 44.1 kHz with zero latency. |
| Momentary LUFS only | Full **EBU R128 / BS.1770-4**: momentary, short-term, integrated (gated) and LRA, plus true-peak and PLR. |
| NaN recovery calls `prepare()`, which allocates and for Amp does file I/O, on the audio thread | `reset()` is a separate, alloc-free method. Recovery calls only `reset()`. Allocation in `process` or `reset` panics in debug builds. |
| Block-rate parameter smoothing; linear parameter lookup by id | **Sample-accurate parameter events** with per-parameter smoothing curves (linear, log or exponential), and an O(1) index. |
| Telemetry as JSON strings polled at 30 Hz | Lock-free **typed telemetry rings** (`triple_buffer` for spectra, `rtrb` for meter and history events) read by the GUI each frame. |
| Stereo-only process | Bus-layout aware: mono, stereo and sidechain now. Surround and Atmos (7.1.4) have an architectural slot (channel-agnostic DSP kernels). |
| Multiband dry/wet comb filtering at Mix < 1 | Dry path runs through the same allpass network, or Mix is applied per band. Always tested by a null test. |
| Time-domain cab IR (≤ 4096 taps) | Non-uniform partitioned convolution: zero latency and long IRs. |

---

## 5. Real-time and quality rules (hard rules)

1. **The audio thread never allocates, locks, does I/O, logs or panics.** `assert_no_alloc` wraps `process()` and `reset()` in debug and CI builds.
2. `prepare(sample_rate, max_block, layout)` is the **only** place buffers are sized. It runs off the audio thread.
3. Anything expensive (FIR design, IR load, model load, preset parse) runs on a worker thread and arrives via `handover`, with a click-free crossfade.
4. **Every parameter change is click-free.** Smoothing is specified per parameter, and a ramp test exists for every continuous parameter.
5. **Stable parameter IDs.** Once released, a param ID and its meaning are frozen. Renames are UI-only.
6. **State is versioned and migrated.** Never reinterpret old state silently. Every version has a golden blob test. **Done (2026-10-22):** every plugin saves `mitefx_state` (its `STATE_VERSION`; states without it are version 1) and migrates in `Plugin::filter_state` through `mite_plugin::state::migrate` (one step per version). `tests/golden/<plugin>.state.v<N>.json` holds a non-default state of every version; the test loads each through the plugin's real `clap_entry` (a minimal CLAP host in `mite_plugin::testing`), saves it again and compares with the file, or with its `.expected.json` once it is an old version. Mite Gain and Mite EQ are at version 1.
7. **Latency is honest.** The reported value equals the measured group delay (tested). Latency changes are signalled to the host.
8. **Denormals:** set FTZ/DAZ per process call (x86 and ARM), plus DC-safe feedback paths.
9. **NaN/Inf guard:** output is scrubbed. Recovery is alloc-free `reset()`, tested per plugin.
10. **DSP crates are framework-free.** Nothing in `mite-dsp` or `mite-core` imports nih-plug, baseview or wgpu.
11. **The UI never owns DSP state.** It reads parameters and telemetry, and writes parameters through gestures (begin, set, end) for correct host undo and automation.
12. **Mono, stereo and sidechain must all work**, and every plugin must handle `max_block` = 1…8192, plus variable block sizes.
13. **CPU budget:** every plugin documents its cost in §8-style tables, and CI fails on a >10 % regression in the `criterion` benchmarks.

---

## 6. Plugin roster

Working names are placeholders; Claude Design may propose the brand naming.
"Rival" is the plugin we must beat.

### 1. Mite EQ (flagship, MVP). Rival: FabFilter Pro-Q 4

Full specification in §8.

**Headline features beyond Pro-Q 4:**
- Per-band **resonance suppression** (soothe-style) as the spectral compressor's relative threshold (decided 2026-10-16: no separate band type).
- **Phase-rotation / allpass** bands.
- **Transient-aware dynamic bands**: the band can respond to attacks only or to sustain only.
- **Snapshots** A, B, C and D (switch and copy). Morph between them was dropped (2026-10-16).
- **Cross-instance masking map** with one-click "carve" suggestions.

### 2. Mite Comp. Rival: FabFilter Pro-C 3

- Starts from Nova's 4-node series curve.
- 7 Characters (Clean, Punch, FET, Opto, Bus, Vari-Mu, Custom).
- Smart modes: Adaptive, Vocal and Master.
- Upward and downward compression and expansion in one curve.
- Feed-forward and feedback.
- M/S with continuous link.
- External sidechain with a sidechain EQ (a full Mite EQ band set).
- Lookahead and Delta.
- **Beyond:** a "Character morph" between models; a per-node detector (peak, RMS or loudness); and stateful analog-style saturation in the gain cell, oversampled.

**Status (2026-10-22): first version built** to the pass-4 design (`docs/design/mockups/comp-main.dc.html`, interaction.md "Mite Comp"). DSP in `mite_dsp::dynamics`: up to four stages in series in threshold order, each in one quadrant, with a soft range cap (no corner). **Thresholds and knees are on the input (2026-10-23, user decision: a higher stage must never change a lower one):** a stage starts where the input reaches its threshold, whatever the stages below it do, and its knee spans its width of input (in the level it works on it is scaled by their slope, so a knee behind a hard stage no longer reaches far below its threshold). The curve below a stage's knee, and the curve a stage and those below it make (where its node sits), are never touched by the stages above (tested in the DSP and through the editor). A stage acting below its threshold (expander, upward compressor) still moves everything under it, by nature. State version 2 migrates version 1 (thresholds and knees in the level the stages below left) to the same curve, in the parameters and every snapshot (`plugins/mite-comp/src/migrate.rs`); Snare Snap's second stage was converted. The level thresholds are worked out per sample only while something glides: 1.01 % of a core with four stages (1.23 % gliding), against 0.90 % (0.96 %). A fixed-node model (each node a fixed point, ratio derived from the segments) was tried the same day and dropped: it let a higher stage reshape the stretch below it; per-stage Peak (instant attack, 10 ms release, so a gate does not chatter on zero crossings; tested), RMS and K-weighted loudness detectors; the six Characters plus Custom shape the motion only (attack/release scale, drive, program dependence, slow tail, Vari-Mu ratio bend); feed-forward or feedback; L/R or M/S with continuous link; lookahead to 20 ms (latency reported, a dip around changes); hold; makeup with curve-aware auto makeup; Smart (crest-adaptive release; Vocal and Master slide every threshold to hold a target GR; Vocal's sibilance guard); external key through a 3-band detector EQ; Listen; stage solo. `curve_db`, `partial_curve_db` and `level_before` are what the editor draws: a stage's diamond sits where it starts on the curve the stages below make, and its node on the curve it and those below make, so the stages above never move it (when one starts before the node, the stage's own continuation to it is drawn dashed). Every parameter glides over 20 ms (click test). Engine (`plugins/mite-comp/src/dsp.rs`): latency-aligned dry path (bypass and mix 0 null exactly, delta + dry = wet), output identical for every block size, one history record per 5 ms (`mite_core::HistoryTap`). Editor: the history (IN, OUT, GR from the top, lift from the bottom; it scrolls with the clock between host blocks), threshold lines and grips, the transfer square (shading, nodes, diamonds, live dot and trail, ghost grips when empty), stages made by pulling the line (quadrant from the pull; 4:1 at 48 px, ∞ at 160 px; a fifth refused), the stage strip, IN/GR/OUT meters, the docked stage panel (envelope scope through the stage's real ballistics with attack/release handles, quadrant radio, detector, dials, bin), Custom, Sidechain (mini EQ), Detector & output, stereo popover, Character and Smart footer menus (hovering a Character auditions it through `CompShared::audition`, never the parameter). Deviations from the boards: no words in the display (the Δ and Listen pills are glowing icons; the empty-state hint is the help strip); host parameter slots never swap (stage numbers re-rank by threshold instead). Measured: 120 fps steady in the preview at 2× DPI; frame GPU+render 1.7 ms median (2.6 ms with the stage panel open) at 2560×1520; clap-validator 34/0 and pluginval strictness 10 with GUI tests pass. **Later:** oversampling and Character saturation (the oversampler is shared with Limit and Sat), the compact layout (with the EQ's), help mode, right-click menus, a key spectrum behind the sidechain EQ.

### 3. Mite Limit. Rival: FabFilter Pro-L 2

- Nova's overshoot-proof core and 5+ styles.
- True-peak limiting with HQ oversampling up to 16×.
- Full R128 metering (M/S/I/LRA/PLR).
- **Beyond:**
  - Streaming loudness targets (Spotify, Apple, YouTube, Tidal, broadcast) with "what will the platform do" preview.
  - **Unity-gain audition** to judge the limiting itself rather than the loudness.
  - Multiband-assisted transient protection.
  - Clipping-before-limiting stage with its own style.

### 4. Mite Multiband. Rival: FabFilter Pro-MB

- Up to 6 bands, each with compress, expand, up-compress, up-expand and gate.
- Crossover modes: minimum phase (LR4/LR8), linear phase, and **dynamic phase** (Pro-MB's trick).
- Per-band M/S and sidechain.
- **Beyond:**
  - Per-band Nova 4-node curves.
  - Per-band transient and sustain targeting.
  - Bands draggable directly on the spectrum, like EQ bands.

### 5. Mite Verb. Rival: FabFilter Pro-R 2

- Hybrid algorithmic engine: input diffusion → early reflections → modulated FDN (16 lines, time-varying).
- Decay-rate EQ (Pro-R style) plus post EQ.
- "Space" morph and stereo width.
- Freeze and ducking.
- **Beyond:**
  - Convolution mode with IR import, and **reverb as an IR-matched algorithm** (analyse an IR, fit the algorithm to it).
  - A real-time decay spectrogram display.

### 6. Mite Delay. Rival: FabFilter Timeless 3

- Up to 4 taps with sync, ping-pong, tape, BBD and digital modes.
- Filters and diffusion inside the feedback loop.
- Ducking.
- A full **modulation matrix** (LFO, envelope follower, MIDI and XY sources), shared with Sat and Filter.
- **Beyond:** a granular/pitch "smear" stage and per-tap EQ on the spectrum display.

### 7. Mite Sat. Rival: FabFilter Saturn 2

- Multiband, up to 6 bands (from Nova Sat).
- **Stateful** analog models: tape with hysteresis, tube stages with bias drift, transformer, and diode clippers via ADAA (antiderivative anti-aliasing) plus oversampling.
- Per-band dynamics.
- Modulation matrix.
- **Beyond:**
  - A harmonics display showing actual measured harmonic levels per band.
  - Mid/side per band.

### 8. Mite DeEss. Rival: FabFilter Pro-DS

- Wideband and split-band modes.
- Single-vocal and allround detection.
- Lookahead.
- **Beyond:** spectral mode (soothe-like, attenuates only the offending bins) and a detection heat-map display.

### 9. Mite Gate. Rival: FabFilter Pro-G

- Gate, expander and ducker.
- Hysteresis, hold and lookahead.
- External sidechain with a sidechain EQ.
- MIDI trigger output (via CLAP note output).
- **Beyond:**
  - Transient-triggered gating using the Shape detector.
  - Spectral gate mode for denoise.

### 10. Mite Shape. Rival: none from FabFilter (SPL Transient Designer, Kilohearts)

Nova Shaper ported:
- Threshold-free detection, attack and sustain.
- Multiband 1–5.
- M/S and Delta.
- **Beyond:** sustain-length control in ms, and a clip/soft-limit stage.

### 11. Mite Soothe. Rival: oeksound soothe2

- Dynamic resonance suppression with sharpness and selectivity.
- M/S.
- Sidechain (for de-masking against another track).
- Delta.
- Shares its engine (`mite-dsp::spectral`) with the EQ's spectral compressor.

### 12. Mite Filter. Rival: FabFilter Volcano 3

- 4 filters with routing.
- Analog-modelled ZDF ladder, SVF, comb, formant and vowel filters.
- Modulation matrix.
- Drive per filter.

### 13. Mite Mod. Rival: Twin-style modulation, Soundtoys

- Chorus, ensemble, flanger (through-zero), phaser (2–24 stages), vibrato and rotary in one plugin.
- Each mode has its own deep editor.
- Nova's LFO shapes plus the modulation matrix.

### 14. Mite Meter and Mite Utility

- **Mite Meter:** loudness (R128 with history graph), true peak, spectrum, spectrogram, phase scope, correlation and stereo imager.
- **Mite Utility:** gain, pan law, width, mono, phase, swap, DC and bass-mono below a set frequency.

### 15. Mite Amp (later)

- NAM neural amp models (A1 and A2).
- Partitioned cab IR convolution.
- Resampling with a delay-matched dry path.
- Ports Nova Amp minus its bugs.

Shared engines across the suite:
- **Filter bank:** EQ, Comp sidechain, Verb decay EQ, DeEss.
- **Dynamics cell:** Comp, Multiband, Gate, DeEss and EQ dynamic bands.
- **Spectral engine** (`mite-dsp::spectral`): EQ, Soothe and DeEss.
- **Modulation matrix:** Delay, Sat, Filter and Mod.
- **Loudness:** Limit, Meter, auto-gain everywhere.

---

## 7. Shared UX system (Claude Design brief)

### 7.1 What we want to beat

FabFilter's strengths:
- Calm dark canvas and a big interactive display.
- Direct manipulation (drag nodes on the curve).
- Contextual floating controls near the selected node.
- Excellent motion and responsiveness.
- Consistency across the line.

**We go further on:**
- **Depth and light:** subtle glow on active curves, bloom on peaks, glass panels with real-time blur over the spectrum, and a spectrum with heat and gradient fills. All of it is GPU-rendered at the monitor refresh rate.
- **Information density without clutter:** progressive disclosure. The default view is minimal and power features slide in.
- **Motion with meaning:** easing that follows the DSP (gain-reduction needles with physical ballistics, band nodes that "breathe" with their dynamic gain).
- **Personalisation:** themes (dark, light, high-contrast and colour-blind-safe palettes), user accent colour, and UI scale from 50 % to 400 %, all vector-based.

### 7.2 Behaviour every plugin shares

- **Window:** freely resizable and aspect-aware, with a full-screen mode. Layout breakpoints at compact, standard and large sizes.
- **Header bar:**
  - Plugin logo and menu.
  - Preset browser: tags, search, favourites, preview-on-hover with audition.
  - Undo and redo; the UI undo history is separate from host automation.
  - A/B/C/D snapshots and copy between them.
  - Morph slider (where supported).
  - Help, settings and the oversampling or quality selector.
- **Footer bar:**
  - MIDI learn.
  - Latency readout.
  - Mix (global dry/wet) and output gain.
  - Auto-gain.
  - Delta, bypass and channel mode (Stereo, L, R, M, S).
- **Main display:** spectrum or graph with direct manipulation.
  - Double-click creates.
  - Drag moves; Shift gives fine control.
  - Alt-drag adjusts the secondary parameter.
  - Mouse wheel adjusts Q or width.
  - Right-click opens a context menu.
  - Multi-select, lasso, group-drag and numeric entry on any value.
- **Floating band panel** next to the selected item, following FabFilter's pattern, with richer content.
- **Help overlay:** hover any control to see an explanation in the help strip (Nova's `NativeHelp` idea), plus a "?" mode that annotates the whole UI.
- **Accessibility:** full keyboard navigation and a visible focus ring. AccessKit names every control and reports its value. Colour-blind-safe band colours. Reduce-motion setting.
- **Touch-ready:** hit targets of 32 px or more at 100 % scale and no hover-only functionality, so AUv3/iPad is possible later.

### 7.3 Components to design (inventory)

**Controls and readouts:**
- Knob: standard, small and bipolar, with a modulation ring.
- Slider: horizontal and vertical.
- Toggle, segmented control and dropdown.
- Numeric field with drag and type entry.
- Meters: peak/RMS, gain-reduction, loudness and correlation.

**Main-display layers:**
- Spectrum: pre and post, sidechain and collision.
- Curve layer: total curve and per-band curves.
- Band node: states for idle, hover, selected, dynamic, solo, bypass and disabled.
- Piano-roll overlay with frequency-to-note lookup.
- Collision/masking overlay.
- Transfer-curve editor (Comp).
- Waveform history with a gain-reduction overlay (Limit, Comp, Gate, Shape).
- Spectrogram (Meter, DeEss, Soothe).

**Chrome and panels:**
- Header and footer bars.
- Floating band panel.
- Preset browser.
- Settings sheet.
- Help strip.
- Modulation matrix (shared).
- Multiband crossover bar.

### 7.4 Design status

**Mite Comp brief sent on 2026-10-22** (`docs/design/comp-brief.md`): the main view (history and transfer curve), creating stages by pulling the curve, the stage panel, Character, Smart, sidechain EQ, stereo link, compact, and a new upward-gain colour token.

**Second pass received on 2026-10-14.** All EQ screens are delivered (12 states × dark, light and colour-blind themes, plus a compact layout), as are the shared components. A short third pass covers accent and token clean-up; see `docs/design/third-pass-brief.md`.

**First pass received on 2026-10-14.** The canvas is https://claude.ai/artifact/W6i7s8rA3Q81U43ruRusH5, with a snapshot and review in `docs/design/README.md`.

What it settles:
- **Brand:** `miteFX`, monoline mite mark, per-plugin accent and glyph.
- **Font:** Manrope only, with tabular readouts.
- **Base:** near-black blue-grey neutrals with glass panels and glow.
- **Knob:** tick-scale dial.
- **EQ layout:** header, display, help strip, footer.
- **Band panel:** glass, floats above or below the node.

`docs/design/tokens.json` is the source for `mite-ui::tokens`. Open items for the second pass (colour collisions, contrast, missing screens, `interaction.md`) are listed in the README's review section. The brief sent to Design is `docs/design/second-pass-brief.md`.

### 7.5 Deliverables expected from Claude Design (into `docs/design/`)

1. **`tokens.json`:** colour, with full dark and light palettes, the band colour ramp, and semantic colours (gain-reduction, sidechain, collision); typography; spacing; radii; elevation and glow; blur; motion durations and easings. We translate this 1:1 into `mite-ui::tokens`.
2. **`mockups/*.html`:** Mite EQ screens (empty, 3 bands, dynamic band, linear-phase, collision view, preset browser, compact size, light theme), plus a component sheet.
3. **`interaction.md`:** gestures, states and transitions for every component, including timings.
4. **Brand:** suite name and logo direction, per-plugin glyph, and the accent rule (each plugin gets one accent hue on the shared neutral base).

Technical constraints for Design:
- Everything must be renderable as vector paths plus gradients plus blur/glow shader passes. Raster images are allowed only for subtle noise textures.
- Fonts must be OFL-licensed (we embed them). Chosen: Manrope.
- Animations must be describable as property tweens with easing curves.

---

## 8. Mite EQ: full specification (MVP)

### 8.1 Bands

- **Up to 24 bands.** Each band has: `enabled`, `type`, `freq` (10 Hz–30 kHz), `gain` (±30 dB), `q` (0.025–40), `slope`, `stereo` (Stereo/L/R/M/S), `solo`.

- **Band types:**
  - Bell
  - Low Shelf / High Shelf (variants: the shelf's Q runs from gentle, Baxandall-like, at low Q to vintage-style overshoot at high Q; no separate variant types)
  - Low Cut / High Cut
  - Notch
  - Band Pass
  - Tilt Shelf
  - Flat Tilt **(done 2026-10-16:** one straight slope across the whole range through the pivot, gain / 5.8 dB per octave, from nine first-order pole/zero pairs spread evenly in log frequency; within 0.3 dB of the line from 20 Hz to 20 kHz, tested; first-order matched sections are now matched at their own corner, between zero and pole, instead of the band frequency, which only changes the flat tilt's pairs: its IIR alone went from 3.0 to 0.25 dB worst at 44.1 kHz, and Zero Latency's correction takes it to 0.000 dB)
  - **All-Pass / Phase Rotate** **(done 2026-10-16:** second order at the frequency and Q; the digital section mirrors its denominator, so the level is exactly 1 and only the phase turns, −180° at the frequency, tested; Natural Phase follows the analog phase; Linear Phase, being zero phase, leaves it without effect, and the help strip says so; solo plays its band-pass region)
- **Slopes (cuts and shelves):** 6, 12, 18, 24, 30, 36, 48, 72 and 96 dB/oct, plus **Brickwall** (linear-phase modes only). **Brickwall done (2026-10-16):** the last slope choice; Linear Phase builds it as a vertical edge (a raised cosine over 1/24 octave down to −150 dB; tested: a tone a third of an octave past the corner is more than 60 dB down, a tone just inside passes within 0.1 dB); Zero and Natural Phase play 96 dB/oct for it, the plot draws whichever plays, and the help strip says so.

**Dynamic EQ (per band):**
- Parameters: `dyn_range` (±30 dB), `threshold` (or auto), `attack`, `release`, `ratio`/knee.
- Detection: band-internal, external sidechain, or another band.
- Direction: by the range's sign (pull up: the band rises when loud; down: it ducks). No below-threshold mode (tried and dropped 2026-10-16).
- **Transient/sustain focus** (beyond).

**Spectral dynamics (per band):** works per FFT bin inside the band, for surgical, dynamic resonance taming. The threshold is a fixed level or relative to each frequency's surroundings (soothe-style).

**Character (global):** Clean, Subtle and Warm. The coloured modes add oversampled saturation.

### 8.2 Processing modes

| Mode | Latency | Implementation |
|---|---|---|
| **Zero Latency** | 0 | Matched-magnitude IIR plus a minimum-phase magnitude correction FIR (no delay): the analog magnitude up to Nyquist |
| **Natural Phase** | 4.0 ms at 48 kHz (192 samples, scaled with the sample rate) | Matched IIR sections plus a 256-tap correction FIR (analog ÷ digital, complex, per 2×2 L/R matrix), so magnitude and phase follow the analog prototypes up to Nyquist |
| **Linear Phase** (Low / Medium / High / Max) | 13.3, 24.0, 45.3 or 88.0 ms at 48 kHz (1024–8192-tap FIR, scaled with the sample rate, plus one 128-sample block) | Uniformly partitioned FFT convolution (f64, 2×2 kernel matrix for L/R/M/S); kernel designed on a worker thread from the analog magnitude; crossfaded swap |

Mode switches are click-free. Dynamic bands in linear-phase mode use the Pro-Q approach: linear-phase static filtering, with dynamic gain applied through parallel filtering, latency-aligned.

### 8.3 Analyzer

- Pre, post and sidechain spectra. FFT size is 1k–32k with selectable range, speed (ballistics) and tilt (0, 3 or 4.5 dB/oct).
- **Spectrum grab:** hover the spectrum to see peaks; drag a peak to create a band there. **Done (2026-10-16)**, see Phase 1 slice 5.
- **Collision detection** against the sidechain or other MiteFX EQ instances (a shared-memory instance registry). The masking map highlights conflicting regions.
- **Carve suggestions** (from design pass 2): **Carve** on this instance, **Carve on <other>**, or **Ignore**. "Carve on <other>" writes to the other instance, so the instance registry needs a command channel as well as spectrum sharing.
- **Preset audition on hover:** crossfades into the hovered preset's state on the live signal without touching undo or the host's parameters, and reverts when the pointer leaves.
- **EQ Match:** match the spectrum to a reference (sidechain or an imported file) with smoothing and band-count limits.
- Piano-roll strip that snaps bands to notes.
- Auto-gain: gain compensation from loudness, not peak.

### 8.4 Global parameters

| ID | Meaning |
|---|---|
| `mode` | processing mode |
| `lp_res` | linear-phase resolution |
| `character` | character mode |
| `out_gain` | output gain |
| `out_pan` | output pan |
| `mix` | global dry/wet |
| `auto_gain` | loudness-based gain compensation |
| `delta` | Delta listen |
| `bypass` | bypass |

**Status (2026-10-16):** `out_gain`, `out_pan`, `mix`, `auto_gain`, `delta`, `bypass`, `mode` and `lp_res` exist (plus per-band `bNN_solo`); `character` follows with its slice; `morph` was dropped (snapshots switch, they do not blend). `gain_scale` was built and removed again (2026-10-16): not worth a control.

**Per-band ID scheme:** `b{NN}_{param}` (for example `b03_freq`, `b03_dyn_range`). All 24 bands are pre-declared, because hosts need static parameter lists. IDs are frozen once released (rule 5.5).

### 8.5 Performance targets (48 kHz, 128-sample buffer, one modern x64 core)

| Configuration | Target |
|---|---|
| 8 static bands, Zero Latency | < 0.5 % CPU |
| 8 dynamic bands, Zero Latency | < 1.5 % CPU |
| 24 bands, Linear Phase High | < 3 % CPU |
| 8 spectral bands (relative) | < 1.5 % CPU (measured 1.14 %) |
| GUI open at 120 fps, 4k-wide spectrum | < 1 ms GPU frame time, < 3 % CPU (2026-10-22, 2560×1520 with bloom and glass: 1.6–2.4 ms including the CPU–GPU round trip; not yet met) |

### 8.6 Acceptance

- Measured magnitude matches the analog prototype within 0.1 dB up to 20 kHz at 44.1 kHz (Zero Latency).
- Linear-phase group delay is flat within 0.01 samples.
- Automation of every parameter and snapshot switches are click-free (ramp tests).
- State round-trips across versions.
- The plugin passes `clap-validator`, `pluginval` (strictness 10) and `auval`.
- The plugin loads and saves correctly in Reaper, Bitwig, Ableton Live, Logic, FL Studio, Cubase and Nova on the relevant OSes.

---

## 9. Roadmap

### Phase 0: Foundation

- Cargo workspace and the crate skeletons from §3.
- CI on Windows, macOS and Linux.
- `cargo xtask bundle|install|validate`.
- **Mite Gain:** a hello-world plugin through `mite-plugin`. It has one param, state, latency reporting and a Vello UI with one knob and a meter.

**Exit criteria:**
- CLAP, VST3 and AU bundles pass the validators in CI.
- The plugin loads in Reaper and Bitwig on all three OSes.
- `assert_no_alloc` is active in debug and CI builds.

**Status (2026-10-14):**

Done and verified on Linux:
- **Workspace:** the workspace and its crates; Mite Gain with a golden parameter manifest; `cargo xtask bundle|install|validate`.
- **Validators:** clap-validator passes every applicable test (34 of 34), and pluginval passes at strictness 10, in both debug and release builds.
- **Real host:** the editor opens and draws inside Reaper (checked by capturing its window).
- **Fixes along the way:**
  - **Windows were never shown.** Fixed by calling `show()` after creating them.
  - **Presentation:** frames go straight from the GPU to the window (wgpu surface, mailbox mode, synced to the display without blocking). Measured inside Reaper: 0.8 ms per frame for the full EQ at 2560×1520. A softbuffer CPU path remains only as a fallback when no GPU surface can be created. An earlier diagnosis that Vulkan could not present into host child windows under XWayland was wrong: the windows were simply never shown or redrawn.
  - **baseview's Present-extension redraw pacing never fires inside Reaper.** Frames are now driven by our own 60 Hz UI timer.
- **nih-plug fork:** we pin the fork `ansabic/nih-plug`, branch `mitefx`, which fixes two CLAP state bugs:
  - the untrusted state length could abort the host;
  - parameter values were not rescanned after a state load.
- **Design:** third-pass tokens (`mitefx-tokens/3`) and `docs/design/interaction.md` are wired in.

CI (Windows, macOS, Linux) runs on every push: tests, clippy, bundles, clap-validator and pluginval strictness 10 (GUI tests skipped: hosted runners have no GPU). Since 2026-10-22 it bundles and validates every plugin (Mite Gain and Mite EQ), not only Mite Gain.

Open:
- AU via clap-wrapper.
- Bitwig and Ableton, and macOS and Windows hosts.
- The WGSL bloom pass. **Done (2026-10-22)**, see Phase 1 slice 9.
- Exact log parameter ranges.
- Offering the two nih-plug fixes upstream.

### Phase 1: Mite EQ (MVP)

**Status (2026-10-14), slice 1 done:**
- **Filters and engine:**
  - Zero-latency matched filters (`mite-dsp::filter`, `mite-dsp::eq`): Bell, Low/High Shelf, Low/High Cut at 6–96 dB/oct, Notch, Band Pass and Tilt.
  - Cuts are designed as inverted boosts, so boost and cut are exact mirror images.
  - 24 bands with click-free smoothing; type, slope and on/off changes crossfade; disabled bands are bit-exact.
- **Analyzer:** `realfft`, 1/12-octave smoothing, 4.5 dB/oct tilt.
- **Display range (2026-10-22, as FabFilter's):** the plot's dB scale is ±3, ±6, ±12 or ±30 (labels at the range; the plot's edge at 1.5 ×), or Auto (the default): it keeps ±12 and widens to ±30 when a band's gain, or gain plus dynamic range, would leave the plot. A pill at the top of the scale shows the range (an A while Auto chooses) and opens the list; the wheel over the scale steps; double-click returns to Auto. The scale glides (log easing, 120 ms); the grid and labels are the target range's round numbers. A picked range is saved with the state (`display_range`; Auto writes nothing, so older states read as Auto). Shared: `mite_ui::widgets::RangePicker`. Mite Comp uses it for the history's level range (24/36/48/60 dB or Auto, which fits the music's quiet tenth and the deepest stage's line; the GR scale zooms with it).
- **Parameters:** 145 frozen IDs (`b01_freq` …) checked against a golden manifest.
- **Editor**, per the mockup:
  - spectrum, curves and nodes;
  - glass band panel;
  - add, drag, Shift-fine, Alt or wheel for Q, reset and delete.
- **Validation:** clap-validator and pluginval (strictness 10) pass, debug and release. Draws in Reaper.

**Measured accuracy** of the analog match:
- Bells up to the mid range: within 0.01–0.1 dB.
- Cuts: exact slopes.
- Bands near Nyquist: the matched IIR alone was worse than this line used to say (measured 2026-10-16 with `mite-dsp/examples/nyquist_error`, up to 20 kHz at 44.1 kHz where the prototype is within 24 dB of unity): bells up to 1.3 dB, cuts up to 8 dB, notches up to 7 dB, and resonant shelves (Q 8) up to 26 dB off; wide bells at 1–2 kHz already 0.25–0.5 dB, because their skirts reach Nyquist. **Done (2026-10-16): Zero Latency correction** (`mite-dsp::min_phase`). The worker designs, for the bands' static settings, the residual analog/digital in magnitude per channel (and per mid/side for mid/side bands), and turns it into a **minimum-phase FIR** (real cepstrum, folded; no delay, so Zero Latency stays zero, tested causal from the first sample). It uses the shortest of 64/128/256/512 taps (× the rate ratio) that brings the response within 0.05 dB of the analog curve where it is audible; the median setting needs 64. Stopbands (analog below −40 to −60 dB) are left alone; corrections go up to ±30 dB. Result: every type within 0.05 dB up to 20 kHz at 44.1 kHz, except high shelves at Q 8 within about 1.5 kHz of 20 kHz (4 of 112 test settings, at most 0.48 dB). It corrects the magnitude only: the phase stays minimum phase, a few degrees off the prototype in the mids and about 15° near Nyquist (as before; Natural Phase matches the phase, tested). A new correction crossfades in over 20 ms (`CorrectionFilter`, direct-form FIRs in eight lanes, allocation-free; tested click-free while a high bell is automated). The plot now draws every band's analog curve up to 30 kHz in every mode, which is what each mode plays. CPU (`bench_dyn`): 8 static bands 0.39 % (0.30 % without), 8 dynamic 0.86 %, the worst case (8 resonant shelves near Nyquist, 512 taps) 0.84 %.

**Next slices:**
1. Dynamic EQ. **Done (2026-10-14):** band-internal detection, above-threshold, range/threshold/attack/release/ratio per band; 8 dynamic bands measured at 0.62 % of a core worst case (`examples/bench_dyn`). **Sidechain and other-band detection done (2026-10-16):** each band's `dsrc` (Own, Sidechain, Band 1–24). The plugin has a sidechain input (stereo, mono with mono), silent when unconnected. A band detecting from the sidechain hears its own region of it (`BandProcessor::process_detecting`); in Linear Phase the sidechain is delayed by the kernel's latency, because the bands' dynamics run after the kernel there (tested: the duck lands where the output shows the key, in both modes; it lands 45 ms early without the delay). A band detecting from another band listens through that band's region (frequency, Q, type) of the signal it processes (`set_detect_region`, set each block; tested: a 1 kHz band listening at 8 kHz ignores its own 1 kHz and ducks on 8 kHz content). Spectral bands keep their own detection. Panel (plain compressor only): a source dropdown right of the dials (Own, Sidechain and the other bands that are on; the dropdown now shows a subset of its choices and shortens rows when a list would not fit). A below-threshold (expander) direction was built and removed again (2026-10-16): pulling the range up already makes a band rise when loud, and the expander added nothing worth its confusion. **Open:** transient/sustain focus. **UX (2026-10-15):** no "Dynamic" switch. A band becomes dynamic by pulling: the two ghost grips above/below a selected node (above = band rises, below = band dips), or the hidden ring around the panel's gain dial (right/left). Back to zero or a double-click makes it static. Nothing on the plot steps (2026-10-16): the band's compressor curve (threshold, ratio, range, transient amounts) eases towards its settings with a 20 ms time constant (`eq::CURVE_SMOOTH_MS`, shared with the spectral stage), so dragging the range glides the gain (tested: −6 → −12 dB in steps under 0.6 dB, where it jumped 6 dB before); and the editor follows the live dynamic and spectral gains and the detected level through two eased stages (12 ms each), so the curve moves at the frame rate even when the host's blocks are 20 ms apart, and its speed changes smoothly too (one stage restarted a ramp at every block). **The range is a depth (2026-10-16):** the compressor curve runs from 0 at the threshold (6 dB soft knee) to the whole range once the level is 6 dB past the threshold after the ratio (`eq::FULL_RANGE_DB`), eased into it at the top (no corner); before, the movement was overshoot × (1 − 1/ratio) capped at the range, so pulling the range past what the signal reached did nothing. The ratio now sets how quickly the band gets there (tested: every range from 3 to 24 dB reached, monotone, no corner). The spectral compressor uses the same curve. CPU for 8 dynamic bands in `bench_dyn` rose to 0.77 % because they now move their whole range. The range end is a draggable grip on the plot. Gain plus range is clamped to the gain dial's ±30 dB (UI and DSP, `GAIN_LIMIT_DB`), so the ring never goes past its ends. Two modes, picked by icon buttons beside the panel's scope. **Compressor** (threshold, attack, release, ratio): the scope plots movement against level on a fixed square-root scale (±30 dB at the edges), with a live dot, and works as an XY pad (sideways = threshold, vertical = range). **Transient shaper** (attack and sustain amounts, ±12 dB each, own parameters independent of the range): moves by shape whatever the level. The ring and plot grips are compressor controls; on a transient band they only show the live motion. A hit (fast envelope 3 dB ahead of a slow one) starts the attack phase, which lasts exactly the transient time. Through the attack phase the band follows the hit: the attack weight rises with the hit's sharpness (fast-over-slow lead, full at 12 dB, eased) and holds its peak, so sharp hits get the whole attack amount and soft ones a share (tested). Gain = attack·w + sustain·(1 − w) with w in 0…1, so it always lies between the two amounts (tested: no louder spot when both cut); around the mark the band crossfades to sustain along a logistic S curve (curl 8, rescaled to run exactly 1 → 0) whose width is the per-band Transition (0–200 % of the transient time, default 100 %, never under 1.5 ms; tested click-free; shaded on the scope; 0.25 ms into a new hit, tested click-free with the amounts 24 dB apart), and sustain is a flat shelf over everything until the next hit, held notes included. Gated below −80 dBFS. The scope runs a synthetic hit through the band's own `ShapeDetector` (shared with the DSP, eased smoothstep response) and draws it as it comes in and as it leaves, on a square-root time axis; dragging the attack part sets the attack amount, the tail sets sustain. A line marks where attack hands over to sustain: the per-band transient time (5–200 ms, default 25 ms), which scales every detector time except the 0.5 ms attacks. Drag the line sideways, or use the TIME dial. The analyzer threshold bar was tried and removed. No numeric "gain now" readout and no explanatory words in the controls. Attack (0.5–200 ms) and release (10 ms–2.5 s) show readable values and move continuously; their dials have their own feel (`DialCurve`, separate from host normalisation), with attack 10–50 ms and release 100–300 ms taking 45 % of the travel each. Analyzer: 1/6-octave raised-cosine smoothing across frequency (Catmull-Rom in dB where bins are sparser than the display, then a light dB rounding pass so dips stay round), eased rise (60 ms) and fall (300 ms), drawn as a Catmull-Rom curve. The panel docks away from the edited band and grows into the plot, so the core row never moves under the pointer.
2. Natural phase and linear phase (partitioned FFT convolution). **Linear Phase done (2026-10-15):** `mite-dsp::conv` (uniformly partitioned overlap-save convolution, f64, Shared/Diagonal/Matrix kernel layouts, raised-cosine kernel crossfade, allocation-free, non-finite input silenced) and `mite-dsp::linear_phase` (zero-phase analog magnitude per bin as a 2×2 L/R matrix per band, Blackman-windowed, exactly symmetric about len/2). `mite-core::handover` (worker → audio `Handover<T>` with audio-side retire; `triple_buffer` for settings snapshots). Engine: a designer thread per instance (polls every 2 ms), `mode`/`lp_res` global params, latency = block + len/2 reported in `initialize` and signalled on change; mode/resolution changes fade out 10 ms, wait muted for the new kernel and for its delay line to fill, then fade in. Dynamic bands in Linear Phase: static curve in the kernel, dynamic movement as a gain-only IIR after it (their detector sees the band's own static gain, unlike Zero Latency). Tests: pure-delay null (< 1e-7), impulse symmetric about the reported latency, magnitude matches Zero Latency within 0.05 dB, click-free switches both ways and kernel updates, allocation guard over 2400 blocks of switches and swaps. CPU (`examples/bench_lp`, 24 bands): 0.30 % at High, 1.06 % at Max with a mid/side band. **Natural Phase done (2026-10-15):** `mite-dsp::natural_phase` designs the correction analog·digital⁻¹ per bin as complex 2×2 matrices (easing to identity where the digital determinant falls from 1e-6 to 1e-8, a cut's stopband; entries capped at ±12 dB), centred on a 128-sample delay, Blackman-windowed to 256 taps; it runs after the zero-latency IIR chain in its own 64-sample-block convolver. Dynamic bands move in the IIR chain; the correction follows their static settings. Measured: within 0.1 dB and 2° of the analog magnitude and phase up to 20 kHz at 44.1 kHz (closes the §8.6 near-Nyquist gap in this mode); pure-delay null; click-free switches through all modes. CPU 0.69 % for 24 bands. **Open:** the dynamic part in Linear Phase is minimum phase (Pro-Q's approach), and its detector offset.
3. Spectral dynamics. **Done (2026-10-16):** a 1024-sample STFT (× the 48 kHz rate ratio; 21.3 ms at 48 kHz) runs after the EQ chain in every mode, only while some band is a spectral compressor. Turning it on or off changes the latency, so it goes through the mode-switch path: 10 ms fade out, muted while the STFT fills, fade in, latency signalled to the host. `mite-dsp::stft` (75 % overlap, sqrt-Hann WOLA, exact reconstruction, latency = size). `mite-dsp::spectral` (`SpectralBands`): a **spectral compressor** (a dynamic bell/shelf/tilt in compressor mode with `spectral` on) gives every point of the spectrum in its region its own envelope (the band's attack and release) and runs the band's compressor curve (ratio, range, knee) on it; its IIR keeps only the static gain. **Detection is the analyzer (2026-10-16):** the band hears the spectrum exactly as the plot draws it, an 8192-point Hann FFT of the stage's input (every second STFT frame) read at the plot's 301 log-spaced points by the same `mite_dsp::analyzer::LogSpectrum` the editor uses (1/6-octave raised cosine, 4.5 dB/oct tilt, dB rounding), so the scope and the main graph agree by construction, the low end included (tested within 0.5 dB of the analyzer's reading). Thresholds are therefore on the analyzer's scale (pink noise flat; a pure sine reads about 12 dB under its dBFS peak, as the plot shows it). The per-point gains are read onto the 1024-sample STFT's bins (the mean of the points a bin spans, linear between points where they are sparser), so the cut itself is as fine as the STFT allows: at the low end, bins 47 Hz apart; the scope shows that applied cut, not a finer one. **Threshold:** a fixed level (`thresh`, on the analyzer's scale), or **relative** (`srel`, `rthresh` 0–24 dB, default 6): each point's envelope against its surroundings, the envelopes averaged in dB over 0.6 octave around it (envelopes on both sides, so the attack/release bias cancels; plain noise moves 0.00 dB on average, tested). Relative catches what sticks out (a resonance, a ringing note) however loud the whole signal is, and leaves loud even material alone where a fixed threshold squashes it (tested). **Smoothing** (`ssmooth`, 0–100 %, default 25 %): each point's gain averaged over up to half an octave (fractional window edges, so the dial is continuous), so neighbours move together (tested). Gains are weighted by the band's region (`spectral::weight`: the power of a band-pass at Q for bells, of a low/high-pass for shelves, all bins for tilt; cut off 40 dB down and rescaled to reach 0 continuously). Power, not amplitude (2026-10-16): that is how a bell's own gain spreads over frequency; the amplitude skirt fell only 6 dB/oct, so a Q 2 bell at 8 kHz still trimmed a little down to a few hundred hertz. Stereo/Left/Right bands add up per channel in dB; Mid/Side bands combine as 2×2 matrices after them. Applying a band's gains touches only the bins its region reaches. A separate Resonance band type (depth/sharpness/selectivity/speed) was built first and then merged into this relative mode (2026-10-16): one mode, both behaviours. Tests: neutral spectral stage nulls against the delayed input in all three modes (< 1e-6) with latency = phase-mode latency + STFT size; a ringing tone in noise is cut > 6 dB through the engine; the fixed compressor moves only the loud bins; adding and removing spectral bands is click-free in stereo and mono; allocation guard over spectral on/off switches in every mode. CPU (`examples/bench_spectral`, analyzer detection): 0.60 % for one relative band (the detector's FFTs), 1.14 % for 8, 2.40 % for 24; 1.92 % for 24 fixed-threshold bands. **UX:** the dynamics modes are three stacked icon buttons beside the scope: compressor, spectral compressor (the compressor's bend over a row of bins), transient shaper. The spectral compressor has its own **spectral scope**: the band's frequencies (three octaves either side, log) against level (−60…0 dB, the analyzer's scale), with each point's detected level (faint), the level after the band (lit), the change between them filled, and the threshold. That threshold is a line at a fixed level, or a curve riding the surroundings in relative mode, drawn from the engine's own per-point thresholds (`EqShared::focus_*`, the selected band only, at the analyzer's 301 points). The scope draws the detector's points directly, eased with the analyzer's 60 ms rise and 300 ms fall like the main spectrum; the cut it shows is what the STFT bins apply, read back through a monotone cubic (smooth, no overshoot, tested). The curve's settings (threshold, relative threshold, ratio, range) ease towards their values with a 20 ms time constant per frame, so dragging the threshold glides the cut instead of stepping (tested). Drag it up or down to set the threshold. A toggle under the remove-dynamics button switches fixed/relative (icon: a line riding the tops of the bins), and the THRESHOLD dial follows (dBFS or dB over the surroundings). The row has a fifth dial, SMOOTHING. On the plot, the total curve includes the live per-bin gain (`EqShared::spectral`, the 301 analyzer points, Catmull-Rom between bins held between neighbours so it never overshoots), and the area between the curve without it and with it is lit. The total curve is drawn as a monotone cubic through its samples (no corners, no overshoot). UI: 120 fps at 2× DPI in the standalone preview with a spectral band cutting live, 0.62 ms mean frame work. **Open:** a finer cut at the low end (a longer or multi-resolution STFT, at a latency cost), and a detection heat map.
4. Output stage and solo. **Done (2026-10-16):** `mite-dsp::output` and the engine's output stage, in this order: auto gain, mix and delta against the dry signal, solo, output gain and balance, bypass, then the mode-switch dip over all of it. The dry signal is delayed by exactly the running latency (`DryDelay`, sized in `prepare` for the longest mode plus the STFT), so mix, delta and bypass stay phase-aligned in every mode: mix 0 % and bypass null bit-exactly against the delayed input in all three modes, delta plus the dry signal equals the wet signal (< 1e-6), and a neutral EQ's delta is silent (all tested). Every switch fades (mix and balance 20 ms; bypass, delta and solo 10 ms; tested click-free through all of them). **Auto gain** reads the static curve, not the signal, so it never pumps: the loudness change of pink noise through the curve, K-weighted (BS.1770 high pass near 38 Hz, +4 dB shelf above 1.5 kHz), over 48 points from 20 Hz to 20 kHz; bands on one channel or on mid or side count for the half of the power they reach. Worked out at most every 1024 samples while the curve moves, glides over 50 ms, limited to ±24 dB. Tested on pink noise, measured K-weighted as it is defined: a broad +6 dB bell at 2 kHz raises the loudness 3.3 dB without it and moves it 0.09 dB with it (within 0.5 dB asserted). **Balance** (`out_pan`): unity in the centre, the far side fades along a quarter sine; ignored for mono. **Solo** (`bNN_solo`, one band at a time): the output crossfades to a filter over the band's region (band pass at its frequency and Q for bells, notches and band passes; 12 dB/oct below or above a shelf's corner; what a cut removes, at its slope) of the part of the stereo image the band processes (left, right, mid or side alone; tested: a side band soloed on identical channels is silent). Tilt has no region and cannot solo. UI: a headphones button right of the stereo dropdown; the soloed node wears a white ring and the other bands fade to 35 %; the solo ends when the panel closes (it is an audition, not a mode). Footer: three icon toggles (auto gain: a boost brought back to the line; delta: Δ; bypass: power), then MIX, PAN and OUTPUT dials with name and value beside each. The band panel's remove button is a bin (the power glyph is only the plugin's bypass). The help strip draws its values in fixed slots as wide as their widest value (tabular digits), so moving the pointer changes the numbers without moving the words after them. The empty-state guide in the middle of the plot is gone (the help strip still says how to add a band). Every icon button shares one look (`icon_button`): hovered, its border lights in the band's (or the accent) colour and the glyph brightens; on, it is also tinted and the glyph takes the colour; the panel and footer dropdowns light their border on hover too. Long host blocks run in 512-sample chunks. The analyzer's pre tap is fed from the same delayed dry signal (2026-10-16): before, it heard the input at once while the post tap heard the output a latency later, so with any latency (spectral stage, Natural or Linear Phase) the two spectra showed different moments and parted everywhere on changing material (tested: below a spectral bell at 8 kHz they now agree within 0.2 dB on bursty noise; they differed by 5.7 dB). CPU (`examples/bench_dyn`): 8 static bands 0.28 %, with the output stage busy (mix, delta, auto gain, balance, solo) 0.34 %; 8 dynamic bands 0.68 % (0.87–0.89 % measured later on the same machine under other load, before and after the sidechain change alike). Editor frame build 0.49 ms mean. Found on the way: a band's gain, frequency or Q glide that ended between two coefficient updates kept the coefficients from up to 16 samples before its end (a 6 dB move landed about 0.1 dB short); it now redesigns when the glide settles (tested). **Open:** `character` (needs oversampled saturation) and `morph` (needs snapshots, slice 6).
5. Spectrum grab. **Done (2026-10-16):** interaction.md's "Spectrum peak" column. A peak is a local maximum of the post spectrum (as drawn) standing 3 dB above the lowest point a third of an octave either side, found within 18 px of the pointer and placed between grid points by a parabola through its neighbours (its level read from the drawn line); nodes and panel controls win over it. Hover: a ring on the peak that glides with the moving spectrum and fades in and out (50 ms), and the help strip names it (frequency, note, level in fixed slots); no tooltip, the strip is the tooltip. **Only under the spectrum:** the spectrum answers the pointer (peak hold, markers, grabbing) only inside its filled area, below the line as drawn (the held line while holding; 4 px of give), never above it, where the plot belongs to the curve and adding bands (tested both ways). Peak levels and that test use the drawn line itself (the same Catmull-Rom spline), so a marker sits on the line and a tone's tip between grid points counts. **Peak hold (as Pro-Q):** once the pointer has rested under the spectrum for 300 ms (not on a node, a control or the panel), the spectrum's maxima accumulate into a held curve, drawn faintly with a dot on each of its peaks and faded in and out (80 ms); peaks are then found on the held curve, so they stay put and can be grabbed after the sound has moved on (tested: a tone that stopped is still grabbed at its frequency); leaving the plot or moving onto a node lets it go. Drag: a ghost Bell (Q 3, the new band's colour) stays on the peak's frequency (sideways movement is ignored) while up and down set its gain from 0 dB, Shift fine; release adds it in the first free band and selects it; Esc drops it; a click without a drag clears the selection as on empty display. Double-click: a Bell at the peak, Q 3, −3 dB. `mite-ui` gained key events (`UiEvent::KeyDown`, Esc, Delete/Backspace, characters): a key the view does not use goes on to the host, so transport shortcuts keep working; Esc also closes an open menu. Tested through the editor with a real tone in the analyzer taps (drag, Esc, double-click). Snapshot gained `SNAPSHOT_DRAG` (a drag in progress) and ticks after `SNAPSHOT_HOVER` so fading hover states show.
6. Collision view and EQ Match. **EQ Match and sidechain masking done (2026-10-21):** `mite-dsp::matching` (long-term power-averaged spectra at the analyzer's points; a greedy fitter that puts each band, a bell or a shelf at the ends, where the residual is worst, then refines every band together by coordinate descent over frequency, gain and Q against their analog responses; masking scores; carve suggestions). The plugin now publishes the sidechain spectrum (`EqShared::sidechain`, silence while unconnected), drawn in `sidechain` colour while it carries signal. **Match:** a header button opens a panel in the band panel's place; it learns this track before the EQ and the reference on the sidechain while both play (pauses don't dilute the averages; a ring fills over 10 s of learned audio; pause and start over). The aim is the reference over the track as the current bands already shape it, smoothed (Smooth, 1/12–1 oct), without its mean (tone, not level), scaled by Amount, and silent past any cut's corner, so the match never undoes a cut. The fit runs on a worker thread (11 ms for 4 bands, 111 ms for 24, `mite-dsp/examples/bench_match`; rms error 0.57 dB at 4 bands, 0.16 dB at 12) and refreshes every 0.5 s while learning or when a dial moves. The plot shows the aim (sidechain colour, only where the match listens), the EQ with the fitted bands as a ghost curve, and a dashed ring per band to add. The tick adds them in free slots as one undo step. Tested: a darker reference yields bands that are more than 10 dB down at 8 kHz against 200 Hz, and one undo removes them. **Masking:** 3 s averages of this track (post) and the sidechain; a point collides where both *peak*, each standing at least 3 dB above its own 3-octave neighbourhood (and that baseline never more than 6 dB under its median), hysteresis down to 1.5 dB, at least 1/6 octave wide. Two broadly bright spectra don't collide. Collisions draw as the token's 45° hatch with 1.5 px edges and fade in and out. Hovering one shows its carve, a bell at the score's centre of mass, cutting half the depth (1.5–6 dB), as wide as the region, as a ghost and a dashed ring. Pressing the ring adds the band, already in hand to drag; right-click ignores that range. Words stay in the help strip. **Other instances and reference files done (2026-10-21):** `mite-core::Registry` lists a plugin's live instances in the process (weak entries, numbered from 1 with the lowest free number; locked, so UI threads and construction only). Every Mite EQ registers itself (`mite_eq::INSTANCES`), and its editor compares its track with the sidechain and with each other instance's post spectrum at its sample rate, analysed every 4th frame, each with its own 3 s average. Collisions remember whom they are with ("Masking with EQ 2"); hovering one draws that instance's average. Hosts that sandbox plugins in separate processes see no peers. **Carve on the other instance:** `mite_plugin::create_editor_with` hands the plugin a `SharedParamHost` over its GUI context each time its window opens. It stays valid while the instance lives, window open or not, and is stored in `EqShared::remote`. A second dashed ring with an arrow, in the other's colour, sits beside the carve ring and writes the same cut into the other instance's first free band, as gestures. nih-plug gives out the GUI context only with a window, so an instance whose editor never opened can't be written: its ring is dim and the help strip says to open it once. Instances are named by number; hosts don't tell the plugin its track name through nih-plug. **Reference file:** the Match panel's page button opens a native dialog (`mite_ui::dialog`, rfd; a worker thread, except macOS, which needs its panels on the main thread). `mite_eq::reference` decodes the file with symphonia (WAV, AIFF, FLAC, MP3, AAC/ALAC in MP4, Ogg Vorbis), sums it to mono and streams it into `mite_dsp::matching::RecordingSpectrum` (analyzer frames, half overlapping, read as the plot draws them, silent frames skipped). With a file, Match learns only this track; the file's name sits by the title, with a × back to the sidechain. Tested: two registered instances find their shared 100 Hz peak and the arrow ring writes the cut into the other's band 1 only; a darker WAV yields bands more than 10 dB down at 8 kHz against 200 Hz without any sidechain. **Open:** track names for instances (CLAP track-info through the nih-plug fork); dropping a file onto the editor.
7. Snapshots and presets. **Snapshots done (2026-10-16):** four snapshots A–D (now `mite_core::snapshots` and `mite_ui::session`, shared by every plugin). One is active: the band parameters are its contents, so editing always edits it. Clicking a letter stores the parameters in the active slot and recalls the clicked one (only parameters that differ are written, each as a gesture, so the band smoothers glide and host undo sees them); a slot never stored starts as a copy. Dragging a letter onto another copies it. Stored slots are the band parameters' normalized values by id (solo excluded), saved in the plugin state under `snapshots` (`serialize_fields`, with the active slot taken from the parameters; tested round trip through `EqParams`). The audio thread only plays the parameters. The letters A–D sit in the header (interaction.md's order), the edited one lit, empty ones dim. **Morph between snapshots was built and removed again (2026-10-16): overkill;** no `morph` parameter. **Presets done (2026-10-16)** (now `mite_core::presets` and `mite_ui::session`; the EQ's `presets.rs` keeps its sound ids and factory files): a preset is the sound parameters as plain values by id (bypass, delta and solo are monitoring and never part of one; the snapshots neither: a preset loads into the snapshot being edited); what a preset leaves out is at its default, so factory presets (`presets/*.json`, built in: Init, vocal presence, kick, mix bus, de-mud, de-ess, resonance tamer, telephone; tested to parse and to name only real parameters) list only what they change, and user presets save only what differs from the defaults. User presets are files in the OS settings folder (`~/.config/MiteFX/Mite EQ/Presets`, `~/Library/Application Support/…`, `%APPDATA%\…`), one JSON file per preset (`plugin`, `version`, `name`, `params`), named after the preset (characters file systems refuse become `_`); a user preset shadows a factory one of the same name. Header: ‹ name › (the arrows step through factory then user presets, wrapping); clicking the name opens the list (Save as…, FACTORY, YOURS; the current one lit; the wheel scrolls past 18 rows). Save as… turns into a name field (a suggestion typed over; Enter or the tick saves, Esc leaves it); a dot after the name shows the sound has moved on from the preset. The current preset's name is saved with the state (`preset`). Loading only writes parameters that differ, as gestures. **UI undo/redo done (2026-10-16):** the editor's own edits only; automation, the host's undo and state loads never become steps (the editor's writes pass through a recording host; changes from elsewhere just move the history's base, tested). A whole drag is one step; loading a preset or switching a snapshot is one step; each step keeps the sound parameters, the snapshots and the preset name; 100 steps. Header buttons (dim with nothing to go to) and Ctrl/Cmd-Z, Shift-Ctrl/Cmd-Z, Ctrl-Y (`mite-ui` gained Backspace and Enter keys for the name field). **Open:** the full preset browser (tags, search, curve thumbnails, audition on hover).
8. Stereo placement (M/S/L/R). **Done (2026-10-14).**
9. WGSL bloom and glass blur. **Done (2026-10-22):** `mite-ui::effects` (`effects.wgsl`) composites around Vello. **Glow:** `glow_stroke`/`glow_fill` draw the shape into the scene and into the frame's glow layer (`DrawCx::glow`), which is blurred by two Gaussians (σ 2 px at half size and 6 px at quarter size, in linear light, f16) and laid over the layer: screen-blended in the dark themes, "over" in the light one; the stacked translucent strokes are gone. A call's `sigma` sets its glow's strength (the source is the shape itself, as an SVG glow blurs the graphic). Off in high contrast. **Glass:** `DrawCx::glass(shape, blur)` (via `paint::glass_panel`) closes the current layer; everything under it is blurred (σ from `blur.glassPanel`/`blur.menu`, 18 px, at quarter size), saturated 1.2× (`blur.saturate`) inside the rounded rect, then the panel's own tint (`glass`, 72 % in dark) and what follows are drawn over it, so the band panel, Match panel, dropdowns and the preset list frost what lies under them, stacked (a menu over the panel frosts both). Each layer's glow is composited before the next layer, so a panel covers the curve's glow. Each layer is one Vello render (its glow drawn at half size in a strip under the frame); a frame with one layer and nothing glowing is a single render as before. Tests (`mite-ui/tests/effects.rs`, on rendered pixels): the halo falls off and is gone 40 px away, the line stays white; glass blurs a sharp edge only under it and leaves what is drawn after it crisp. Measured at 2560×1520 (2× DPI) on an RX 7900 GRE, waiting for the GPU each frame (`MITE_UI_GPU_TIME=1`): 1.58 ms per frame with bloom (1.38 ms before), 2.41 ms with the band panel open (1.42 ms); live preview at 120 fps steady with the panel open. `PREVIEW_CLICK` clicks in the preview (e.g. a node, to open its panel).

Ordered steps:
1. `mite-dsp` filters (SVF, matched designs, cascades), the FFT wrapper, partitioned convolution and oversampling.
2. `mite-core` param, state, telemetry and undo model.
3. `mite-ui` tokens from Claude Design, the widget kit, spectrum and curve rendering.
4. EQ static bands, then dynamic, spectral, linear phase, analyzer extras, Match, collision, presets.

**Exit criteria:** everything in §8.6.

**MVP review (2026-10-22):** feature-complete for the MVP. Before plugin 2 the shared parts were taken out of the EQ (see §3, "What a new plugin reuses"), state got its version and golden tests, and CI validates the EQ. Measured locally the same day: clap-validator 34 passed, 0 failed (10 not applicable); pluginval strictness 10 passes with and without its GUI tests. Still open before a public release: hosts other than Reaper on Linux, the §10 gates without EQ tests yet (ramp test over every parameter, block-size invariance, 44.1–384 kHz, NaN/Inf recovery through `reset()`, the CPU-fallback render), and the GUI frame-time target. Deferred past the MVP: `character`, the full preset browser, the piano-roll strip, transient/sustain focus.

### Phase 2: Dynamics core

Mite Comp, Mite Limit and Mite Multiband. These build the dynamics cell and the loudness engine.

**Status (2026-10-22):** Mite Comp's first version is built (§6.2). Next: oversampling (shared) and Character saturation, then Mite Limit.

### Phase 3: Space and colour

Mite Verb, Mite Delay and Mite Sat, plus the modulation matrix.

### Phase 4: The rest

DeEss, Gate, Shape, Soothe, Filter, Mod, Meter, Utility and Amp.

### Phase 5: Special cases

AAX, AUv3/iPad, standalone, LV2 and surround layouts.

---

## 10. Quality gates (every plugin, every pull request)

- [ ] DSP unit tests, including frequency, phase and group-delay measurements against analytic references.
- [ ] **Null tests:** bypass, mix 0 % and neutral settings null to −140 dBFS after latency compensation.
- [ ] **Aliasing and THD+N measurements** with sine sweeps at every oversampling setting, stored as reports in `docs/dsp/`.
- [ ] **Ramp tests:** automating every continuous parameter at its full range in one block produces no step discontinuity above the threshold.
- [ ] NaN/Inf injection recovers via `reset()` without allocating.
- [ ] Block sizes 1, 7, 64, 128, 4096 and 8192 plus random variable blocks give identical output (where the DSP is block-invariant).
- [ ] Sample rates 44.1k–384k work.
- [ ] Parameter golden file is unchanged, or the change is an explicit, versioned migration.
- [ ] State golden blobs from every released version load correctly.
- [ ] `clap-validator`, `pluginval` and `auval` pass.
- [ ] `criterion` CPU benchmarks show no regression greater than 10 %.
- [ ] UI renders a frame with the GPU and with the CPU fallback (snapshot images in CI).
