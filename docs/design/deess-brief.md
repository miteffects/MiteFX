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
- **Lookahead** (on by default): the audio 3 ms behind the detector (the latency; reported, and a
  switch fades out, changes, fades in over 5 ms each): the reduction is in place when the sibilant
  arrives (tested: its first ms > 3 dB lower than without).
- **Listen:** the trigger band alone. **Bypass:** the input, delayed by the lookahead (switching
  keeps time). Both glide 10 ms.

CPU (48 kHz stereo, `examples/deess_cpu`): shelf 0.65 %, band 1.05 %, wide 0.46 % of a core.

## 2. Main view (1280 × 760)

- **Spectrum** (top): the input dim, the output lit; the trigger band shaded with a handle at each
  edge (drag an edge; drag the band by its top to move both; double-click: back), labelled
  TRIGGER; the reduction's response now (the filter itself, lit) and at the full RANGE (dashed),
  on a 0…−24 dB scale.
- **History** (bottom left, 6 s): the voice (whole RMS, dim), the trigger band (lit), the
  threshold where it acts (dashed: in RELATIVE it rides on the voice), the reduction from the top;
  the threshold's grip at the right (drag, wheel, double-click).
- **DETECT:** RELATIVE / ABSOLUTE / LOOKAHEAD pills, THRESH dial. **REDUCE:** SHELF / BAND / WIDE,
  RANGE, RELEASE. **REDUCTION:** a tall meter (down from the top, 0…24 dB) with the peak held
  1.5 s and its value.
- **Meters** IN, OUT. **Footer:** Listen, Bypass, OUTPUT.

120 fps in the preview with a voice running (scene 1.05 ms, render 0.57 ms median).

## 3. Parameters (`tests/golden/mite_deess.params.txt`)

`threshold`, `detection`, `range`, `release`, `lo`, `hi`, `reduce`, `lookahead`, `listen`,
`out_gain`, `bypass`. Listen and bypass are monitoring. State 1. Presets: Init, Gentle Vocal,
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
2. The spectral mode (only the offending bins, soothe-like, `mite_dsp::spectral`) and a detection
   heat-map; mid/side detection.
