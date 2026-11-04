# Mite Verb: design and build spec (in-house, 2026-11-04)

**From:** MiteFX engineering, designed here, as Sat, Limit, Gate and Multiband. Rival: FabFilter
Pro-R 2. The suite's bar: every pixel from the DSP, direct manipulation, no words in the display,
no corners, honest lines, nothing glowing outside its plot (`DrawCx::begin_clip`).

**Accent:** ice (tokens.json `verb`, dark `#BADBFE`, light `#385573`, hc `#73B6FA`; already in the
token set and checked by `accents_clear_the_ramp_and_semantics`).

**Scope of the baseline (BLUEPRINT §6.5):** a hybrid algorithmic reverb (input diffusion, early
reflections, a modulated 16-line FDN), Pro-R's decay-rate idea (decay time per frequency), a post
EQ (low and high cut on the reverb), width, freeze and ducking. Convolution with IR import, the
IR-matched algorithm, the full decay-rate EQ (more bands) and the decay spectrogram come later;
their parameters are appended (state 2 onwards).

---

## 1. What it is (`mite_dsp::reverb`)

**A room, and only a room** (user, 2026-11-04: "room sound only, no plates or springs", "the best
sound there is", "sound is critical on size 0"; the controls modelled on Valhalla Room's). The
first engine (16 lines, line-end outputs, sine modulation) "sounded cheap"; measured, it was:
`mite_dsp::reverb_quality` (and `examples/verb_quality`) puts numbers on what a cheap reverb
does, and every choice below was made against them:

- **Periodicity:** the tail's strongest normalised autocorrelation at 0.5…60 ms (flutter, a
  metallic pitch). Gaussian noise: 0.024.
- **Peakiness:** spectral bins over 10 dB above their third-octave mean (ringing modes). Noise: 0.
- **Echo density** (Abel & Huang) and the L/R correlation.

| (flat decay, over 150 Hz) | periodicity | peaky | L/R ρ |
|---|---|---|---|
| first engine, size 0, 2 s | 0.146 | 0.84 % | +0.04 |
| first engine, size 50, 2 s | 0.083 | 0.10 % | −0.01 |
| now, size 0, 2 s | 0.040 | 0.18 % | −0.01 |
| now, size 50, 2 s | 0.032 | 0.03 % | +0.01 |
| now, size 100, 4 s | 0.036 | 0 % | +0.01 |

What it took (each measured; the dead ends are worth keeping):

1. **The outputs read inside the lines, once per line per side.** Reading the line ends made each
   side a fixed combination of what the matrix had just mixed: a pass later it is one line, two
   passes later itself (H·H = I), an echo at each loop length. Read at 30–75 % into each main
   delay, a recurrence lands on N² lags instead of N. Two reads of one line echo at their gap.
   The two sides' vectors are orthogonal (the first hand-typed "random" ones correlated at −0.35).
2. **The mixing matrix is P·H·D** (random signs, the Hadamard transform, a permutation): lossless
   and dense like H, not its own inverse.
3. **32 lines.** A small room's modes are the problem: at size 0 sixteen 6–16 ms lines give one
   mode per 4 Hz, each about 1 Hz wide at a 2 s decay; they do not overlap, and isolated modes
   ring (metallic). Longer allpasses in the loop add modal density but stretch the echo pattern
   (the room sounds bigger, and the bass decay goes off: an allpass's group delay swings from
   0.3× to 3.4× its length); more lines of the same lengths add both. 64 lines measured better
   still (peaky 0.07 % at size 0) at 6.9 % of a core; 32 cost 3.9 %.
4. **Two short allpasses inside every line** (1.1–2.3 ms and 0.5–0.9 ms, ±g, g = 0.3…0.65 with
   DIFFUSION): every pass is smeared. Short so the loop's group delay stays even down to the bass
   (2.5–4 ms ones put the 125 Hz decay 11 % off).
