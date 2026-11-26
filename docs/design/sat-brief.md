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
runs oversampled (1/2/4/8/16×, 4× by default, linear-phase half-bands, ≥ 100 dB image and alias rejection).
Modulation (two LFOs, an envelope follower, 8 slots) can move any band control.

Neutral settings null: drive 0 dB, band bypass, band mix 0 or global mix 0 are the input
(delayed by the reported latency) bit for bit.

### 1.1 The band signal path (what the visuals draw)

```
u      = K·g·x + b          g = drive (0…60 dB), K = −12 dB calibration, b = operating point
b      = 0.5·β + (0.5·β [− 0.2 Tube])·(√(u² + 1) − 1)   β = bias (−1…1): grows with the sample, no memory
F(x)   = (f(u + b) − f(0.5·β)) / K · L   L = 10^(−min(drive, 24 dB)/20): quiet signals keep slope g·L
wet    = Tone( down( F( up(x) ) ) )
y      = x_d + a·HP₅(wet − x_d)      a = smoothstep(drive / 6 dB) · engaged · band mix
out    = y · auto gain · D(e_in) · band output      D: the band's dynamics (below)
```

`x_d` is the band delayed by the oversampler's round trip, so `a = 0` is the band exactly.
The 5 Hz high-pass acts on the difference only: it removes the DC a bias makes and leaves the
band's own lows alone. Under 24 dB drive only moves the knees towards the signal (quiet signals
pass at unity, `g·L = 1`); from 24 dB up each dB is a dB of gain too (louder, as hardware); auto
gain (on by default) brings the level back.

The editor's transfer curve is `x + a·(F(x) − x)` times the auto gain, from the same functions.

**Calibration (2026-11-01):** user report: drive had to reach 36 dB to be heard on a kick. Measured:
the knees sat near 0 dBFS, so a kick peaking at −18 dBFS only started to distort past +18 dB, and
auto gain hid the level change too. `K` puts the knees 12 dB under full scale at drive 0.

**Static operating point (2026-11-24, user chose static):** the point used to follow the envelope
of what enters the curve (10/150 ms) so asymmetry held at any drive; after a loud passage it held
the point where the quiet audio after fell on the curve's flat part: muted 15–25 dB for 150 ms,
with a thump. Now it grows with the sample itself: no memory, no muting (swing ≤ 0.2 dB). Even
harmonics hold through 36 dB of drive; nearer a square wave (48–60 dB) they fade, except Diode's.
Auto gain's level meter is 2 s (was 300 ms) and starts as a plain average, its gate 5 ms (was
20): it no longer rides between loud and quiet stretches (±0.5 dB → ±0.03).

**Drawn harmonics and Punch / Body (2026-11-24, state 8; user picked them from "more than
Saturn"):** model 8, **Harmonic**: `P(x) = x + Σ aₖ·(Tₖ(x) − Tₖ(0) − Tₖ'(0)·x)` over the 2nd…9th
Chebyshev polynomials, the input the identity to ±1 and landing on ±1.05 past it (value, slope,
curvature matched). For a sine reaching ±1 each harmonic is its drawn level within 0.5 dB (the
weights solved for the fundamental the odd ones add, `recipe()`); quieter, the low orders lead;
slope 1 and 0 at 0. Per band `h2…h9`, dB against the fundamental, −72 off (default a warm one).
The bars stay what the band makes; in Harmonic each carries a marker at its target, and dragging
a bar's column sets it. On any other model a drag turns the band Harmonic, seeded from the bars
as shown, so the sound carries on; model and recipe are one undo step. Feedback divides its loop
by Harmonic's steepest slope (times the bias's widening), so a drawn curve cannot run away.
**Punch / Body**: per band ±24 dB of extra drive on the hits and on what sustains, weighted by
Mite Shape's transient detector (30 ms) on the band going in; like drive under 24 dB it moves the
knee, not the level (the push comes back off after the curve). 0 / 0 is the band bit for bit.
Tested: hits through Soft, Punch moves the first 20 ms 6 dB more than the tails, Body the reverse.

