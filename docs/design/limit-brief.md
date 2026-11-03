# Mite Limit: design and build spec (in-house, 2026-11-03)

**From:** MiteFX engineering, designed here rather than in a Claude Design pass (user decision,
2026-11-03, as Sat). This file is both the design and the build spec; `interaction.md` "Mite
Limit" holds the gestures in the shared table format once the editor is built.

Rival: FabFilter Pro-L 2. The bar is the suite's: better looking than FabFilter, every pixel
computed from the DSP, direct manipulation, no words in the display. **One exception (user,
2026-11-03):** the loudness column shows numbers (M, S, I, LRA, TP, PLR), because those values
are what a mastering engineer reads. The plot itself stays wordless.

Scope (user, 2026-11-03): the full roster, delivered in slices (§6). Every parameter exists from
slice 1, so the state stays at version 1 while the features arrive.

---

## 1. What it is

A brickwall limiter with a ceiling that holds: in sample peak always (proved), and in true peak
when True Peak is on (measured against a reference interpolator in tests). Around that core:
a clipper stage before it, a multiband pre-stage that keeps transients in one band from ducking
the others, oversampling to 16×, styles that shape the release, full EBU R128 metering,
streaming loudness targets with a "what will the platform do" preview, unity-gain audition,
Delta, dither.

Neutral settings null: gain 0 dB with the ceiling above every peak, clipper off and protection
off give the input delayed by the reported latency, bit for bit (oversampling 1×), or within
the oversampler's passband error (above 1×; tested as with Sat).

### 1.1 The gain law (from Nova, kept for its proof)

Per channel, at the engine rate (the base rate times the oversampling factor), with `p[n]` the
peak the detector sees, `c` the ceiling, `L` the lookahead and `A₁ + A₂ − 1 = L`:

```
g[n] = min(1, c / p[n])                 the gain each sample needs
m[n] = min(g[n … n+L−1])                sliding minimum, looking ahead (audio delayed by L−1)
s[n] = box_A₂( box_A₁( m ) )[n]         two boxcars: the attack ramp, S-shaped
r[n] = release(s)[n], with r[n] ≤ s[n]  styles shape the way up, never above s
y[n] = x[n − L + 1] · r[n]
```

Every term inside the two averages at time `n` is an `m[j]` with `j ≥ n − L + 1`, so its window
contains the delayed sample, so `s[n] ≤ g` of that sample and `|y| ≤ c`.
The release may only lower the gain further (clamped to `s`), so whatever a style does, the
ceiling holds. One boxcar (Nova) ramps in a straight line with a corner at each end; two give a
quadratic S, which the suite's no-corners rule asks for. **The ramp spans the whole lookahead**
(Nova's "attack is the lookahead"; 2026-11-03, measured in slice 0: a shorter ramp only reaches
its depth early and holds flat until the peak, so a separate ramp length buys nothing). The
ATTACK control is the slow envelope's attack (§1.3), as Pro-L 2's attack is.

- **Exact arithmetic:** gains are integers in units of 2⁻²⁴, rounded down, so the boxcars are
  exact sums with no drift and the proof holds bit for bit (`|y| ≤ 1 + 1e-6` in the fuzz).

- **Lookahead 0** is allowed and honest: `L = 1`, `s = g`, a clipper. The editor says so.
- **Proof as a test:** a seeded fuzz over signals, ceilings, lookaheads, attacks, styles,
  releases and links asserts `|y| ≤ c` on every sample (bit-level, no tolerance), at every
  oversampling factor before the down filter.
