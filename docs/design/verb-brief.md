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
5. **Absorption (the decay EQ):** every line has a gain and one section per decay band (eight;
   none by default, user 2026-11-04: "fully flexible eq like eq plugin … dont make default
   bands"), so that a pass loses −60 dB × loop / T60(f): DECAY where no band acts, and each band
   multiplies the decay where it acts by its `mult` (rates add). A band is a bell, a low or high
   shelf (the decay below or above its frequency × mult) or a tilt (pivoting on its frequency),
   each one section of `mite_dsp::eq`'s matched design. Any frequency's loop gain is kept < 1 at
   design time (66 grid points plus every band's frequency, the highest refined by ternary search,
   a 1e−5 margin for broad maxima where bands overlap), so no setting can blow up. A new design is
   reached along a straight line over 32 samples, every sample; a band glides to nothing before
   it stops running, and before it changes shape. `decay_at`/`decay_curve` are the mean T60 the
   lines' filters give, from the same design: what the editor draws.
6. **Post:** width (M/S, 0…150 %), ducking (from THRESHOLD up 1 dB per dB, at most DEPTH, letting
   go over RELEASE, through a 3 ms one-pole), then the tone EQ: eight of Mite EQ's bands
   (`mite_dsp::eq::BandProcessor`: every shape, cuts at 6…96 dB/oct; smoothed, a shape change
   crossfaded), the same on both sides (so it commutes with the width). None by default; the
   fixed low and high cut are gone.
7. **Output:** mix against the dry input (no latency: mix 0 % and bypass give the input bit for
   bit), output gain, bypass (an ordinary parameter, never the host's).

**Freeze:** the input to the tank and the early field fade out over 50 ms while every line's
absorption fades to unity: the tail holds forever, and fades back to the set decay when released.

**Control rate:** designs every 32 samples, counted across blocks, from 20 ms-smoothed settings
(Size over 200 ms: a pitch glide, as the room's walls move); every gain within a step glides per
sample. Output identical for any block size. 3.8 % of a core at 48 kHz; 5.2 % with all sixteen
bands on.

## 2. Main view (1280 × 760)

- **The plot** (the hero, top, full width but the meters): frequency across (20 Hz…20 kHz), the
  input spectrum dim and the reverb's lit behind it (their own dB scale), and both EQs: the decay
  EQ (T60(f) in the accent from `decay_at`, decay time up on a log scale 0.1…30 s) and the tone
  EQ (the bands' summed analog response in the suite's curve colour, ±24 dB about the middle).
  Two glyph pills top left (a falling tail, a bell) pick which one is edited: it is drawn lit
  with its grid labels (seconds or dB), the other dim; grabbing one of the other's bands picks it.
  No grip until a band is made (user: "it doesnt have to have that middle default volume band"):
  DECAY is a dial in SPACE. The curve, the grips and the drag's solving use `DecayModel` (one line
  of the mean loop length, designed once per change; within 3 % of the 32-line mean, tested): the
  exact mean cost 16 ms a drag event and froze the UI (user: "ui freezes when i change decay eq");
  now about 0.5 ms an event and 0.6 ms a draw (`examples/drag_bench`). Frozen: the curve rises off the top (it is
  infinite) and the plot says so with the snowflake pill.
- **Bands** (eight per EQ, none by default): double-click the plot for a bell through the
  pointer (decay: the curve passes through it; tone: its gain); right-click the plot for a band of
  any shape there; right-click a band for its shape or to remove it; drag it (across: frequency;
  up and down: the decay through the pointer, or the gain; a tilt's decay doubles per 80 px);
  wheel for its width (a cut's slope); double-click removes it. A bell shows a bracket under it
  where it does half its dB.
- **History** (bottom left, 6 s): the input's peaks dim, the reverb lit; the tail is visible
  decaying. The duck threshold as a line with a grip (where the input crosses it, ducking
  starts) and the ducking from the top at the levels' scale.
- **Dials** (bottom right) grouped by what they do, each group a named panel (user: "group knobs
  per feature, right now everything is clustered"): SPACE (SIZE, DECAY, DIFFUSION, WIDTH, PREDELAY), MOD
  (DEPTH, RATE), EARLY (LEVEL, SIZE, SEND), DUCK (DEPTH, RELEASE; the threshold is on the
  history).
- **Meters** IN and OUT. **Footer:** Freeze, Bypass, latency, MIX, OUTPUT.

## 3. Parameters (frozen with the first release; `tests/golden/mite_verb.params.txt`)

State 2 (2026-11-04) appended `early_size`, `early_send` and the bells `d1…d4_{on,freq,mult,q}`.
State 3 (2026-11-04) made both EQs free bands: `d1…d8_{on,type,freq,mult,q}` (type: Bell, Low
Shelf, High Shelf, Tilt) and `e1…e8_{on,type,freq,gain,q,slope}` (Mite EQ's types and slopes);
`lo_mult`, `lo_freq`, `hi_mult`, `hi_freq`, `lo_cut` and `hi_cut` are gone. Its migration
(`migrate::v2_to_v3`) keeps an old state's sound: the low shelf becomes decay band 5, the high
shelf band 6, a cut that acted tone band 1 or 2 at 12 dB/oct; the snapshots are lifted too.

`predelay`, `size`, `decay`, `diffusion`, `early`, `mod_depth`, `mod_rate`, `width`, `freeze`,
`duck`, `duck_thresh`, `duck_release`, `mix`, `out_gain`, `bypass`, `early_size`, `early_send`, the
decay bands, the tone bands. Freeze and bypass are performance, not sound: never in presets or
snapshots.

## 4. Tests (theory, not taste)

- Mix 0 % and bypass null bit for bit; latency 0.
- The measured T60 (Schroeder integration, T20 × 3, octave bands at 125 Hz…8 kHz) matches
  `decay_at` within 10 %, at 44.1, 48 and 96 kHz and three sizes, with a low and a high shelf band;
  `decay_at` is DECAY × the low shelf's mult at DC, × the high shelf's at Nyquist, and DECAY within
  12 % between them.
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
   size-0 Tight Booth preset. 120 fps at 2× DPI. Not yet heard by the user.
2b. (done 2026-11-04, user: "verb should have fully flexible eq like eq plugin … multiple shapes
   of bands. dont make default bands … remove default low cut and high cuts"; "group knobs per
   feature") Both EQs as free bands on one plot (§1.5, §1.6, §2), state 3 with its migration, the
   dials in four named groups. Tests: every shape keeps the loop gain < 1, a shape change and the
   tone EQ are click-free, no band is on by default, the editor's bands and menus, the groups.
   120 fps, 1.7 ms a frame.
3. Convolution (IR import, worker-thread load, handover, partitioned convolution), then the
   IR-matched algorithm (analyse an IR's T60(f), ER pattern and density, fit the algorithm).
4. The decay spectrogram.
