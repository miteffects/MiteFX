# Mite Sat: design and build spec (in-house, 2026-10-26)

**From:** MiteFX engineering, designed here rather than in a Claude Design pass (user decision,
2026-10-26). This file is both the design and the build spec; `interaction.md` "Mite Sat" holds
the gestures in the shared table format.

Rival: FabFilter Saturn 2. The bar is the suite's: better looking than FabFilter, every pixel
computed from the DSP, direct manipulation, no words in the display.

---

## 1. What it is

A saturation and distortion plugin. It splits the signal into up to **6 bands** (5 crossovers,
LR4 with allpass compensation, shared with Shape: `mite_dsp::crossover`), and each band has its
own **model**, **drive**, **bias**, **tone**, mix, output, dynamics and stereo mode. Every model
runs oversampled (1/2/4/8×, linear-phase half-bands, ≥ 100 dB image and alias rejection).
Modulation (two LFOs, an envelope follower, 8 slots) can move any band control.

Neutral settings null: drive 0 dB, band bypass, band mix 0 or global mix 0 are the input
(delayed by the reported latency) bit for bit.

### 1.1 The band signal path (what the visuals draw)

```
u      = g·x + b            g = drive (0…36 dB), b = bias (operating point)
F(x)   = f(u) − f(b)        so silence stays silence
wet    = Tone( down( F( up(x) ) ) )
y      = x_d + a·HP₅(wet − x_d)      a = smoothstep(drive / 6 dB) · engaged · band mix
out    = y · auto gain · band output
```

`x_d` is the band delayed by the oversampler's round trip, so `a = 0` is the band exactly.
The 5 Hz high-pass acts on the difference only: it removes the DC a bias makes and leaves the
band's own lows alone. Drive is input gain into the curve (louder, as hardware); auto gain (on
by default) brings the level back.

The editor's transfer curve is `x + a·(F(x) − x)` times the auto gain, from the same functions.

### 1.2 Models (frozen order; append only)

| # | Model | Curve `f(u)` (`f(0) = 0`, `f'(0) = 1` but Rectify's 0, smooth: no corners) | Character |
|---|---|---|---|
| 0 | Tape | Jiles–Atherton hysteresis (slice 3) | Soft, loose, HF saturates first |
| 1 | **Tube** (default) | `u > 0`: `0.8·tanh(u/0.8)`; `u < 0`: `−1.25·(1 − e^{u/1.25})`; bias drifts down with level (blocking) | Asymmetric: even harmonics, more as it is pushed |
| 2 | Transformer | Integrator → JA core → differentiator (slice 3) | Low end saturates first |
| 3 | Diode | `0.3·asinh(u/0.3)` | Hard knee, log growth, never a ceiling |
| 4 | Soft | `u − 4u³/27` to ±1.5, then ±1 | Classic cubic: odd harmonics, gentle |
| 5 | Hard | `u` to 0.95, a quadratic knee to 1 at 1.05, then ±1 | Clipping, rounded so it never folds a corner into the spectrum |
| 6 | Fold | `sin(u)` | Wavefolder: past ±π/2 it folds back |
| 7 | Rectify | `tanh(√(u² + 0.05²) − 0.05)` | Full-wave: the fundamental gives way to the octave |

**No ADAA.** First-order antiderivative anti-aliasing was planned and dropped (2026-10-26).
On the curve's nonlinear part alone, it leaks `g·Δu/2` of high-frequency energy at high drive.
On the whole curve, it delays the wet by half a sample against the dry, so mix and drive below
6 dB would comb. Every curve is instead smooth (C¹), and the oversampler does the
anti-aliasing. Aliasing is measured per factor (`docs/dsp/sat-aliasing.md`).

---

## 2. Colour

- **Accent:** `sat` = **crimson**: dark `#F0284A`, light `#A00E38`, hc `#FF2D55`, cb `#238EA9`.
  It is 17 ΔE from Comp's ember. It sits on `danger`/`masking` (2.9 in dark), so **Sat draws no
  danger or masking role**: overs and clipping are `warn`, and a delete button is text plus a
  cross glyph (tokens.json `rules.satRed`).
- Bands are frequency regions: every band is drawn in the accent with its number, never in the
  ramp (`rules.frequencyBands`, as Shape).
- **Added harmonics:** where the output spectrum stands above the input spectrum, the gap is
  filled in the accent (20 % fill, lit edge, glow), so the plot shows where the saturation adds
  energy.

---

## 3. Main view (1280 × 760)

Shape's frame: header 56, display 620, help strip 28, footer 56. The meter column (IN, OUT) is at
x 1180–1280, as in Shape.

