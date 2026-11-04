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
- **The switch before the note:** at the onset each double's reading crossfades from its old
  reading to the note's new one, for as long as there is room before the note reaches either
  (3…8 ms; equal gain, since the two readings are the same signal a little apart and an
  equal-power fade bumped the level); both still read the previous note's tail (every reading is
  ≥ 8 ms), so the new attack arrives whole at its new time (tested: the attack correlates > 0.999
  with the input at exactly DELAY + the note's offset).
- **Clean readings** (2026-11-05, user: "can we make it somehow less distorted"): the readings
  slide (the detune, the chorus), and cubic Lagrange's treble rose and fell with the fraction as
  it slid, a ~200 Hz roughness on everything bright: on a 10 kHz tone the junk was only 34 dB under
  it (`examples/doubler_purity`). Now a 32-tap Kaiser-windowed sinc (β 8, cut at 0.92 of Nyquist,
  512 tabulated fractions): 64 dB under it at 2, 6 and 10 kHz alike.
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

- **Auto gain** (2026-11-05, user: "give it auto gain too"; on by default; reworked the same day:
  "auto gain doesnt work at least on takes 2 and 3" — it turned the whole output down, the source
  with it, so more takes still read as louder doubles). Two parts, both from the settings (no
  measuring, no pumping): the doubles trimmed together to the loudest one's power (`Σ double²` →
  `max double²`: a second and third take add width and weight, not level), then the whole kept as
  loud as the input on both sides (`2 / (source² + max double²)` in power, −12…+6 dB). Measured on
  a riff: −0.26, −0.09, +0.05 dB with 1, 2, 3 doubles (`examples/mono_check`); tested within
  0.6 dB on noise for six placements and counts.
- **Mono bass** (2026-11-05, user: "i get a lot of correlation issues"; on by default, 150 Hz, 40…400):
  measured, the doubles' own lows cancelled the source's by 5–6 dB at moments in the 63–250 Hz
  octaves summed to mono (a delayed copy's low end is in and out of phase note by note). Now
  under the split the output is the source's lows alone, on both sides as the input itself (at the
  source's level); the source and the doubles keep only their highs (Linkwitz–Riley 4th order, so
  the source's side rebuilds it exactly: an allpass). The low octave sums in phase (+2.7 dB, as a
  mono signal), and more takes don't pile up bass. The doubles' highpass sits at 0.7 × the split:
  the source's lows and a double's highs add as powers and two LR halves at one frequency leave a
  3 dB hole (it cost 0.6 dB overall), the overlap fills it. A first version lent each double the
  source's lows instead: three doubles stacked them +1.2 dB.

0.43 % of a core with one double, 1.10 % with three (`examples/doubler_cpu`).

## 2. Main view (1280 × 760)

- **Stereo field** (top): pan across, level up (+6…−36 dB); the source puck (S) and a puck per
  double, each with a ring breathing at the level it plays; drag to place, wheel for level,
  double-click puts it back, right-click a double: reset or remove; dashed ghosts for doubles not
  playing (click: in). Under it the MONO strip: the output's L/R correlation over the last 300 ms
  (below 0 in warning: it would thin out summed to mono), and beside it the MONO BASS pill (click:
  on/off; wheel: its frequency, also the MONO dial in TONE).
- **TIMING and PITCH lanes** (bottom left, 6 s): each double's reading (ms) and detune (cents),
  note by note, the spreads TIMING and PITCH shaded, the mean DELAY dashed, a tick at every note
  heard.
- **Dials:** TAKE (DELAY, TIMING, PITCH, VARIATION), CHORUS (DEPTH, RATE), DOUBLES (count pills
  1/2/3, NEW TAKE EVERY NOTE / EVERY FEW SECONDS), TONE.
- **Meters** IN, OUT L/R. **Footer:** Auto gain, Bypass (the input as it came; a mono input on both sides),
  OUTPUT.

120 fps in the preview with a riff running (scene 0.21 ms, render 0.58 ms median).

## 3. Parameters (`tests/golden/mite_double.params.txt`)

`delay`, `timing`, `pitch`, `variation`, `chorus`, `chorus_rate`, `tone`, `follow`, `voices`
(1, 2, 3), `source_pan`, `source_level`, `out_gain`, `bypass`, `v1…v3_{pan, level}`, `auto_gain`
`mono_bass`, `bass_freq` (appended 2026-11-05, before any release: state 1 re-blessed) (defaults:
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

1. (done 2026-11-04) Everything above. The user: "sounds actually ok".
1b. (done 2026-11-05) Auto gain; the sinc reading (less distortion); the fade by the room there is.
1c. (done 2026-11-05) Auto gain trims the doubles as a group; mono bass.
2. Ideas: per-double "player" character (a consistently early or late double), a pick-attack
   variation (transient shaping per note), formant-preserving detune for larger spreads, stereo
   input kept as stereo.
