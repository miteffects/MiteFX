# Mite Mod: design and build spec (in-house, 2026-11-05)

**From:** MiteFX engineering (user, 2026-11-05: "let's tackle mod plugin"; roster §13: chorus,
ensemble, flanger (through zero), phaser (2–24 stages), vibrato and rotary in one plugin, each
mode with its own view, Nova's LFO shapes; the modulation matrix later). Rivals: Soundtoys
PhaseMistress / Crystallizer-era mod boxes, Arturia's Chorus/Flanger/Phaser collection,
Kilohearts' modulation snapins, FabFilter-style display quality.

**Accent:** lilac (tokens.json `mod`, already in the roster's palette).

## 1. The engine (`mite_dsp::modulation`)

- **One LFO** (`shape_at`, a pure function of the total phase, shared with the editor): Sine,
  Triangle, Ramp Up, Ramp Down, Square, Random. Every shape is continuous: the ramps fall back
  along a half cosine over a tenth of the cycle, the square is `tanh(4 sin)`, Random glides
  (smoothstep) to a random height each cycle, on alternate sides of the middle so it never holds
  still (two near values in a row stalled it for a cycle). A shape change crossfades 50 ms. RATE
  0.02–20 Hz, or SYNC to the host's tempo (15 divisions, 4 bars … 1/32 with dotted and triplets),
  phase-locked to the song position (a relocation is nudged over blocks, never a jump; tested).
  SPREAD puts the right side's LFO up to half a cycle ahead (0…180°).
- **Clean readings:** every swept read is the doubler's 32-tap Kaiser sinc (a sliding read adds
  no roughness); Lagrange only under 15 samples (a flanger's feedback loop is shorter than the
  sinc's reach), crossfaded over 11…15 samples so the sweep never steps.
- **Chorus:** 1–4 voices, each its own delay (DELAY × 1, 1.23, 0.81, 1.41) and place in the cycle
  (0, ½, ¼, ¾), swept by up to half its delay (6 ms at most) × DEPTH, summed equal-power (a voice
  count change fades the voice in or out).
- **Ensemble:** three voices a third of a cycle apart, each swept by the LFO (1.6 ms × DEPTH) and
  a fast sine shimmer at 9.5 × the rate (0.35 ms × DEPTH): the string-machine sound.
- **Flanger:** MANUAL 0.1–5 ms swept ±2 octaves of delay × DEPTH, FEEDBACK ±95 % around the line.
  **Through zero:** the dry waits 5 ms + 16 samples (the latency, reported) so the wet can sweep
  from +MANUAL to −MANUAL (DEPTH 50 %: it just touches zero; 100 %: through); at zero the wet is
  the dry exactly (tested).
- **Phaser:** 2, 4, 6, 8, 12, 16 or 24 first-order allpasses, corners at CENTRE × 2^(±3 octaves ×
  DEPTH), FEEDBACK through one sample; a stage count change crossfades the chain's taps.
- **Vibrato:** all wet around a 5 ms centre (its latency, so the voice stays in time); DEPTH 100 %
  is 100 cents at the sine's steepest (the sweep from the rate: `(2^(c/1200) − 1) / (2π rate)`,
  at most 4.5 ms). Its reading is the pitch played, from the delay's slope (tested ±50 cents at
  50 %).
- **Sound:** MIX (vibrato ignores it), the wet's WIDTH (M/S, 0–200 %), LOW CUT and HIGH CUT
  (Butterworth on the wet, redesigned every sample while they glide).
- **Programs:** each mode (and the flanger twice: through zero has its own latency and line) keeps
  its state; a switch crossfades the two programs' outputs over 30 ms. Every continuous setting
  glides (20 ms; delays 120 ms, a fast delay change is a pitch blip; tone 80 ms).
- **For the editor:** `chorus_delay_ms`, `ensemble_delay_ms`, `flanger_tau_ms`, `phaser_fc`,
  `vibrato_amp_ms`, and the responses `flanger_response` / `phaser_response` (dry, wet, loop and
  tone), the same functions the engine plays (tested against tones through the engine within
  0.3 dB).

