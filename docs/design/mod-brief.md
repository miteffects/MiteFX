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
- **Rotary** (slice 2): the mono sum split at 800 Hz (Linkwitz–Riley 4th order) into the horn
  and the drum; each read at 2 ms (the latency) ± its Doppler (horn 0.45 ms, drum 0.2 ms × DEPTH,
  nearest when facing a mic) and quieter facing away (horn 60 %, drum 40 % × DEPTH; the horn's
  highs over 2.5 kHz drop 60 % further: it beams); the horn's back off the cabinet wall 1.3 ms
  later at 0.3. Two mics at ±SPREAD/2 (0–180°). Speeds: Slow 0.83 / 0.67 Hz, Fast 6.7 / 5.8 Hz
  (horn / drum), Brake 0; each eases to its target over its own time (horn 0.7 s up, 1.1 s down;
  drum 3.2 / 4.2 s) × 4^(2·INERTIA − 1). BALANCE: a ±6 dB tilt, then the other rotor fades out
  towards the ends. It ignores the LFO (RATE, shape, sync).
- **Micro** (slice 2b): the left shifted DETUNE cents down, the right as many up (0–50 ct, the LFO
  swaying it by up to half × DEPTH), read at DELAY and 1.5 × DELAY. One tap per side slides
  through a 50 ms window at (1 − ratio) a sample; past the window's end a new tap starts at the
  other end, placed within ±7 ms where its last 5 ms best match the old tap's (normalised
  correlation; coarse then refined above 48 kHz), and the two crossfade over 10 ms (raised
  cosine). Two fixed taps always crossfading cancel where they meet out of phase (−2.9 dB on a
  1030 Hz tone, dropouts at worse frequencies); matched splices hold it within 0.14 dB. DETUNE 0
  is a clean delay.
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
flanger 0.6 %, phaser 0.4 % (4 stages) – 0.8 % (24), vibrato 0.5 %, rotary 1.6 %, micro 0.66 % of a core.

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
- **Rotary's view:** the cabinet from above: the horn (its live bell lit, the dummy dim, its beam
  shaded) and the drum (its scoop lit) turning past the two mics (front is down; L filled, R a
  ring); turning fast, the beam and scoop smear wider. A click: slow ⇄ fast. The LFO strip shows
  the rotors' speeds over 8 s instead (horn solid, drum dashed, slow and fast as faint lines);
  RATE becomes a plaque with the speed.
- **The mode's own controls** (right): DELAY + VOICES 1–4 (chorus); DELAY (ensemble); MANUAL,
  FEEDBACK, THROUGH ZERO (flanger); CENTRE, FEEDBACK, STAGES (phaser); DEPTH (vibrato); BALANCE, INERTIA + SLOW/FAST/BRAKE (rotary); DETUNE, DELAY (micro: its view is
  each side's cents scrolling, a drag sets DETUNE).
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
`bypass` (monitoring), then `rotor`, `balance`, `inertia` (state 2). State 2 appended Rotary to
`mode`: the snapshots store it normalised, so `migrate::v1_to_v2` re-normalises them (× 4/5);
golden v1 (with its expected migration) and v2. State 3 appended Micro to `mode` (snapshots × 5/6,
`migrate::v2_to_v3`) and `detune`; golden v2 expected and v3. Presets: Init, Lush Chorus,
Eighties Chorus, String Ensemble, Jet Flanger, Tape Flange, Four Stage Swirl, Deep Phaser, Gentle
Vibrato, Seasick, Rotary Chorale, Rotary Tremolo, Warm Cabinet, Micro Shift, Thicken, Wide Detune.

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
2. (done 2026-11-05) Rotary, above: tested for latency (impulse), each rotor's own inertia
   (the horn near fast in 2 s, the drum not; braking to a stop), the horn's swing at DEPTH 100 %
   and none at 0, balance, clicks, block sizes, no allocation; the editor's pills, cabinet click
   and a 0.6 ms draw. Not yet heard by the user.
   2b. (done 2026-11-05) Micro pitch shift, above: tested for each side's pitch (within 0.3 Hz at 9
   and 30 ct), the level through splices (within 0.5 dB), clicks, block sizes, no allocation; the
   editor's dials and drag. Not yet heard by the user.
3. The modulation matrix (shared with Delay, Sat and Filter; deferred on Sat by the user).
