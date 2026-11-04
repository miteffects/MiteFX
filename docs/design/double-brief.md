# Mite Double: design and build spec (in-house, 2026-11-04)

**From:** MiteFX engineering (user, 2026-11-04: "i want doubler plugin but kind of smart one … as
close as possible real guitar doubler. like you would double take a guitar … options of panning
source and wet, delay, chorus and other smart features"). Rivals: Waves Doubler, Soundtoys
MicroShift, Eventide H910-style ADT, Nugen/Polyverse "double tracker" plugins.

**Accent:** rose (tokens.json `double`, dark `#FBB5B1`, light `#5A2420`, hc `#FBB9B1`; new,
CIEDE2000 ≥ 20 from the ramp and the semantics in every theme).

## 1. What makes a double a double (`mite_dsp::doubler`)

A delay or a chorus makes a copy that moves the same way all the time; a second take does not.
The same part played again lands **each note** a little early or late, a few cents off, a little
louder or softer, brighter or duller, differently every note. So:

- **Notes.** An onset detector on the input's top (a 150 Hz highpass; a 0.5 ms / 10 ms envelope
  against a 40 ms one, ×2, over −50 dBFS, at most one a 60 ms) hears each picked note.
- **Per note, per double:** a timing offset (TIMING: its full spread, the mean of two uniforms so
  most notes land near the middle), a detune (PITCH, cents either way), a level (±2 dB × VARIATION)
  and a tone tilt (±2 dB × VARIATION). Deterministic random (a seed per double).
- **The switch before the note:** at the onset each double's reading crossfades (3 ms, equal
  power) from its old reading to the note's new one; both still read the previous note's tail
  (every reading is ≥ 8 ms), so the new attack arrives whole at its new time (tested: the attack
  correlates > 0.999 with the input at exactly DELAY + the note's offset).
- **Detune as playing:** within a note the reading changes at the rate that puts its pitch the
  note's cents off (`d(delay)/dt = 1 − 2^(cents/1200)`), easing off as it nears 6 ms of drift (a
  bent string settling); measured within 1.5 cents of the note's detune.
- **CHORUS:** smoothed random motion on top (up to 3 ms × DEPTH, at RATE).
- **TONE:** the doubles' tilt about 900 Hz (±6 dB): another guitar, another mic.
- **FOLLOW NOTES off:** new readings every 1.5…3 s instead (tape ADT, for parts without picked
  notes, pads).
- **Placement:** the input summed to mono is the source (never delayed: no latency), placed by
  SOURCE pan and level; each double by its own pan and level (constant power). Up to three doubles
  (quad tracking: the source and three).

0.38 % of a core with one double, 0.89 % with three (`examples/doubler_cpu`).

## 2. Main view (1280 × 760)

- **Stereo field** (top): pan across, level up (+6…−36 dB); the source puck (S) and a puck per
  double, each with a ring breathing at the level it plays; drag to place, wheel for level,
  double-click puts it back, right-click a double: reset or remove; dashed ghosts for doubles not
  playing (click: in). Under it the MONO strip: the output's L/R correlation over the last 300 ms
  (below 0 in warning: it would thin out summed to mono).
- **TIMING and PITCH lanes** (bottom left, 6 s): each double's reading (ms) and detune (cents),
  note by note, the spreads TIMING and PITCH shaded, the mean DELAY dashed, a tick at every note
  heard.
- **Dials:** TAKE (DELAY, TIMING, PITCH, VARIATION), CHORUS (DEPTH, RATE), DOUBLES (count pills
  1/2/3, NEW TAKE EVERY NOTE / EVERY FEW SECONDS), TONE.
- **Meters** IN, OUT L/R. **Footer:** Bypass (the input as it came; a mono input on both sides),
  OUTPUT.

120 fps in the preview with a riff running (scene 0.21 ms, render 0.58 ms median).

## 3. Parameters (`tests/golden/mite_double.params.txt`)

`delay`, `timing`, `pitch`, `variation`, `chorus`, `chorus_rate`, `tone`, `follow`, `voices`
(1, 2, 3), `source_pan`, `source_level`, `out_gain`, `bypass`, `v1…v3_{pan, level}` (defaults:
the first double hard right against the source hard left, the others at ±60 % and −3 dB).
Bypass is monitoring. State 1. Presets: Init, Tight Rhythm, Loose Take, Quad Tracked, Wide Lead,
Classic ADT.

## 4. Tests

`crates/mite-dsp/tests/doubler.rs`: the source alone is the input at its place; every note gets a
new reading within the spread (different note to note); each attack arrives whole at its note's
time; the detune is heard as the pitch; without notes the readings move every few seconds;
block-size invariance; click-free moves (pans, levels, count, tone, chorus, delay). The plugin:
bypass and a mono input, the source never late, no allocation, the manifest and golden state v1,
presets, the editor (dragging pucks with one undo step, ghosts, the menu, double-click, pills,
groups).

## 5. Slices

1. (done 2026-11-04) Everything above. Not yet heard by the user.
2. Ideas: per-double "player" character (a consistently early or late double), a pick-attack
   variation (transient shaping per note), formant-preserving detune for larger spreads, stereo
   input kept as stereo.