**Minimum Phase is low latency (2026-11-24, user chose it over a third choice):** the Phase menu's
Minimum Phase now also oversamples with minimum-phase allpass half-bands (`mite_dsp::oversample_iir`,
HIIR's design; ≥ 100 dB against folding, flat within 0.01 dB to 0.9 × Nyquist): latency 4 / 5 / 6 /
6 samples at 2 / 4 / 8 / 16× (was 69 / 76 / 78 / 79; Saturn 2's Good: 8), aliasing identical to
the linear-phase filters. Each band's dry goes through the same round trip (`dry_os`): against a
plain delay the minimum-phase wet combed −4…−15 dB at 15–18 kHz in a 50 % blend. So in Minimum
Phase, mix 0 and neutral settings are the input's level but not its phase above ~10 kHz; bypass
still nulls, and Linear Phase keeps the bit-exact null and no phase shift anywhere (the BLUEPRINT
§5 null rule, with this exception the user chose). Reported latency is the round trip's measured
low-frequency delay, within half a sample (tested).

**Aliasing (2026-11-24, state 7):** measured in 20 Hz–20 kHz on a 4987 Hz tone at −6 dBFS (as the
Saturn 2 comparison reads it), the hard curves were the limit at 4×: Hard −54.5 dB at 24 dB of
drive, −38.6 at 36. A smoother join on Hard's knee changed nothing (its sharpness, not its
corner, is the cause; a wider knee would make it softer), so 16× is offered: Hard −90.5 and
−65.4, Soft at 36 dB −50.3 → −89.9, Tape at 36 dB −27.7 → −54.8. The default is 4× (was 2×: Hard
−40 at moderate drive). CPU, one band, 4× → 16×: Soft 1.1 → 3.2 %, Tube 1.4 → 4.5 %, Tape 4.3 →
11.5 % of a core. `distortion()`'s alias figure also counts what folds into 20–24 kHz near
Nyquist (inaudible), so it plateaus with the factor; read the audible band.

**The thump after loud passages is physics, kept (2026-11-24, measured):** an asymmetric curve
(Bias, Tube) makes DC with its even harmonics, in a fixed ratio (a u² term: as much DC as H2), and
that DC follows the level. Any DC removal (ours: the 5 Hz high-pass on the difference; analog: the
coupling capacitors) turns a level drop into a low pulse as big as the DC step: after a −6 dBFS
burst stops, −26 dB (Tube, 12 dB of drive) to −10 dB (Tube, bias 60 %, 48 dB) against the burst,
nearly all of it above 30 Hz, so a steeper filter would not shrink it. Only knowing the level
ahead would, and a remembered level is what muted quiet audio before. Saturn 2's styles show the
same recovery (+0.3…+1.5 dB in the compare memory table).

**Feedback (2026-11-24, state 6, user picked it from Saturn 2):** per band, `fb` 0–100 % and
`fb_freq` 20 Hz–10 kHz (250 Hz). At the high rate, the curve's last output (its own units)
through a state-variable band-pass (Q 2, unity at its peak) adds to `u`: `u = K·g·x + 0.9·fb·BP(f)`.
Joining after the drive keeps the loop gain under 1 at any drive (curves are at most 1.05 steep),
so it rings and never runs away; joined before it, the drive would multiply the loop. Noise
through Soft at 30 dB: +1.8, +4.1, +7.2, +11.5 dB at 1 kHz for 25…100 %, 4 kHz untouched. 0 %
clears its state: the band bit for bit. The harmonic bars and the transfer square do not show it.

**Drive law (2026-11-24, state 5):** measured against Saturn 2 (`cargo xtask compare sat`,
docs/compare/saturn-2.md): with knees 12 dB under full scale at drive 0, a −12 dBFS track already
sat at them, and the first 6 dB faded a hard-driven sound in as a mix: untouched to 6 % THD+N in 3
dB of drive, no gentle range, and the light settings full of upper harmonics. `K` is now −12 dB
and the range 0…60 dB: a −12 dBFS track starts about 24 dB under the knees (Tube under 1 %) and
meets them near 24 dB. `L` keeps the level: an old setting `d ≥ 6` is `d + 24` with the same push
and the same output, auto gain on or off (the old fade-in 0…6 spreads over 0…30; migrated
sessions, snapshots, modulation amounts on drive and presets, `migrate::v4_to_v5`).

**Auto gain matches the hits (2026-11-02):** user report, after the calibration: 36 dB still did not
sound like much. Measured on the user's kick stem (peak −12 dBFS, RMS −39): the engine clipped it
nearly square, but auto gain predicted the curve's gain at the band's 300 ms level, which on sparse
material is the gaps; it read the kick as quiet, expected the whole 36 dB and took it off, so the
hits came out at −29 dBFS. Its meters (the prediction's level and the trim's) now only run while
the band's 20 ms level is within 10 dB of its recent loudest (falling back with a 1 s time
constant), so tails and bleed are left out and steady material is unchanged. Same stem: the hits'
peaks −22 dBFS, the hit bodies within 2 dB of the input (tested on synthetic hits over bleed, for
Tube, Hard and Tape; with the gate open they came out 6.8 dB low). A kick clipped near square has a
crest factor of about 3 dB, so at matched loudness its peaks sit lower; turn auto gain off for level.