5. **Smoothed random modulation** per line (smoothstep between random values, each move 0.6–1.4
   periods), never periodic, depth up to 1.6 ms × MOD (at most a fifth of the main delay).
6. **The early field does not feed the tail by default** (EARLY SEND 0 %): fed in, its 24-tap
   pattern colours the whole tail (a product of two random spectra: 0.3–0.6 % peaky bins).

Signal flow per channel (mono runs the left path only):

1. **Pre-delay** 0…500 ms. A change crossfades between two read heads over 30 ms (a glide would
   be a pitch sweep: 500 ms in 20 ms is 25 times the pitch).
2. **Early field:** 24 reflections a side off the pre-delay line within EARLY SIZE (2…200 ms),
   denser as they come later (their times follow the cube root of an even spread, as a room's
   image sources grow with the square of time), falling off with distance, random in sign, a third
   from the other side, each side at unit power; two short allpasses blur them. EARLY sets their
   level, EARLY SEND how much feeds the tail.
3. **Input diffusion:** four Schroeder allpasses per side (1.3…5.3 ms), coefficient
   0.75 × DIFFUSION.
4. **The tank:** 32 lines, main delays 47…123 ms at the largest size spread geometrically
   (jittered), scaled by Size from 13 % (6–16 ms: a small room) along an exponential, each read
   with first-order allpass interpolation (lossless: cubic Lagrange in the loop shortened the highs
   by 13 % and leaked a frozen tail) through its two allpasses, mixed by P·H·D. The topology is
   drawn once from a fixed seed (`Topology`).
5. **Absorption (the decay EQ):** every line has a gain, a low and a high RBJ shelf and four RBJ
   bells, so that a pass loses −60 dB × loop / T60(f): DECAY at mid, × LOW at DC, × HIGH at
   Nyquist, and each bell multiplies the decay around its frequency (rates add). Any frequency's
   loop gain is kept < 1 at design time (34 grid points plus every section's frequency, the highest
   refined by ternary search), so no setting can blow up. A new design is reached along a straight
   line over 32 samples, every sample (stepping it was heard on a fast HIGH move); a bell glides to
   nothing before it stops running. `decay_at`/`decay_curve` are the mean T60 the lines' filters
   give, from the same design: what the editor draws.
6. **Post:** low cut and high cut on the reverb (second-order Butterworth), width (M/S, 0…150 %),
   ducking (from THRESHOLD up 1 dB per dB, at most DEPTH, letting go over RELEASE, through a 3 ms
   one-pole).