- Parameters that size `L` are not smoothed (latency must follow the exact value; Nova's rule).
  A lookahead change re-reports latency to the host and crossfades two cores, as Sat's split
  changes do.

### 1.2 True peak

Off: `p[n] = |x[n]|` at the engine rate. On: `p[n]` is the largest magnitude of a polyphase
interpolation of the engine stream to at least 4× the **base** rate (so at 4× and above the
engine's own samples already count, plus a 2× interpolation for margin), and the window `L`
grows by the interpolator's half length so the gain is down before the interpolated peak.

The ceiling is on the **output after the down filter**, measured as BS.1770-4 measures it.
Tests: the output's true peak, by our BS.1770-4 meter and by a 32× reference interpolator,
stays within 0.05 dB (meter) and 0.1 dB (reference) of the ceiling over a battery: sines near
Nyquist, the ITU-R BS.2217 true-peak signals as described there (generated in the test), square
waves, clipped noise, kick-and-hat loops at +20 dB of gain. If the down filter's ripple breaks
that, a final true-peak guard (the same law, 0.5 ms, on the output) catches it; its lookahead is
part of the reported latency.

**Measured (slice 1, 2026-11-03):** it does. Above 1× the down filter removes what the limiting
put above the base Nyquist, and on bright music that lifted the peaks by up to 0.2 dB (−0.81
dBTP against a −1 ceiling, at 2× and 4×). The guard runs whenever the engine oversamples (true
peak on: its own 4× detector; off: sample peaks), with a 20 ms release; it adds 0.5 ms plus, with
true peak, the detector's 0.5 ms. With it the output reads −1.000 dBTP. The engine test plays a
bright square under a kick at every factor and style and fails without the guard (0.08 dB over).

### 1.3 Release and styles

Two envelopes in dB. The **fast** one drops with `s` at once and rises at the release. The
**slow** one follows `s` down at the ATTACK time (sustained reduction builds it, a lone hit hardly
does) and rises at the release × its ratio. The gain is `min(fast, share · slow)`, never above
`s`. Program dependence: sustained reduction (the slow envelope) slows the fast release. A style
is a row of numbers (as Comp's Characters): attack scale, release scale, slow share, slow ratio,
program dependence (`mite_dsp::limit::STYLES`).

Styles (frozen order, append only; glyphs in the editor, names in the help strip):

| # | Style | Feel |
|---|---|---|
| 0 | Transparent | Single release, light program dependence. The least colour. |
| 1 | Punch | Short attack scale, fast release on transients, slow on sustain: keeps the hit. |
| 2 | Dense | Fast release, high program dependence: loud and thick, the modern master. |
| 3 | Bus | Long attack and release: glue rather than catching. |
| 4 | Safe | Longest attack and release: no distortion at any cost, for broadcast. |
| 5 | Custom | The dials' values unscaled, and the slow envelope's share as a dial. |

### 1.4 Clipper stage (before the limiter)

At the engine rate, after input gain: `x → clip(x)`, with its level `C` set relative to the
ceiling (0…+12 dB above it) and its curve: Hard, Soft (cubic knee) or Round (tanh-like, the
knee as wide as the level above the ceiling). It shaves the sharpest few-sample peaks, so the
limiter does less on them and pumps less. The limiter after it still guarantees the ceiling.
Off, it is not in the path.

### 1.5 Transient protection (multiband pre-stage)

On deep limiting, one band's hit (a kick) ducks every other band. The pre-stage splits the
engine stream into three bands (`mite_dsp::crossover`, LR4 with allpass compensation; split
points 150 Hz and 3 kHz, not exposed), and on the same lookahead gives each band its share of
the reduction the next over needs: a band's share is its contribution to the peak (the part of
the peak with the peak's sign), so the band that makes the over is the band that ducks. The
amount (0…100 %) moves between broadband (0) and fully assigned (100). The broadband law after
it guarantees the ceiling on whatever is left. At 0 % the bands are not computed and the path is
the plain limiter.

### 1.6 Stereo

L/R with two links (as Pro-L 2): **transients** (how far the attack follows the louder channel)
and **release** (how far the release does). 100 %/100 % keeps the image fixed. M/S is not
offered: a limiter's ceiling is on L and R.

### 1.7 Output

After the limiter and the down filter: output level is the ceiling itself (Pro-L's "output"),
then **unity-gain audition**, **platform preview**, **Delta**, then **dither**.

- **Unity-gain audition** (monitoring): the output is turned down to the input's loudness, so
  you judge the limiting and not the gain. Better than Pro-L's (which subtracts the gain dial):
  it matches the measured short-term loudness of output and input (K-weighted, 3 s), so it is
  right whatever the clipper and the release did. The trim glides (2 s) and the help strip and
  the footer button warn that it is a monitoring switch.
- **Platform preview** (monitoring): the output is turned by the gain the chosen platform would
  apply (§3.3), so you hear what its listeners hear.
- **Delta:** the delayed input times the input gain, minus the output: what the limiter and the
  clipper took away, time-aligned.
- **Dither:** Off, 24, 20, 16 bit (TPDF), with noise shaping Off / Light / Strong (fixed
  error-feedback filters, tested for spectrum and level). Last in the chain; off by default.
- **DC filter** (before everything; 5 Hz, second order): off by default.

---

## 2. Colour

- **Accent:** `limit` = **silver** (tokens.json): dark `#A4ACB4`, light `#4C535B`, hc `#BDC5CE`.
- Gain reduction in the semantic `gr` role, input in `preSpectrum`, output in `curve`, as Comp.
- The ceiling is a line in the accent with a lit grip; the clipper's level a dashed accent line.
- Loudness above the chosen target is `warn`; true peak above the ceiling (never, in theory) or
  the target's TP limit is `danger`.

---

## 3. Main view (1280 × 760)

The suite's frame: header 56, display 620, help strip 28, footer 56.

```
┌ header: mark · Mite Limit · ‹ preset › · undo redo · A B C D ───────────────────────┐
│ history (x 0–936): 6 s scrolling, input dim, output lit,       │ IN  GR  OUT │ LOUD │
│ GR hanging from the top in gr, ceiling line + grip,            │ meters      │ M S  │
│ clipper line (dashed) + grip, gain = drag the waveform up      │ (TP)        │ I    │
│                                                                 │             │ LRA  │
│ ┌ shape panel (docked bottom-left) ─────────────────────┐      │             │ TP   │
│ │ style glyphs  ○ ● ○ ○ ○ ○     the gain shape (probe)  │      │             │ PLR  │
│ │ LOOKAHEAD ATTACK RELEASE  LINK T  LINK R               │      │             │target│
│ └───────────────────────────────────────────────────────┘      │             │      │
├ help strip ─────────────────────────────────────────────────────────────────────────┤
└ footer: oversampling · TP · latency · clip · protect · dither · unity · Δ · ⏻ · GAIN · CEILING ┘
```

### 3.1 History
- 6 s across, the level range 24/36/48/60 dB or Auto (as Comp's), the same column binning and
  easing (shared with Comp: the history drawing moves to `mite_ui` in slice 1 rather than being
  copied).
- Input after gain, dim filled; output, lit; GR from the top at the levels' own scale; where the
  clipper acts, the output's peaks are tinted in the accent (what the clipper took).
- **Ceiling:** a horizontal accent line at the ceiling, its grip on the right edge (drag:
  ceiling; double-click: −1 dBTP). **Clipper:** a dashed line above it with its own grip, shown
  while the clipper is on; dragging it down to the ceiling is the most clipping.
- **Gain: drag the waveform** up or down anywhere else in the history (Pro-L's gesture); the
  input lifts with it live, so you see the peaks cross the ceiling. Wheel: gain. Shift: fine.
- Hover: the help strip reads the time, level and GR under the pointer.

### 3.2 Shape panel
- **Style glyphs:** one 32 px icon per style, the current one lit in the accent; hover previews
  the style's gain shape as a ghost.
- **Gain shape:** the real gain the limiter applies to a 6 dB, 50 ms over burst (a probe through
  the engine's own code, as Sat's harmonics), drawn against time: the S-shaped attack before the
  burst, the hold, the release after it. Drag its left flank sideways: attack; its right flank:
  release; the lookahead is the dashed line where the burst starts. Never a corner (two boxcars).
- Dials: LOOKAHEAD (0–10 ms; the ramp), ATTACK (0.1–100 ms; the slow envelope's), RELEASE
  (1 ms–2.5 s, common values given travel), LINK T and LINK R (0–100 %), and SLOW (the slow
  envelope's share) on Custom only.

### 3.3 Loudness column (numbers allowed here)
- Momentary, short-term, integrated (gated), LRA, true peak (max), PLR. A click resets. Bars for
  M and S with the target as a mark; I as a large number.
- **Target:** a dropdown of platforms (frozen table, append only; values as published in 2026,
  reviewed yearly):

  | Platform | Integrated | TP limit | Turns up? |
  |---|---|---|---|
  | Spotify | −14 LUFS | −1 dBTP | yes, to −1 dBTP headroom |
  | Apple Music | −16 LUFS | −1 dBTP | yes, to −1 dBTP headroom |
  | YouTube | −14 LUFS | — | no |
  | Tidal | −14 LUFS | — | no |
  | Amazon Music | −14 LUFS | −2 dBTP | no |
  | Deezer | −15 LUFS | — | no |
  | EBU R128 | −23 LUFS | −1 dBTP | delivery spec |
  | ATSC A/85 | −24 LKFS | −2 dBTP | delivery spec |
  | Custom | the custom target dial | | |

- The column shows what the platform will do to this master (its gain, from the integrated
  loudness), and the **preview** button plays it.

### 3.4 Footer
Oversampling (1×, 2×, 4×, 8×, 16×; the help strip says each one's latency and cost), true peak,
the latency readout, clipper (on, its curve as glyphs), protection (amount dial), dither, unity,
Δ, bypass, GAIN, CEILING, as in the suite's other footers.

### 3.5 Meters
IN (input after gain), GR, OUT (true peak when TP is on, else sample peak), at 60–120 fps from
telemetry atomics; peak hold with its value in the loudness column (exception).

---

## 4. Parameters (frozen with the first release; `tests/golden/mite_limit.params.txt`)

- Core: `gain` (0…+30 dB, 0), `ceiling` (−30…0 dB, −1.0), `style` (0…5, Transparent),
  `lookahead` (0…10 ms, 2 ms), `attack` (0.1…100 ms, 30 ms), `release` (1…2500 ms, 100 ms),
  `slow` (0…100 %, Custom only), `link_t` (100 %), `link_r` (100 %), `true_peak` (on),
  `oversampling` (1×/2×/4×/8×/16×, 2×).
- Clipper: `clip_on` (off), `clip_level` (0…+12 dB above the ceiling, +3), `clip_curve` (Hard,
  Soft, Round; Soft).
- Protection: `protect` (0…100 %, 0).
- Output: `unity` (off), `delta` (off), `bypass`, `dither` (Off, 24, 20, 16), `shaping` (Off,
  Light, Strong), `dc_filter` (off).
- Loudness: `target` (the table above, Spotify), `target_custom` (−30…−5 LUFS, −14), `preview`
  (off). `target` is not automatable.

---

## 5. Shared work this needs

- `mite_dsp::loudness`: BS.1770-4 K weighting, momentary, short-term, integrated (two-stage
  gating), LRA (EBU Tech 3342), true-peak meter (4× polyphase, BS.1770-4 Annex 2). Tested
  against EBU Tech 3341 and 3342 cases, generated in the tests from their descriptions. Meter,
  Comp's loudness detector and auto makeup can later move onto it.
- `mite_dsp::oversample`: a fourth half-band stage for 16×.
- `mite_dsp::limit`: the sliding minimum, the two boxcars, the release and styles, the clipper,
  the protection pre-stage, the true-peak detector, dither.
- `mite_ui`: the history drawing out of Comp (Comp pixel-identical after the move).

---

## 6. Slices

0. Groundwork (done 2026-11-03): `mite_dsp::loudness` (K weighting equal to BS.1770's table at
   48 kHz; EBU Tech 3341 cases 1–5 and 9 and Tech 3342 cases 1–4 pass, generated in the tests;
   the true-peak interpolator, 48 taps a phase, flat within 0.01 dB to 0.9 × Nyquist), the 16×
   stage (latency 79), and the gain law in `mite_dsp::limit` with its proof fuzz (160 random
   cases, settings moving mid-stream) and true peak held within 0.01 dB on tones to 0.49 fs.
1. The limiter (done 2026-11-03): core, styles, true peak, oversampling with the guard (§1.2),
   Delta, bypass; the plugin crate with every parameter (state 1); the history moved out of Comp
   into `mite_ui::widgets::history` (Comp's renders equal within GPU noise), meters, the ceiling
   and gain gestures, the strip's style glyphs and dials; the loudness column's numbers
   (momentary, short-term, integrated, range, true peak, PLR; the targets come in slice 2). The
   strip sits under the history (468 px tall) rather than over it. A lookahead, true-peak or
   oversampling change dips (10 ms out, in) as Comp's does rather than crossfading two cores.
   CPU (20 s of music, 9 dB in, true peak on): 1× 2.0 %, 2× 3.4 %, 4× 4.7 %, 8× 7.3 %, 16× 12.5 %
   of one core (the release's per-sample dB conversions are the next thing to make cheaper).
   Editor 120 fps at 1× and 2× DPI. clap-validator 34 passed, 0 failed.
2. Loudness column (done 2026-11-03): the target list (`dsp::PLATFORMS`, in `params::TARGETS`'
   order: level, true-peak limit, whether it turns quieter masters up, tolerance), its level
   marked on the M and S bars, the integrated value in warn while over the target, true peak in
   danger over the target's limit, PLATFORM GAIN (`dsp::platform_gain_db`: down to the target;
   up only where the platform turns masters up, and no further than its true-peak limit), and
   the preview, which plays that gain (300 ms glide). The column measures the master before the
   preview; the meters and the history show what is heard, and a speaker marks the history while
   it is on. Tested: a master 5 LU over Spotify is heard at −14 LUFS ±0.5.
3. Gain shape probe in the shape panel, its flank gestures.
4. Unity-gain audition (loudness-matched).
5. Clipper stage, its line and grip.
6. Transient protection.
7. Dither, noise shaping, DC filter.
8. Report and finish: `docs/dsp/limit-truepeak.md` (compliance and CPU per factor), presets,
   `docs/compare/pro-l-2.md`, validators, CI.
