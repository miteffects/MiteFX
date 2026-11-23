# Mite Limit against FabFilter Pro-L 2

Pro-L 2 is the limiter Mite Limit has to beat (BLUEPRINT §6.4, docs/design/limit-brief.md). This
page lists what each one has, then measures what both do. The bar is to be as good as Pro-L 2 or
better on each.

FabFilter's side comes from the plugin itself (its parameter list as REAPER reads it, and its
behaviour in the renders below). Ours comes from the code as of the date of the measured section.

## Features

| Feature | Pro-L 2 | Mite Limit |
|---|---|---|
| Styles | 8: Transparent, Punchy, Dynamic, Allround, Aggressive, Modern, Bus, Safe | 5 (Transparent, Punch, Dense, Bus, Safe) **plus Custom** (the style's numbers by hand) |
| Gain, ceiling | gain; output level in dB or dBTP | gain 0–30 dB; ceiling −30…0 dB |
| Lookahead, attack, release | yes | yes; lookahead 0–10 ms (the latency follows it; 0 is an honest clipper) |
| Slow envelope share | per style | **Slow dial** in Custom |
| Channel links | transients, release, centre and LFE (surround) | transients, release |
| True-peak limiting | yes | yes, with a guard after the down filter (output −1.000 dBTP) |
| Oversampling | up to 32× | up to 16× |
| Clipper stage | no | **Hard, Soft or Round, before the limiter, with its own level** |
| Transient protection | no | **three-band pre-stage**: a band's hit ducks only its band |
| Unity gain | yes (subtracts the gain) | **yes, measured**: the output matched to the input's short-term loudness |
| Audition limiting (Delta) | yes | yes |
| Platform preview | loudness targets on the meter | **targets and a preview**: hear what a platform's normalisation does |
| Dither, noise shaping | yes | 24, 20, 16 bit, shaping Off / Light / Strong |
| DC filter | yes | yes |
| Loudness meters | short-term, integrated, range, true peak | momentary, short-term, integrated, range, true peak, PLR |
| History | level and gain display | honest true-peak history, the gain shape strip (a test hit through the limiter's own code) |
| External side chain | yes | no |
| Surround | yes | no |
| Undo / redo, A/B | yes | yes; **snapshots A–D** |
| Resizing | yes | fixed size |

### Gaps to close (Pro-L 2 has, Mite Limit has not)

Oversampling at 32×; more styles (Dynamic, Allround, Modern); an external side chain; surround
with centre and LFE links; resizing.

### Ahead of Pro-L 2

The clipper stage, transient protection, a unity gain that measures instead of guessing, the
platform preview, the gain-shape strip, snapshots A–D.

### What the measurements say (2026-11-23)

- **True peak:** on the standard's meter Mite Limit holds the ceiling within 0.004 dB on every
  signal, with and without oversampling; Pro-L 2 goes up to 0.29 dB over without oversampling
  (clipped noise) and 0.04 dB at 4×. On an ideal 32× reconstruction both read higher (content up
  to Nyquist the standard filters out), ours lower than Pro-L 2's on all but one row.
- **Transparency:** under the ceiling Mite Limit nulls bit for bit (−155 dB is the float floor);
  Pro-L 2 leaves a −19 dB residual without oversampling (it changes the signal even when it
  does not limit). Both line up after delay compensation; ours reports less latency (226 against
  562 samples at 4×).
- **Distortion under steady limiting:** Transparent is 20 dB cleaner than Pro-L 2's (−46 against
  −26 dB on 8 dB of limiting on a 50 Hz tone). Safe is not: Pro-L 2's Safe settles to a constant
  gain on a steady tone (−88 dB, the measurement's floor), ours still ripples with the waveform
  (−52 dB). **To fix:** Safe's release on sustained limiting.
- **Level:** for the same push, Pro-L 2's Transparent and Aggressive keep about 2 dB more level
  than our Transparent and Dense; Punch and Safe are within 0.6 dB. A reading, not a verdict, but
  Dense is meant to be the loud one.
- **CPU:** Mite Limit takes twice Pro-L 2's render time at 4× with true peak, even with Pro-L 2
  behind the wine bridge. **To fix:** see `docs/dsp/limit-truepeak.md`.

### Our own true-peak figures

`docs/dsp/limit-truepeak.md` measures Mite Limit alone on harder signals and at every factor;
the renders below are the head-to-head.

## How this is measured

`cargo xtask compare limit` renders test signals through both plugins in a separate, headless
REAPER (a copy of the user's settings) and writes the measured section below; see
`docs/compare/pro-c-3.md` for how the harness sets parameters and compensates latency. Pro-L 2
runs under wine through yabridge, so its render times include the bridge.

- Both are set alike: gain, ceiling (Pro-L 2's output level), lookahead 2 ms, attack 30 ms,
  release 100 ms, both links 100 %, true peak on, dither, DC filter and unity gain off.
- True peak is read on a 32× band-limited reconstruction of each render (zero-padded spectrum),
  away from the ends of the file.
- Verdicts: **pass** means equal to FabFilter or better at the precision shown, **close** within
  the stated tolerance, **fail** worse. Rows without a verdict are readings.

## Measured

<!-- measured:begin -->
_Generated by `cargo xtask compare` on 2026-11-23. Do not edit by hand._

**11 pass · 0 close · 2 fail**

### True peak: 20 dB of gain into a −1 dBTP ceiling (Transparent, lookahead 2 ms)

How far the output's true peak goes over the ceiling (dB; 0 holds it; lower is better), without oversampling and with 4×. **BS.1770** is the standard's 4× meter (`mite_dsp::loudness::true_peak_of`), what a delivery spec checks; the verdict is on it. **32×** is an ideal band-limited reconstruction (the zero-padded spectrum): it counts content up to Nyquist that the standard's filter and a real DAC attenuate, so it reads higher for both. The last row has true-peak limiting off, for scale.

| Signal · oversampling | Pro-L 2 BS.1770 | Mite BS.1770 | Pro-L 2 32× | Mite 32× | Verdict |
|---|---|---|---|---|---|
| sine at 0.45 fs · off | 0.066 | 0.000 | 0.374 | 0.308 | **pass** |
| sine at 0.45 fs · 4× | 0.000 | 0.000 | 0.000 | 0.026 | **pass** |
| clipped noise · off | 0.291 | 0.004 | 1.717 | 1.254 | **pass** |
| clipped noise · 4× | 0.040 | 0.001 | 0.543 | 0.419 | **pass** |
| drum hits · off | 0.169 | 0.002 | 1.200 | 0.978 | **pass** |
| drum hits · 4× | 0.034 | 0.001 | 0.309 | 0.257 | **pass** |
| pink noise · off | 0.043 | 0.001 | 0.500 | 0.303 | **pass** |
| pink noise · 4× | 0.003 | 0.000 | 0.158 | 0.120 | **pass** |
| pink noise · 4×, true peak off | 0.174 | 0.123 | 0.224 | 0.162 |  |

### Level at the same settings: pink noise, 20 dB of gain, −1 dBTP, 4×

The output's RMS level (dBFS) with each style pair: how much level a style keeps for the same push into the same ceiling. Styles trade level for transparency on purpose, so these are readings, not a contest.

| Style ↔ style | Pro-L 2 | Mite Limit |
|---|---|---|
| Transparent ↔ Transparent | -8.87 | -10.82 |
| Punchy ↔ Punch | -10.06 | -10.62 |
| Aggressive ↔ Dense | -8.71 | -10.76 |
| Bus ↔ Bus | -9.49 | -11.21 |
| Safe ↔ Safe | -11.31 | -11.25 |

### Distortion: 50 Hz at −6 dBFS, 13 dB of gain into −1 dBTP, release 100 ms, 4×

THD+N of the output (20 Hz–20 kHz, dB re the fundamental; lower is cleaner): 8 dB of steady limiting on a bass tone, where a limiter's gain riding the waveform shows as distortion.

| Style ↔ style | Pro-L 2 | Mite Limit | Verdict |
|---|---|---|---|
| Transparent ↔ Transparent | -26.2 | -46.2 | **pass** |
| Safe ↔ Safe | -88.4 | -52.5 | **fail** |

### Under the ceiling: drum hits peaking at −20 dBFS, gain 0

Nothing reaches the ceiling, so a limiter should leave the signal alone. Residual: the output minus the input after delay compensation, dB re the input's RMS (lower is better; −∞ is a bit-exact null), without oversampling. Then the latency each reports and what is left after REAPER compensates it (4×, true peak on).

|  | Pro-L 2 | Mite Limit | Verdict |
|---|---|---|---|
| residual, no oversampling | -19.2 dB | -155.5 dB | **pass** |
| reported latency | 562 smp | 226 smp |  |
| output delay after compensation | 0 smp | 0 smp | **pass** |

### Render time (rough CPU): 20 s of pink noise, 12 dB of gain, true peak, 4×

Offline render time, the fastest of three renders. Pro-L 2 runs through wine and yabridge, so its figure includes the bridge: indicative only. **close** is within 5 % of FabFilter.

|  | Pro-L 2 | Mite Limit | Verdict |
|---|---|---|---|
| seconds | 0.472 | 0.914 | **fail** |

<!-- measured:end -->