7. **Output:** mix against the dry input (no latency: mix 0 % and bypass give the input bit for
   bit), output gain, bypass (an ordinary parameter, never the host's).

**Freeze:** the input to the tank and the early field fade out over 50 ms while every line's
absorption fades to unity: the tail holds forever, and fades back to the set decay when released.

**Control rate:** designs every 32 samples, counted across blocks, from 20 ms-smoothed settings
(Size over 200 ms: a pitch glide, as the room's walls move); every gain within a step glides per
sample. Output identical for any block size. 3.9 % of a core at 48 kHz.

## 2. Main view (1280 × 760)

- **Decay plot** (the hero, top, full width but the meters): frequency across (20 Hz…20 kHz),
  decay time up on a log scale (0.1…30 s). The input spectrum dim and the reverb's spectrum lit
  behind it (their own dB scale), the T60(f) curve in the accent from `decay_at`, three grips:
  LOW (x = low frequency, y = its decay), MID (y = DECAY, between them), HIGH (x = high frequency,
  y = its decay). The post EQ's cuts as two vertical grips on the plot's floor, the region they
  remove shaded. Frozen: the curve rises off the top (it is infinite) and the plot says so with
  the snowflake pill.
- **History** (bottom left, 6 s): the input's peaks dim, the reverb lit; the tail is visible
  decaying. The duck threshold as a line with a grip (where the input crosses it, ducking
  starts) and the ducking from the top at the levels' scale.
- **Dials** (bottom right): SIZE, PREDELAY, DIFFUSION, WIDTH, MOD, RATE / EARLY, E.SIZE, SEND,
  DUCK, RELEASE.
- **Decay bells** (up to four) on the curve: double-click the plot to make one through the
  pointer, drag it (across: frequency; up and down: the curve passes through the pointer), wheel
  for its width (a bracket under it shows where it does half its dB), double-click to remove it.
- **Meters** IN and OUT. **Footer:** Freeze, Bypass, latency, MIX, OUTPUT.

## 3. Parameters (frozen with the first release; `tests/golden/mite_verb.params.txt`)

State 2 (2026-11-04) appended `early_size`, `early_send` and the bells `d1…d4_{on,freq,mult,q}`.


`predelay`, `size`, `decay`, `lo_mult`, `lo_freq`, `hi_mult`, `hi_freq`, `diffusion`, `early`,
`mod_depth`, `mod_rate`, `width`, `lo_cut`, `hi_cut`, `freeze`, `duck`, `duck_thresh`,
`duck_release`, `mix`, `out_gain`, `bypass`. Freeze and bypass are performance, not sound: never
in presets or snapshots.

## 4. Tests (theory, not taste)

- Mix 0 % and bypass null bit for bit; latency 0.
- The measured T60 (Schroeder integration, T20 × 3, octave bands at 125 Hz…8 kHz) matches
  `decay_at` within 10 %, at 44.1, 48 and 96 kHz and three sizes; `decay_at` is DECAY × LOW at
  DC, DECAY × HIGH at Nyquist, and DECAY within 12 % between the shelves.
- Stability: the loop gain < 1 at every extreme setting; 60 s of noise never grows.
- Freeze holds the tail's energy within 1 dB over 10 s, and releases to the set decay.
- Echo density: after 80 ms the impulse response's kurtosis is near Gaussian (< 4).
- Stereo: at width 100 % the two sides' tails are decorrelated (|ρ| < 0.3); at 0 % identical.
- Every continuous parameter swept fast is click-free (no step larger than the signal's own).
- Output identical for every block size; no allocation in `process`.

## 5. Slices

1. (done 2026-11-04) `mite_dsp::reverb` with the tests above (`crates/mite-dsp/tests/reverb.rs`);
   the plugin with every parameter; the editor (decay plot with grips and spectra, history with
   the duck threshold, dials, meters, footer, the snowflake toggle); six presets; golden manifest
   and state v1. The grips sit on the curve: MID scales the decay so the curve passes through
   the pointer; LOW and HIGH move their frequency across and solve their multiplier (bisection)
   so the curve passes through the pointer there, as far as 0.25…4× (0.1…4× high) reach; each
   grip is one undo step. Measured: 1.4 % of a core at 48 kHz (`mite-dsp/examples/verb_level`;
   the wet level within 3 dB of the input from a small room to a 15 s hall); the preview at 2×
   DPI holds 120 fps (scene 0.44 ms, render 0.62 ms median); clap-validator 34/0.
2. (done 2026-11-04, user: "sounds cheap", "advanced decay EQ", "steal Valhalla's sound, room
   only", "size 0 is critical") The room engine of §1 against the quality metrics (now tests:
   `the_tail_is_smooth_at_every_size`, `a_decay_bell_changes_the_decay_where_it_sits`, the band's
   measured T60 against `decay_at` folded across the octave as the measurement folds it); the
   decay EQ's four bells; Valhalla Room's early controls (EARLY SIZE, EARLY SEND); state 2; a
   size-0 Tight Booth preset. 120 fps at 2× DPI. Not yet heard by the user. Next: the post EQ as
   bands on the plot.
3. Convolution (IR import, worker-thread load, handover, partitioned convolution), then the
   IR-matched algorithm (analyse an IR's T60(f), ER pattern and density, fit the algorithm).
4. The decay spectrogram.
