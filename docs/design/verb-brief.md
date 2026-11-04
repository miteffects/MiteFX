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

Signal flow per channel (mono runs the left path only):

1. **Pre-delay** 0…500 ms. A change crossfades between two read heads over 30 ms (a glide would
   be a pitch sweep: 500 ms in 20 ms is 25 times the pitch).
2. **Early reflections:** ten taps per side off the pre-delay line, 7…90 ms at the largest size,
   scaled by Size, alternating signs, gains falling off, each side's taps at different times (a
   room is never symmetric). EARLY sets their level (0…100 %).
3. **Input diffusion:** four Schroeder allpasses per side (1.3…5.3 ms), coefficient
   0.75 × DIFFUSION: 0 % leaves the transient sharp, 100 % smears it into a wash.
4. **Late reverb:** 16 delay lines, 61…163 ms at the largest size, scaled by Size from 7 % (a
   small room, 4…11 ms) to 100 % (a hall) along an exponential, under a 16 × 16 Hadamard feedback
   matrix (orthogonal, so the loop is lossless before its absorption). Each line's read position
   is modulated by its own sine (rates spread ±30 % around RATE, phases spread), depth up to
   1.2 ms × MOD (at most a tenth of the shortest line), read with first-order allpass
   interpolation: lossless at every frequency, so the decay is the absorption's alone (cubic
   Lagrange in the loop shortened the highs by 13 % in a small room and leaked a frozen tail). The left
   input feeds the even lines and the right the odd ones, with signs; the outputs are two
   orthogonal sign patterns over all 16 lines.
5. **Absorption (decay per frequency):** every line has a gain and two second-order shelves
   (RBJ, slope 1: monotone, transitions centred on LOW FREQ and HIGH FREQ; first-order ones
   were too broad: 13 % off at mid) so that a pass round the loop
   loses exactly −60 dB × length / T60(f): T60 = DECAY at mid, DECAY × LOW at DC, DECAY × HIGH
   at Nyquist (each pass loses at most 100 dB). Any frequency's loop gain is checked < 1 at
   design time (a 34-point grid, its highest point refined by ternary search; the gain scaled if
   not), so no setting can blow up, crossed shelves included. `decay_at(settings, f)` (what the editor draws) is the
   mean T60 the lines' filters give at `f`, from the same design.
6. **Post:** low cut and high cut on the reverb (second-order Butterworth), width (M/S, 0…150 %),
   ducking (the reverb turned down by the dry input: from THRESHOLD up 1 dB per dB, at most
   DEPTH, letting go over RELEASE).
7. **Output:** mix against the dry input (no latency, so mix 0 % and bypass give the input bit
   for bit), output gain, bypass (an ordinary parameter, never the host's).

**Freeze:** the input to the tank and the early reflections fade out over 50 ms while every
line's absorption fades to unity: the tail holds forever, unchanged, and fades back to the set
decay when released.

**Control rate:** the absorption and lengths are designed every 32 samples, counted across
blocks, from 20 ms-smoothed settings (Size glides over 200 ms: a pitch glide, as the room's walls
move); the ducking gain goes through a 3 ms one-pole so its ramps have no corners. Output
identical for any block size.

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
- **Dials** (bottom right): SIZE, PREDELAY, DIFFUSION, EARLY, WIDTH / MOD, RATE, DUCK, RELEASE.
- **Meters** IN and OUT. **Footer:** Freeze, Bypass, latency, MIX, OUTPUT.

## 3. Parameters (frozen with the first release; `tests/golden/mite_verb.params.txt`)

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
2. The full decay-rate EQ (more bands, any shape) and the post EQ as bands on the plot.
3. Convolution (IR import, worker-thread load, handover, partitioned convolution), then the
   IR-matched algorithm (analyse an IR's T60(f), ER pattern and density, fit the algorithm).
4. The decay spectrogram.
