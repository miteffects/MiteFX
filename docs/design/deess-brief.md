# Mite DeEss: design and build spec (in-house, 2026-11-05)

**From:** MiteFX engineering (user, 2026-11-05: "let's make de-esser"). Rival: FabFilter Pro-DS.
**Accent:** deep sky (tokens.json `deess`, already in the token set).

## 1. What it is (`mite_dsp::deess`)

- **Detection.** The trigger band (LO…HI, 5…12 kHz by default: two Butterworth highpass and two
  lowpass sections) and the whole input, each as RMS over 2 ms, linked (the louder side).
  - **RELATIVE** (default, the smart one): a sibilant is the band loud *against the voice*:
    `band − whole` (dB) over THRESHOLD (dB under the voice). The same de-essing whether the line is
    sung loud or whispered (tested: 24 dB apart, the reduction within 1 dB; absolute mode differs
    by > 6 dB). Under −60 dBFS nothing is a sibilant.
  - **ABSOLUTE:** the band's level over THRESHOLD (dBFS): cymbals, anything not a voice.
- **Reduction:** as far as the reading pokes over (4 dB soft knee), at most RANGE: in relative mode
  the band is brought back to the threshold (tested: a sibilant reduced > 6 dB, the vowel under it
  within 0.3 dB). Attack 0.5 ms. **Smart release:** RELEASE, divided by `1 + below / 5 dB` where
  `below` is how far the band sits under the threshold, so a vowel after a sibilant comes back
  clear (a plain 60 ms release left 3…8 dB on the next vowel's first 50…100 ms; now 1 dB at 20 ms,
  none by 60 ms — `examples/gr_trace`); a sibilant hovering at the threshold still lets go at the
  set pace. Detection mode and threshold glide (20 ms), so switching never jumps the reduction.
- **REDUCE:** SHELF (RBJ high shelf at LO: everything above, the classic split de-esser), BAND
  (three RBJ bells sharing the band, a third each, 1.2 × as wide: flat to about 2 dB across it and
  steep outside: 12 dB in the band takes < 2 dB at 15 kHz and < 1 dB at 2.5 kHz, tested; one bell
  over the band took 2.5 and 3 dB), WIDE (the whole signal). Redesigned every sample; at no
  reduction every filter is exactly the identity, so nothing is touched bit for bit (tested, every
  mode).
- **SPECTRAL** (2026-11-05, user: "let's do the spectral mode"): the input in frames (`Stft`,
  512 samples at 48 kHz scaled with the rate: 94 Hz a bin, a 2.7 ms hop), every bin of the trigger
  band against its own threshold: the broadband one spread over the band (RELATIVE: the voice's
  power, the whole frame, shared evenly by the band's bins, + THRESHOLD; ABSOLUTE: THRESHOLD
  shared evenly). So a sibilant spread evenly across the band comes down as BAND would, and a
  peaky one (a whistle, a lisp) only where it peaks (tested: a 7 kHz whistle down > 6 dB, the hiss
  beside it at 9.5 kHz < 1.5 dB, where BAND takes the hiss > 3 dB). Each bin as far as it pokes
  over (the knee), at most RANGE, at once, letting go at the smart release per bin; its reduction
  shared a little with its neighbours (0.2 / 0.6 / 0.2); the band's edges taper over a sixth of an
  octave. Transparent at rest (within 1e−5: the frames rebuild the input exactly but in f64), the
  vowel, 2.5 kHz and 15 kHz within 1 dB while a sibilant comes down > 6 dB. Its latency is the
  frame (10.7 ms at 48 kHz; the lookahead is built in): switching in and out goes behind the same
  dip as the lookahead, the host told. The spectral path runs in every mode (warm for the switch:
  +0.35 % of a core).
- **Lookahead** (on by default): the audio 3 ms behind the detector (the latency; reported, and a
  switch fades out, changes, fades in over 5 ms each): the reduction is in place when the sibilant
  arrives (tested: its first ms > 3 dB lower than without).
- **Listen:** the trigger band alone. **Bypass:** the input, delayed by the lookahead (switching
  keeps time). Both glide 10 ms.

CPU (48 kHz stereo, `examples/deess_cpu`): shelf 0.98 %, band 1.43 %, wide 0.84 %, spectral 0.83 %
of a core.

## 2. Main view (1280 × 760)

- **Spectrum** (top): the input dim, the output lit; the trigger band shaded with a handle at each
  edge (drag an edge; drag the band by its top to move both; double-click: back), labelled
  TRIGGER; the reduction's response now (the filter itself, lit) and at the full RANGE (dashed),
  on a 0…−24 dB scale.
- **History** (bottom left, 6 s): the voice (whole RMS, dim), the trigger band (lit), the
  threshold where it acts (dashed: in RELATIVE it rides on the voice); the threshold's grip at the
  right (drag, wheel, double-click). Over it the **heat-map** (96 px): where in frequency it
  turned down and how far (16 rows, 2…20 kHz, the brighter the deeper), every mode: SPECTRAL from
  its bins, the others from their filter's response at the reduction. In SPECTRAL the spectrum
  draws every bin's reduction live (dashed: RANGE across the band).
- **DETECT:** RELATIVE / ABSOLUTE / LOOKAHEAD pills, THRESH dial. **REDUCE:** SHELF / BAND / WIDE /
  SPECTRAL,
  RANGE, RELEASE. **REDUCTION:** a tall meter (down from the top, 0…24 dB) with the peak held
  1.5 s and its value.
- **Meters** IN, OUT. **Footer:** Listen, Bypass, OUTPUT.

120 fps in the preview with a voice running (scene 1.05 ms, render 0.57 ms median).

## 3. Parameters (`tests/golden/mite_deess.params.txt`)

`threshold`, `detection`, `range`, `release`, `lo`, `hi`, `reduce`, `lookahead`, `listen`,
`out_gain`, `bypass`. Listen and bypass are monitoring. State 2 (2026-11-05): REDUCE gained
Spectral; `migrate::v1_to_v2` re-normalises `reduce` in stored snapshots (a choice is stored
normalised over its count: version 1's Band, 0.5, would have read as Wide). Presets: Init, Gentle Vocal,
Strong Vocal, Bright Voice, Low Voice, Keep the Air (band), Cymbal Tamer (absolute).

## 4. Tests

`crates/mite-dsp/tests/deess.rs`: nothing touched without sibilants (bit for bit, every mode, at
the lookahead); a sibilant brought to the threshold, the vowel untouched; relative detection
level-independent, absolute not; each mode reduces what it says (air, presence, the whole);
the lookahead; block sizes; click-free moves (threshold, range, band, mode, lookahead,
detection). The plugin: bypass at the latency, mono, no allocation, manifest and golden state v1,
presets, the editor (band edges and band, an edge never passes the other, the threshold grip,
pills, double-click, groups).

## 5. Slices

1. (done 2026-11-05) Everything above. Not yet heard by the user.
2. (done 2026-11-05) The spectral mode and the heat-map; state 2 with its migration. Tests: transparent
   at rest with its latency, a sibilant down and the rest kept, only the peak of a peaky sibilant,
   block sizes, switching in and out without a click, the snapshot migration.
3. Mid/side detection.