```
┌ header: mark · Mite Sat · ‹ preset › · undo redo · A B C D ─────────────────┐
│ spectrum (x 14–1166, y +12…+236): input dim, output lit,   │ IN  OUT        │
│ added harmonics in crimson; band regions tint the plot      │ meters         │
│ rail (36 px; 64 px with bands): grips, ghosts, badges       │                │
│ funnels                                                     │                │
│ ┌ card (wide when selected) ───────────────────────────┐    │                │
│ │ model glyphs  ○ ○ ● ○ ○ ○ ○ ○        S  M  ⏻  ×     │    │                │
│ │ ┌ transfer ┐  ┌ harmonics ┐  DRIVE BIAS TONE        │    │                │
│ │ │  ╱‾‾     │  │ ▮ ▮ ▯ ▮ … │  MIX  OUTPUT DYN        │    │                │
│ │ └──────────┘  └───────────┘                         │    │                │
│ └──────────────────────────────────────────────────────┘    │                │
├ help strip ────────────────────────────────────────────────────────────────┤
└ footer: oversampling · latency · auto gain · Δ · ⏻ · MIX · PAN · OUTPUT ───┘
```

### 3.1 Spectrum
- Log frequency 20 Hz–20 kHz, −84…−18 dB as Shape's rail spectrum (1/6-octave smoothing, 60 ms
  rise and 300 ms fall, Catmull-Rom; the analyzer shared with the EQ and Shape).
- Input in `preSpectrum`, a quiet filled shape. Output as a lit line in `curve`.
- Added harmonics: the area between them where the output is above, in the accent.
- With bands: each region tinted at 10 % accent (selected at 18 %), breathing with the band's
  drive engagement `a`.

### 3.2 Band card
- **Model glyphs**: one 32 px icon button per model, each glyph a tiny drawing of its own curve
  (from the same function). The current one is lit in the accent; hovering one previews its
  curve as a ghost in the transfer square (never writes the parameter).
- **Transfer square** (260 × 260 on the wide card): input −1…1 across, output −1…1 up, the
  identity as a faint diagonal, the band's curve as a glowing accent line (`x + a(F − x)` with
  auto gain), and lit where the music is: the stretch from −p to +p, `p` the band's input peak
  (eased 60 ms up, 300 ms down), with a live dot riding the curve at +p. Tube's drifting bias
  moves the curve live. Tape and Transformer (slice 3) draw their loop.
  - Drag up/down: drive. Drag sideways: bias. Shift: fine. Alt-drag up/down: tone. Wheel: drive.
    Double-click: drive and bias back to their defaults.
- **Harmonics** (slice 4): H2–H9 bars, measured by a probe tone through the band's real
  processor at its live operating point. A harmonic above the base Nyquist is hollow.
- **Dials**: DRIVE (0…36 dB; 0–12 dB takes half the travel), BIAS (±100 %), TONE (±12 dB),
  MIX, OUTPUT; DYN when dynamics arrive (slice 6).
- Narrow cards (other bands): number badge, S and M, a transfer square of their own (lit where
  their music reaches; drag it as the wide one, which selects the band), DRIVE and MIX. The curve
  is the model's drawing, so no separate glyph.
- Wide card (the selected band of several, 600 px): badge, model icons, the band's range, S, M,
  power and remove (a cross in the text colour: Sat draws no danger role), the square, DRIVE
  large beside BIAS, TONE, MIX and OUTPUT in a 2×2 grid.

### 3.3 Footer
Oversampling (dropdown button: 1×, 2×, 4×, 8×; the help strip says each one's latency), the
latency readout, auto gain, Δ, bypass, MIX, PAN, OUTPUT, all as in Shape.

---

## 4. Parameters (frozen with the first release; `tests/golden/mite_sat.params.txt`)

Every parameter of the full design exists from slice 1, so the state stays at version 1 while
the features arrive. A parameter whose feature is not built yet sits at its neutral default and
the editor does not show it.

- Globals: `oversampling` (1×/2×/4×/8×, default 2×), `auto_gain` (on), `mix`, `out_pan`,
  `out_gain`, `delta`, `bypass`.
- Crossovers `x1…x5_on`, `x1…x5_freq` (defaults 120, 500, 2000, 6000, 12000 Hz). Crossover slot
  k owns band slot k + 1; band 1 is always the lowest region; slots never swap.
- Bands `b1…b6_`: `model`, `drive`, `bias`, `tone`, `mix`, `out`, `dyn`, `dyn_thr`, `dyn_att`,
  `dyn_rel`, `stereo` (Stereo, Mid, Side, M/S), `bypass` (clean but heard), `mute`, `solo`
  (monitoring).
- Modulation: `lfo1_*`, `lfo2_*` (`rate`, `sync`, `div`, `shape`, `phase`), `env_*` (`att`,
  `rel`, `gain`, `src`), slots `m1…m8_` (`src`, `dst`, `amt`). `src` and `dst` are not
  automatable; `dst` indexes a frozen, append-only target table
  (`tests/golden/mite_sat.modtargets.txt`).

---

## 5. Slices

0. Groundwork (done 2026-10-26): shared crossover, 8× oversampling, the crimson accent.
1. One band: models Tube, Diode, Soft, Hard, Fold and Rectify; oversampling; auto gain; the
   spectrum and one card. The engine already splits into bands (tested); the editor shows one.
2. Multiband (done 2026-10-26): the rail, cards and band editing shared with Shape
   (`mite_ui::widgets::rail`).
3. Tape and Transformer (hysteresis).
4. Harmonics probe and bars.
5. Per-band stereo modes.
6. Per-band dynamics.
7. Modulation matrix (shared with Delay, Filter and Mod).
8. Aliasing and THD report, CPU table, presets, validators.