**The operating point follows the level (2026-11-02):** user report: hardly any even harmonics.
Measured: six of eight models made none at bias 0, and every model's H2 fell as drive rose (Tube
from −24 dB at 12 dB of drive to −35 at 36; with bias 30 %, Transformer to −56), because a fixed
offset is lost under a signal 60 times its size, and a curve clipped on both sides at equal times
(50 % duty) makes no evens. Now the point is `β·0.5` in silence plus `β·0.5` of the envelope of what
enters the curve (Tape and Transformer: after their emphasis), so bias sets where on the wave the
clipping starts, at any drive (a hard clip at relative offset θ gives H2/H1 = θ); Tube's blocking
drift is `0.2·e` with no ceiling (it was capped at 0.5). Fold and Rectify keep a fixed point (theirs
is a place on a curve that never flattens; a moving one would sweep through the folds). Since
`f(b)` comes off, a moving point does not thump. Measured at −18 dBFS: Tube's H2 −25 → −16 dB from
12 to 36 dB of drive, Diode −22…−25 dB at bias 0, and at bias 30 % every saturating model holds
−13…−17 dB at 36 dB of drive. The probe solves the envelope's steady state (bisection on one cycle;
running it would take hundreds of cycles), Tape's and Transformer's rest magnetisation is
interpolated along its slope between recomputes (every 1 % of `1 + |b|`); CPU unchanged.

**Stereo modes (slice 5):** left and right, or `M = (L+R)/2` and `S = (L−R)/2`, go through the path
above. Each part that is driven gets its difference and auto gain, and a part left clean only the
band's fader. L and R start from what they were and only take on the differences, so a neutral band
is bit-exact in every mode. Auto gain reads the level of what is driven. A mode change crossfades
two cores, as a split change does. Mono ignores the mode.

**Dynamics (slice 6, 2026-11-03):** after the curve, each band has a gain `D` set by its input,
not its output: a peak detector (instant attack, 10 ms release) hears what goes into the curves
(the band before drive, the driven parts in a stereo mode), so drive never moves where it acts.
**The threshold is automatic (user, 2026-11-03, as Saturn has none):** it is the band's own
level, the detector's reading in dB averaged over a second (once per 16-sample stretch, on its
loudest reading), held below −70 dBFS so silence does not drag it down. Steady material sits at
its own average and is left alone; what rises above it (hits, accents) is what moves, whatever
the track's level (tested within 1 dB across 20 dB). The `dyn_thr` parameter stays in the state
but is not used. The gain computer is `dynamics_gain_db`: with `o` the input's excess over that
average through a 6 dB soft knee, DYN `d < 0` gives `d·o` (−50 % is 2:1, −100 % ∞:1) and `d > 0` gives
`d·o` capped softly at +12 dB (an upward expander: +100 % is 1:2, so hits the curve flattened come
back). The gain eases in dB with the band's attack (moving away from 0 dB) and release (back).
It acts on what the band drives (in Mid, the side keeps its fader only), even at drive 0, and it
is not seen by auto gain (which matches the curve alone). DYN 0 and band bypass leave `D` exactly 1,
so both null bit for bit. Cost: six bands of Tube at 4×, 8.9 → 9.2 % of a core.

### 1.2 Models (frozen order; append only)

