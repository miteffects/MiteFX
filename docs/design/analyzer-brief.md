# Mite Analyzer: design brief

Requested 2026-11-09 ("we need analyzer for master"; picked Spectrum, Loudness, Stereo and
Spectrogram; "rename it as analyzer" — BLUEPRINT §14 had it as Mite Meter). Accent: pewter
(`analyzer`).

## 1. What it is

A meter for the master bus. The audio passes through untouched (no latency, nulls). Loudness
and true peak are measured on the audio thread (exact whether the editor is open or not); the
editor reads the raw left and right streams for everything drawn from spectra and the scope.

## 2. Editor (1520×900)

Bigger than the other plugins (user, 2026-11-09: "make more room for everything it can be bigger, fonts are small and numbers are changing way too fast … so they don't shake"): readings 24 px (integrated 64 px), labels 11–12 px. The numbers are sampled every 250 ms and right-aligned with tabular digits, so a digit stays where it is; the graphs, bars and scope stay live.


- **Spectrum** (left, top): 8192-point FFT read at the shared analyzer points, 20 Hz–20 kHz,
  −96…−6 dB. View: the sum (L + R), Mid (lit) and Side, or Left (lit) and Right. Tilt Flat,
  3 or 4.5 dB/oct (pink noise level). Speed Fast/Normal/Slow (rise/fall 20/150, 60/400,
  200/1200 ms). Peak hold line (falls 3 dB/s). Hover: frequency, note and cents, level.
- **Spectrogram** (left, under the spectrum, the same frequency axis): the last 8 s, newest at
  the top, a row every 1/30 s, a magma-like heat scale (one hue could not show levels).
- **Loudness** (right, top): integrated (large; click to reset) against the target (±1 LU green,
  over it warned), momentary, short-term, range (EBU Tech 3342), the highest true peak (4×,
  red over −1 dBTP), PLR; the last minute of short-term (lit) and momentary (dim) with the
  target dashed.
- **Stereo** (right, bottom): vectorscope (mono vertical, auto-gained), correlation bar, balance,
  correlation per band (20–120, 120–500, 500–2k, 2k–6k, 6k–20k Hz; a band under 0 is red: it
  cancels in mono).
- **Footer:** TARGET (Streaming −14, Apple Music −16, EBU R128 −23, ATSC A/85 −24, Club −9),
  VIEW, TILT, SPEED, HOLD; RESET on the right.

- **Phase page** (2026-11-21, "focus on phase analysis" → "phase response of a chain"): the
  footer's SPECTRUM | PHASE switch. Put a Mite Analyzer before a chain and one after it (or feed
  the chain's input to the sidechain, "Reference"); the one after it shows the chain's
  magnitude, phase (wrapped, on top of its delay) and group delay (total, the delay dashed), each
  curve fading where the coherence says the two signals don't explain each other; the chain
  delay in ms and samples, polarity, coherence (mean and across the band); the impulse response
  around the delay. REFERENCE: Auto (the sidechain if it plays, else another instance), Sidechain,
  Analyzer N. The one before the chain says it is the reference. Footer: SMOOTHING (1/48–1/3 oct),
  AVERAGE (Fast 0.5 s, Normal 2 s, Slow 6 s, Long 20 s), RESET (measure again).

## 3. Parameters

`target`, `view`, `tilt`, `speed` (choices), `hold` (bool); `page`, `psmooth`, `pavg` (state 2,
the phase page; `page` stays out of presets and snapshots). None affects the sound.

## 4. Open

- A reference curve (tonal balance target, or a loaded track) on the spectrum.
- Longer loudness history (the engine keeps ~13 minutes; the editor shows the last minute).
- Spectrogram length/range controls; FFT size choice.