CPU (48 kHz stereo, `examples/mod_cpu`): chorus 1.1 % (2 voices) – 2.1 % (4), ensemble 1.8 %,
flanger 0.6 %, phaser 0.4 % (4 stages) – 0.8 % (24), vibrato 0.5 % of a core.

## 2. Main view (1280 × 760)

- **Mode tiles** (top): five, each a little picture of what it does (two waves, three waves, a
  comb, a few wide notches, a wobbling wave) and its name.
- **The mode's view** (880 × 298):
  - Chorus, ensemble: each voice's delay scrolling (left side solid, right dashed), each voice's
    reach shaded and its middle dashed; the span fits the voices with air around it; about three
    LFO cycles across (0.6–7.5 s). Drag up and down: DELAY; wheel: DEPTH; double-click: back.
  - Vibrato: the pitch it plays, in cents, the same way; drag: DEPTH.
  - Flanger, phaser: the response now (left solid, right dashed) over 20 Hz–20 kHz, two points a
    pixel (a comb is dense at the top), the span the sweep covers shaded (24 LFO positions,
    cached until a setting moves); a puck: across MANUAL's first notch (1 / 2·MANUAL) or CENTRE,
    up and down FEEDBACK, a dashed line where it anchors. Wheel: FEEDBACK; double-click: back.
- **The mode's own controls** (right): DELAY + VOICES 1–4 (chorus); DELAY (ensemble); MANUAL,
  FEEDBACK, THROUGH ZERO (flanger); CENTRE, FEEDBACK, STAGES (phaser); DEPTH (vibrato).
- **LFO** (bottom left): the shape over the cycle playing, the left side a dot riding on it, the
  right a ring SPREAD ahead; six shape buttons drawn by `shape_at` itself; SYNC and the division
  (a menu; picking one syncs; wheel steps); the rate it runs at (a cycle's length under 1 Hz;
  "waiting for the host's tempo" when synced without one).
- **MOTION:** RATE (a dashed ring with the division when synced), DEPTH, SPREAD. **SOUND:** MIX
  (a dashed WET ring in vibrato), WIDTH, LOW CUT, HIGH CUT.
- **Meters** IN, OUT L/R. **Footer:** Bypass (the input at the latency), OUTPUT.

118–119 fps in the preview with a riff running (scene 0.7 ms chorus, 1.1 ms phaser, 2.4 ms
flanger); a puck drag event plus its draw 2.4–2.6 ms (tested under 4 ms in release builds).

## 3. Parameters (`tests/golden/mite_mod.params.txt`)

`mode`, `rate`, `sync`, `division`, `shape`, `depth`, `spread`, `feedback`, `delay`, `voices`,
`manual`, `thru_zero`, `centre`, `stages`, `mix`, `width`, `low_cut`, `high_cut`, `out_gain`,
`bypass` (monitoring). State 1. Appending a mode (Rotary) changes `mode`'s snapshot
normalisation: it needs a migration then (the DeEss lesson). Presets: Init, Lush Chorus,
Eighties Chorus, String Ensemble, Jet Flanger, Tape Flange, Four Stage Swirl, Deep Phaser, Gentle
Vibrato, Seasick.

## 4. Tests

`crates/mite-dsp/tests/modulation.rs`: every shape continuous and in range; mix 0 is the input at
the latency (bit for bit, every mode); the latency is where an impulse comes out (through zero,
vibrato); through zero the wet meets the dry (0 dB at zero, the same comb either side, crossing
only from 50 %); the drawn flanger and phaser responses match tones through the engine; vibrato's
cents; block-size invariance (every mode); no click moving any setting or switching mode or
through zero; a synced LFO locks to the song position. The plugin: bypass at the latency with a
mono input, no allocation, the manifest and golden state v1, presets, the editor (tiles and each
mode's controls, the puck in one undo step, the time view drag and double-click, shapes, sync and
the division menu, groups, drag timing).

## 5. Slices

1. (done 2026-11-05) Everything above. Not yet heard by the user.
2. Rotary (horn and drum split at 800 Hz, each a Doppler delay plus amplitude, two mics, slow/fast
   with each rotor's own inertia; a top-down view of the rotors) — state 2 with the `mode`
   migration.
3. The modulation matrix (shared with Delay, Sat and Filter; deferred on Sat by the user).
