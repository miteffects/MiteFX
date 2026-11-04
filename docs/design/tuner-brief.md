# Mite Tuner: design and build spec (in-house, 2026-11-04)

**From:** MiteFX engineering, designed here (user, 2026-11-04: "simple tuner plugin for guitars.
make it in a way it's big. keep the styling"). Rival: the tuners in every amp sim and pedalboard.

**Accent:** seafoam (tokens.json `tuner`, dark `#C8FCF4`, light `#0E4F48`, hc `#A5FDF1`; new,
CIEDE2000 ≥ 21 from the ramp and the semantics in every theme, checked by
`accents_clear_the_ramp_and_semantics`).

## 1. What it is

- **Audio:** passes untouched (bit for bit, no latency), or muted (faded over 10 ms). The mono sum
  goes to a tap; the input's peaks to the meter.
- **Pitch** (`mite_dsp::pitch`, in the editor, every 20 ms): the McLeod pitch method. The NSDF of a
  ~90 ms window (8192 samples at 48 kHz), from its autocorrelation by FFT; the first key maximum
  within 0.9 of the highest (the fundamental, not a loud second harmonic); a parabola, then a
  golden-section search on the NSDF over a windowed-sinc interpolation of the window. A clarity
  under 0.8 or a level under −60 dBFS is no pitch. 0.85 ms a detection at 48 kHz.
- **Accuracy** (tested, a plucked string with decaying harmonics and noise, 44.1/48/96 kHz):
  within 0.2 cent from 80 Hz to 1 kHz, 0.5 cent on a bass's B0 and E1; a second harmonic louder
  than the first still reads the note.
- **Note and cents** against A4 = REFERENCE (420…460 Hz, 440 by default); the same note eases
  toward each new reading (×0.45 a step), a new note jumps; a reading holds 1.5 s after the sound
  goes, fading.

## 2. Main view (1280 × 760)

- **Strings row** (top): the TUNING pill (opens the list) and the tuning's strings as pills
  (note and octave); the string nearest the note heard lights, filled while in tune.
- **The note** (huge, 220 pt, the middle): letter, sharp, octave; in the accent while in tune
  (±2 cents), else white; a dash when nothing is heard. An arrow each side, both pointing to the
  middle: the one on the side the string is on lights (left: flat, tune up; right: sharp, tune
  down); both while in tune.
- **The cents strip:** −50…+50, ticks every 5, the ±2 zone (lit while in tune), a glowing needle;
  over it the frequency, the cents, and the note's exact frequency.
- **Trace:** the last six seconds of cents (gaps where nothing was heard), ±25 lines, the zone.
- **Meter:** IN. **Footer:** Mute, the A4 dial.

120 fps in the preview (six strings plucked in turn), 0.09 ms scene, 0.56 ms render (median).

## 3. Parameters (`tests/golden/mite_tuner.params.txt`)

`reference` (Hz), `tuning` (Chromatic, Standard, Drop D, Half Step Down, D Standard, DADGAD,
Open G, 7-String, Bass), `mute` (monitoring: never in presets or snapshots). State 1.

## 4. Slices

1. (done 2026-11-04) Everything above; tests: the detector (accuracy, octave errors, silence and
   noise, notes and cents), the engine (passes untouched, mute fades), no allocation, the
   manifest and golden state v1, the editor against a real tap (note and cents, the reference,
   sharps, Drop D's strings, silence, the tuning list). Not yet tried by the user.