| # | Model | Curve `f(u)` (`f(0) = 0`, `f'(0) = 1` but Rectify's 0, smooth: no corners) | Character |
|---|---|---|---|
| 0 | Tape | +18 dB shelf above ~2 kHz → Jiles–Atherton core (wide loop) → its exact inverse | Soft, loose, HF saturates first (and comes out compressed, little added distortion) |
| 1 | **Tube** (default) | `(tanh(A(u + C)) − T)·S`, one off-centre tanh: ceilings +0.8 and −1.25, slope 1 at 0; the point drifts down by 0.2 of the envelope (blocking) | Asymmetric: even-led (H2 first) when gentle, more as it is pushed, held at any drive. Analytic: the two-piece curve it replaced (tanh above 0, exponential below, 2026-11-24) jumped in curvature at 0 and kept the 5th harmonic and up at −30 dB at any level |
| 2 | Transformer | +18 dB shelf below ~120 Hz (the flux integrates) → JA core (narrow loop) → its exact inverse | Low end saturates first |
| 3 | Diode | `k·asinh(u/k)`, `k` 0.3 above 0, 0.6 below (one diode against two) | Hard knee, log growth, never a ceiling; halves unlike, so even harmonics |
| 4 | Soft | `u − 4u³/27` to ±1.5, then ±1 | Classic cubic: odd harmonics, gentle |
| 5 | Hard | `u` to 0.95, a quadratic knee to 1 at 1.05, then ±1 | Clipping, rounded so it never folds a corner into the spectrum |
| 6 | Fold | `sin(u)` | Wavefolder: past ±π/2 it folds back |
| 7 | Rectify | `tanh(√(u² + 0.05²) − 0.05)` | Full-wave: the fundamental gives way to the octave |

### 1.3 Hysteresis (Tape, Transformer)

