# MiteFX — Blueprint

> The backbone document for MiteFX. Every design and implementation decision either
> follows this file or changes this file first. Audience: Claude Design (visual
> system), implementation sessions, and future contributors.

**Status:** Phase 0 (not started) · design first pass in · **License:** GPL-3.0-or-later · **Last revised:** 2026-10-14

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
6. **State is versioned and migrated.** Never reinterpret old state silently. Every version has a golden blob test.
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
- Per-band **resonance suppression** (soothe-style) as a band type.
- **Phase-rotation / allpass** bands.
- **Transient-aware dynamic bands**: the band can respond to attacks only or to sustain only.
- **EQ morph** between snapshots A, B, C and D (automatable).
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
- Shares its engine with the EQ's resonance band.

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
- **Resonance engine:** EQ, Soothe and DeEss.
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
  - Low Shelf / High Shelf (with variants: standard, Baxandall, vintage-style overshoot)
  - Low Cut / High Cut
  - Notch
  - Band Pass
  - Tilt Shelf
  - Flat Tilt
  - **All-Pass / Phase Rotate** (beyond)
  - **Resonance** (beyond: soothe-like suppression limited to the band's range)
- **Slopes (cuts and shelves):** 6, 12, 18, 24, 30, 36, 48, 72 and 96 dB/oct, plus **Brickwall** (linear-phase modes only).

**Dynamic EQ (per band):**
- Parameters: `dyn_range` (±30 dB), `threshold` (or auto), `attack`, `release`, `ratio`/knee.
- Detection: band-internal, external sidechain, or another band.
- Direction: above or below the threshold.
- **Transient/sustain focus** (beyond).

**Spectral dynamics (per band):** works per FFT bin inside the band, for surgical, dynamic resonance taming.

**Character (global):** Clean, Subtle and Warm. The coloured modes add oversampled saturation.

### 8.2 Processing modes

| Mode | Latency | Implementation |
|---|---|---|
| **Zero Latency** | 0 | ZDF SVF / matched-magnitude IIR, no cramping |
| **Natural Phase** | 4.0 ms at 48 kHz (192 samples, scaled with the sample rate) | Matched IIR sections plus a 256-tap correction FIR (analog ÷ digital, complex, per 2×2 L/R matrix), so magnitude and phase follow the analog prototypes up to Nyquist |
| **Linear Phase** (Low / Medium / High / Max) | 13.3, 24.0, 45.3 or 88.0 ms at 48 kHz (1024–8192-tap FIR, scaled with the sample rate, plus one 128-sample block) | Uniformly partitioned FFT convolution (f64, 2×2 kernel matrix for L/R/M/S); kernel designed on a worker thread from the analog magnitude; crossfaded swap |

Mode switches are click-free. Dynamic bands in linear-phase mode use the Pro-Q approach: linear-phase static filtering, with dynamic gain applied through parallel filtering, latency-aligned.

### 8.3 Analyzer

- Pre, post and sidechain spectra. FFT size is 1k–32k with selectable range, speed (ballistics) and tilt (0, 3 or 4.5 dB/oct).
- **Spectrum grab:** hover the spectrum to see peaks; drag a peak to create a band there.
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
| `gain_scale` | −200…200 %; scales every band's gain |
| `delta` | Delta listen |
| `bypass` | bypass |
| `morph` | A/B/C/D morph position |

**Per-band ID scheme:** `b{NN}_{param}` (for example `b03_freq`, `b03_dyn_range`). All 24 bands are pre-declared, because hosts need static parameter lists. IDs are frozen once released (rule 5.5).

### 8.5 Performance targets (48 kHz, 128-sample buffer, one modern x64 core)

| Configuration | Target |
|---|---|
| 8 static bands, Zero Latency | < 0.5 % CPU |
| 8 dynamic bands, Zero Latency | < 1.5 % CPU |
| 24 bands, Linear Phase High | < 3 % CPU |
| GUI open at 120 fps, 4k-wide spectrum | < 1 ms GPU frame time, < 3 % CPU |

### 8.6 Acceptance

- Measured magnitude matches the analog prototype within 0.1 dB up to 20 kHz at 44.1 kHz (Zero Latency).
- Linear-phase group delay is flat within 0.01 samples.
- Morph and automation of every parameter are click-free (ramp tests).
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

Written but not yet run:
- The CI workflow for Windows, macOS and Linux, with no validator exclusions.

Open:
- AU via clap-wrapper.
- Bitwig and Ableton, and macOS and Windows hosts.
- The WGSL bloom pass.
- Exact log parameter ranges.
- Offering the two nih-plug fixes upstream.

### Phase 1: Mite EQ (MVP)

**Status (2026-10-14), slice 1 done:**
- **Filters and engine:**
  - Zero-latency matched filters (`mite-dsp::filter`, `mite-dsp::eq`): Bell, Low/High Shelf, Low/High Cut at 6–96 dB/oct, Notch, Band Pass and Tilt.
  - Cuts are designed as inverted boosts, so boost and cut are exact mirror images.
  - 24 bands with click-free smoothing; type, slope and on/off changes crossfade; disabled bands are bit-exact.
- **Analyzer:** `realfft`, 1/12-octave smoothing, 4.5 dB/oct tilt.
- **Parameters:** 145 frozen IDs (`b01_freq` …) checked against a golden manifest.
- **Editor**, per the mockup:
  - spectrum, curves and nodes;
  - glass band panel;
  - add, drag, Shift-fine, Alt or wheel for Q, reset and delete.
- **Validation:** clap-validator and pluginval (strictness 10) pass, debug and release. Draws in Reaper.

**Measured accuracy** of the analog match:
- Bells up to the mid range: within 0.01–0.1 dB.
- Cuts: exact slopes.
- Bands near Nyquist (above about 4 kHz at 44.1 kHz): within about 1 dB. **Open:** the §8.6 target of 0.1 dB up to 20 kHz still needs oversampling or a higher-order correction near Nyquist.

**Next slices:**
1. Dynamic EQ. **Done (2026-10-14):** band-internal detection, above-threshold, range/threshold/attack/release/ratio per band; 8 dynamic bands measured at 0.62 % of a core worst case (`examples/bench_dyn`). **Open:** external sidechain and other-band detection, below-threshold direction, transient/sustain focus. **UX (2026-10-15):** no "Dynamic" switch. A band becomes dynamic by pulling: the two ghost grips above/below a selected node (above = band rises, below = band dips), or the hidden ring around the panel's gain dial (right/left). Back to zero or a double-click makes it static. The range end is a draggable grip on the plot. Gain plus range is clamped to the gain dial's ±30 dB (UI and DSP, `GAIN_LIMIT_DB`), so the ring never goes past its ends. Two modes, picked by icon buttons beside the panel's scope. **Compressor** (threshold, attack, release, ratio): the scope plots movement against level on a fixed square-root scale (±30 dB at the edges), with a live dot, and works as an XY pad (sideways = threshold, vertical = range). **Transient shaper** (attack and sustain amounts, ±12 dB each, own parameters independent of the range): moves by shape whatever the level. The ring and plot grips are compressor controls; on a transient band they only show the live motion. A hit (fast envelope 3 dB ahead of a slow one) starts the attack phase, which lasts exactly the transient time. Through the attack phase the band follows the hit: the attack weight rises with the hit's sharpness (fast-over-slow lead, full at 12 dB, eased) and holds its peak, so sharp hits get the whole attack amount and soft ones a share (tested). Gain = attack·w + sustain·(1 − w) with w in 0…1, so it always lies between the two amounts (tested: no louder spot when both cut); around the mark the band crossfades to sustain along a logistic S curve (curl 8, rescaled to run exactly 1 → 0) whose width is the per-band Transition (0–200 % of the transient time, default 100 %, never under 1.5 ms; tested click-free; shaded on the scope; 0.25 ms into a new hit, tested click-free with the amounts 24 dB apart), and sustain is a flat shelf over everything until the next hit, held notes included. Gated below −80 dBFS. The scope runs a synthetic hit through the band's own `ShapeDetector` (shared with the DSP, eased smoothstep response) and draws it as it comes in and as it leaves, on a square-root time axis; dragging the attack part sets the attack amount, the tail sets sustain. A line marks where attack hands over to sustain: the per-band transient time (5–200 ms, default 25 ms), which scales every detector time except the 0.5 ms attacks. Drag the line sideways, or use the TIME dial. The analyzer threshold bar was tried and removed. No numeric "gain now" readout and no explanatory words in the controls. Attack (0.5–200 ms) and release (10 ms–2.5 s) show readable values and move continuously; their dials have their own feel (`DialCurve`, separate from host normalisation), with attack 10–50 ms and release 100–300 ms taking 45 % of the travel each. Analyzer: 1/6-octave raised-cosine smoothing across frequency (Catmull-Rom in dB where bins are sparser than the display, then a light dB rounding pass so dips stay round), eased rise (60 ms) and fall (300 ms), drawn as a Catmull-Rom curve. The panel docks away from the edited band and grows into the plot, so the core row never moves under the pointer.
2. Natural phase and linear phase (partitioned FFT convolution). **Linear Phase done (2026-10-15):** `mite-dsp::conv` (uniformly partitioned overlap-save convolution, f64, Shared/Diagonal/Matrix kernel layouts, raised-cosine kernel crossfade, allocation-free, non-finite input silenced) and `mite-dsp::linear_phase` (zero-phase analog magnitude per bin as a 2×2 L/R matrix per band, Blackman-windowed, exactly symmetric about len/2). `mite-core::handover` (worker → audio `Handover<T>` with audio-side retire; `triple_buffer` for settings snapshots). Engine: a designer thread per instance (polls every 2 ms), `mode`/`lp_res` global params, latency = block + len/2 reported in `initialize` and signalled on change; mode/resolution changes fade out 10 ms, wait muted for the new kernel and for its delay line to fill, then fade in. Dynamic bands in Linear Phase: static curve in the kernel, dynamic movement as a gain-only IIR after it (their detector sees the band's own static gain, unlike Zero Latency). Tests: pure-delay null (< 1e-7), impulse symmetric about the reported latency, magnitude matches Zero Latency within 0.05 dB, click-free switches both ways and kernel updates, allocation guard over 2400 blocks of switches and swaps. CPU (`examples/bench_lp`, 24 bands): 0.30 % at High, 1.06 % at Max with a mid/side band. **Natural Phase done (2026-10-15):** `mite-dsp::natural_phase` designs the correction analog·digital⁻¹ per bin as complex 2×2 matrices (easing to identity where the digital determinant falls from 1e-6 to 1e-8, a cut's stopband; entries capped at ±12 dB), centred on a 128-sample delay, Blackman-windowed to 256 taps; it runs after the zero-latency IIR chain in its own 64-sample-block convolver. Dynamic bands move in the IIR chain; the correction follows their static settings. Measured: within 0.1 dB and 2° of the analog magnitude and phase up to 20 kHz at 44.1 kHz (closes the §8.6 near-Nyquist gap in this mode); pure-delay null; click-free switches through all modes. CPU 0.69 % for 24 bands. **Open:** the dynamic part in Linear Phase is minimum phase (Pro-Q's approach), and its detector offset.
3. Spectral and resonance bands. **In progress (2026-10-15):** decided: 1024-sample STFT (~21 ms at 48 kHz, only while a spectral/resonance band exists), spectral dynamics as a toggle in compressor mode, Resonance band type with Depth/Sharpness/Selectivity/Speed. Done and tested: `mite-dsp::stft` (75 % overlap, sqrt-Hann WOLA, exact reconstruction, latency = size), `mite-dsp::resonance` (`SpectralBands`: spectral compressor per bin, resonance cut vs a dB spectral envelope, region weights, 2×2 L/R matrices, telemetry), band settings and params (`spectral`, `rdepth`, `rsharp`, `rselect`, `rspeed`, type `Resonance`). **Next:** run the spectral stage in `EqEngine` (latency through the switch path), Resonance panel dials and Spectral toggle, live per-bin reduction on the plot.
4. Collision view and EQ Match.
5. Snapshots, morph and presets.
6. Stereo placement (M/S/L/R). **Done (2026-10-14).**
7. WGSL bloom and glass blur.

Ordered steps:
1. `mite-dsp` filters (SVF, matched designs, cascades), the FFT wrapper, partitioned convolution and oversampling.
2. `mite-core` param, state, telemetry and undo model.
3. `mite-ui` tokens from Claude Design, the widget kit, spectrum and curve rendering.
4. EQ static bands, then dynamic, spectral, linear phase, analyzer extras, Match, collision, presets.

**Exit criteria:** everything in §8.6.

### Phase 2: Dynamics core

Mite Comp, Mite Limit and Mite Multiband. These build the dynamics cell and the loudness engine.

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