A Jiles–Atherton core in normalised units (`mite_dsp::hysteresis`): `m_an = L(h + A·m)` with the
Langevin function, an irreversible part that relaxes `m` towards `m_an` over a field distance of
about `K` (the loop's width), a reversible share `c`. Tape: `A` 0.1, `K` 0.3, `c` 0.97 (was 0.82);
Transformer: `K` 0.12, `c` 0.95 (was 0.9). At 0.82 the loop alone made 1 % THD+N at a tenth of
full field, rich in upper harmonics; a biased tape machine is clean there, so tape keeps only a
little loop (2026-11-24); a transformer, unbiased, keeps more. A high reversible share keeps quiet signals clean (an unbiased core's gain grows
with level at the bottom of its loop, what AC bias hides on tape): within 1.5 dB below the knee,
H3 of a quiet sine under −40 dB. Output scaled to slope 1 for a quiet sine, so the blend with the
dry band has no level step; the rest magnetisation at the operating point is taken off, so silence
stays silence.

The relaxation is stiff (an explicit solver diverges once a sample moves the field further than
`K`), so each substep is solved exactly with its coefficients held and the target moving linearly
(an exponential integrator), as predictor and corrector; substeps are spaced evenly in
`asinh(h)`. The settled loop is within 0.005 of a finely solved one for swings to ±400 at 12
samples a cycle. The core is rate independent; only the emphasis sees the frequency.

Auto gain reads the band's level after the model's emphasis (what drives the core); the measured
trim reaches ±12 dB for these two (their prediction depends on the band's spectrum).

**No ADAA.** First-order antiderivative anti-aliasing was planned and dropped (2026-10-26).
On the curve's nonlinear part alone, it leaks `g·Δu/2` of high-frequency energy at high drive.
On the whole curve, it delays the wet by half a sample against the dry, so mix and drive below
6 dB would comb. Every curve is instead smooth (C¹), and the oversampler does the
anti-aliasing. Aliasing is measured per factor (`docs/dsp/sat-aliasing.md`).

### 1.4 The tape machine (2026-11-25, state 9; user: "a real tape machine", as a stage)

**Moved out on 2026-11-26 into Mite Tape** (its own plugin, BLUEPRINT §6.17; user: a tape-style
curve and a tape machine in one plugin made little sense). What follows describes the machine,
which is unchanged; in Sat only its eight hidden, unused `tape_*` parameters remain.

One deck after the bands are summed, as a mix goes to tape (`mite_dsp::tape`): it acts on the
whole signal, as speed, head bump and wow do on a real machine (per band, the bands would wobble
apart). Off by default; on, it is part of the wet signal (mix and delta read it; the band sum is
delayed to meet it), and switching it moves the latency behind the oversampling dip.

```
x → R(s) → up → ·g → core → ·1/g → down → + hiss → R⁻¹(s) → bias shelf → head loss → bump → transport
```

- **Record level** `g` (−12…+24 dB), applied around the core alone at the high rate and ramped per
  sample over each control step, so nothing with memory but the tape sits between it and its
  undoing (applied before the record EQ, a jump end to end clicked +42.7 dB through the
  filters). At 0 dB a −18 dBFS tone is the operating level and −8 dBFS the maximum output level:
  3 % THD at 1 kHz on 15 ips (tested); −18 dBFS stays under 0.6 %.
- **Core:** Tape's Jiles–Atherton loop (`hysteresis`) with `FIELD = 4.4`, scaled to unity for a
  quiet sine at every bias (tabulated).
- **Record EQ** `R`: the standard's treble shelf from τ₁ (NAB 50 µs, 90 at 3.75 ips; CCIR 35 µs at
  15, 70 at 7.5; AES 17.5 µs at 30) to a depth that grows as the tape slows (9…18 dB), NAB's bass
  cut under τ₂ = 3180 µs (to −10 dB), and the record-loss compensation (a second shelf from
  4 kHz × speed/7.5 ips, +12 dB). Playback is its exact inverse, so a quiet signal comes out flat
  but for the head. Slow tape saturates its highs first (10 kHz at −20 dBFS: compressed more at
  7.5 ips than at 30, tested).
- **Bias** (−100…+100 %): under-biased keeps more loop (reversible share 0.97 → 0.85, a 40 % wider
  loop) and lifts the top 2.5 dB; over-biased is cleaner (0.99) and loses its highs (−8 dB at
  the top, bias erasure). The shelf is `x + (g − 1)·HP(x)` with the high-pass fixed (3 kHz ×
  √(speed/7.5 ips)): only the gain moves, per sample (switching its coefficients every control
  step clicked +23 dB). Tested: THD at −26 dBFS more than doubles from 0 to −100 %; 12 kHz drops
  more than 3 dB at +100 %.
- **Head:** gap 2 µm and spacing 0.5 µm against the wavelength `λ = v/f`:
  `|sinc(π·g/λ)|·e^(−2π·d/λ)`, a 32-tap minimum-phase FIR per speed (−1.6 dB at 20 kHz on 15 ips,
  −3.5 on 7.5, −8.4 on 3.75); the head bump (16 / 30 / 52 / 105 Hz, +3 / 2.5 / 2 / 1.5 dB, Q 1.4),
  a −1 dB dip at 2.2× it, and the fringing loss under it (second-order high-pass at bump / 2.5).
  Tested: quiet tones within 0.6 dB of the head's loss at 200 Hz, 1 kHz and 5 kHz on every speed.
- **Hiss:** white on the tape (after the core), through playback; at 100 % the deck's own noise,
  −58 / −63 / −66 / −68 dB under the operating level for 3.75…30 ips (within 1 dB, tested). One
  source per channel, so the output is the same at every block size.
- **Transport:** one for both channels: a 32-tap sinc reading of a delay line around a 3 ms centre.
  Wow (0.6 % peak at 100 %: 0.55 Hz and a smoothed random drift) and flutter (0.25 % at 100 %:
  7.3 and 12.9 Hz). Both at 0: an exact delay. Default 10 % each.
- **Speed or standard change:** two heads crossfade over 30 ms (each with its own state), the
  transport after them shared.
- **Latency:** the minimum-phase oversampler's round trip plus the transport's centre (164
  samples at 48 kHz, 174 with Sat's own at 4×), an impulse lands there (tested at every factor).
- **CPU** (`examples/bench`, music, stereo, hiss and wow on): the deck adds 5.1 % of a core at 1×,
  8.9 % at 4×, 19 % at 16× (the core on both channels is the cost).
- **Gates:** the newest golden state (v9) has the deck on (+4 dB, 7.5 ips, hiss 30 %, wow 20 %),
  so every gate runs through it. Running it found two older Sat clicks the step test's window had
  missed (Crossover 1 and Band 2 Tone, skipped with the reason in `tests/gates.rs`).

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
- **Model glyphs**: one 32 px icon button per model, a pictogram of what it is (2026-11-24, user: the tiny curves all looked alike): Tape a cassette, Tube a valve, Transformer two coils on a core, Diode its schematic symbol, Soft a pillow, Hard a brick wall, Fold a pleated sheet, Rectify a bridge of four diodes. The card's transfer square draws the curve.
  The current one is lit in the accent; hovering one previews its
  curve as a ghost in the transfer square (never writes the parameter).
- **Transfer square** (260 × 260 on the wide card): input −1…1 across, output −1…1 up, the
  identity as a faint diagonal, the band's curve as a glowing accent line (`x + a(F − x)` with
  auto gain), and lit where the music is: the stretch from −p to +p, `p` the band's input peak
  (eased 60 ms up, 300 ms down), with a live dot riding the curve at +p. Tube's drifting bias
  moves the curve live. Tape and Transformer: the faint curve is their core's anhysteretic
  curve, the lit part is the loop a sine at the band's peak traces through the real core and
  emphasis at the band's centre frequency (`magnet_loop`). The full-scale loop is not drawn: that
  deep in saturation the emphasis turns the harmonics against the fundamental and it twists into
  a figure of eight.
  - Drag up/down: drive. Drag sideways: bias. Shift: fine. Alt-drag up/down: tone. Wheel: drive.
    Double-click: drive and bias back to their defaults.
- **Harmonics** (slice 4): H2–H9 bars, measured by a probe tone through the band's own code
  (`probe_harmonics`) at the band's centre, its peak and the live operating point, 0 to −72 dB
  against the strongest partial (Rectify gives the fundamental away). A harmonic above the base
  Nyquist is hollow. With no music the probe plays at −12 dBFS and the bars are dim. Single and
  wide cards only; on the wide card they sit above the dials.
- **Dials**: DRIVE (0…36 dB; 0–12 dB takes half the travel), BIAS (±100 %), TONE (±12 dB),
  MIX, OUTPUT; under them the dynamics row: DYNAMICS (±100 %), ATTACK (0.1–100 ms), RELEASE
  (10–1000 ms); no threshold dial (automatic). While DYNAMICS is not 0 the square shows the live
  threshold, the band's average level, as two dashed lines at ±T (the lit stretch reaching past
  them is when the dynamics move), and the curve is drawn times the live dynamics gain.
- Narrow cards (other bands): number badge, S and M, a transfer square of their own (lit where
  their music reaches; drag it as the wide one, which selects the band), DRIVE and MIX. The curve
  is the model's drawing, so no separate glyph.
- Wide card (the selected band of several, 600 px): badge, model icons, the band's range, S, M,
  power and remove (a cross in the text colour: Sat draws no danger role), the square, DRIVE
  large beside BIAS, TONE, MIX and OUTPUT in a 2×2 grid, the dynamics row of three below them
  (the bars 56 px tall to make room).

### 3.3 Footer
Oversampling (dropdown button: 1×, 2×, 4×, 8×, 16×; the help strip says each one's latency), the
latency readout, auto gain, Δ, bypass, MIX, PAN, OUTPUT, all as in Shape.

---


### 3.4 The tape deck's panel

The footer's TAPE button (two small reels, lit while the deck runs or the panel is open) opens
the deck over the cards, where the matrix opens; one of the two at a time, Esc or × closes it
(the deck keeps running). Power beside the title. A deck drawn from the machine: the reels turn
at the tape's speed (0.35 turns a second at 7.5 ips, scaled with the speed and the transport's
reported deviation); the tape runs over three heads, the playback head lit; the trace under them
is the transport's speed against nominal, block by block, ±0.3 % full height; the meter beside
it is the peak going into the tape against the operating level (0) and the maximum output level
(+10, above which it turns `warn`), falling 20 dB/s. Speed chips (3¾, 7½, 15, 30 ips) and NAB /
CCIR, then LEVEL (large), BIAS, WOW, FLUTTER, HISS. The help strip names each.

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
3. Tape and Transformer (done 2026-10-26): `mite_dsp::hysteresis` (see §1.3).
4. Harmonics probe and bars (done 2026-11-01).
5. Per-band stereo modes (done 2026-11-01).
6. Per-band dynamics (done 2026-11-03).
7. (done 2026-11-05) Modulation matrix, shared with Delay and Mod (BLUEPRINT §13), on the
   parameters frozen in slice 1; `env_src` unused (no sidechain input). Not yet heard.
8. Aliasing and THD report, CPU table, presets, validators (done 2026-11-03):
   `docs/dsp/sat-aliasing.md` (from `mite_dsp::sat::distortion`, regression-tested), the half-band's
   first stage vectorised (six Tube bands at 4×: 8.9 → 7.7 %), five presets that use dynamics,
   bands and stereo modes (every factory preset is played through the engine in a test), and
   clap-validator 34 passed, 0 failed (pluginval runs in CI).
