# Mite Sat: aliasing, THD and CPU (2026-11-03)

Measured on the engine itself (`mite_dsp::sat::distortion`): one band, auto gain off, a sine
that is whole in a 16 384-sample window (prime cycle counts, so every harmonic and every alias
lands on a bin of its own and none falls onto another), run for a second at 48 kHz, then the last
window read with a rectangular FFT. **Aliasing** is every bin that is neither a harmonic below
Nyquist nor DC, against the strongest partial (Rectify gives its fundamental away, so the
fundamental cannot be the reference). Regenerate with

    cargo run --release -p mite-dsp --example sat_report

`aliasing_stays_where_the_report_has_it` (in `mite_dsp::sat`) holds the 8× figures for Tube, Hard
and Diode and checks that a clean pass at 8× leaves less than −100 dB (the oversampler's own
filters).

## What the numbers say

- The oversampler is not the limit: its images and folds are more than 100 dB down (tested in
  `mite_dsp::oversample`). What aliases is the curve itself: a hard-driven curve makes harmonics far
  past even 8× Nyquist (a −6 dBFS sine at 18 dB of drive plus the +12 dB calibration is clipped close
  to square, harmonics falling as 1/h), and the ones above the high rate fold back. ADAA was dropped
  for the reasons in sat-brief §1.3, so the factor is the remedy.
- 2×, the default, gains 10–13 dB over 1× on the 5 kHz tone for the static saturating models (Tape
  5 dB: its treble lift drives the highs hardest). 8× puts
  them 33–43 dB under the strongest partial on a 5 kHz tone at −6 dBFS and 18 dB of drive. Real
  music has far less energy up there.
- Fold and Rectify are broadband by nature. Fold at 36 dB folds the wave many times, and its aliases
  outweigh its harmonics at every factor. Rectify on a 10 kHz tone has a single partial below Nyquist
  (the octave), so everything else is folded. Use them oversampled, on material with little top end.
- THD (999 Hz at −18 dBFS, 2×): Hard is clean until its knee (−50 dB at 6 dB of drive) and then
  matches Soft; Tape stays the gentlest when pushed (−15.6 dB at 36 dB); Rectify reads high because its
  fundamental is nearly gone.

### Aliasing, 5007 Hz at −6 dBFS, drive 18 dB (dB under the strongest partial)

| Model | 1× | 2× | 4× | 8× |
|---|---:|---:|---:|---:|
| Tape | -10.0 | -14.9 | -26.5 | -33.2 |
| Tube | -12.2 | -24.6 | -37.3 | -40.5 |
| Transformer | -13.0 | -26.1 | -37.9 | -39.0 |
| Diode | -17.0 | -30.4 | -40.7 | -42.8 |
| Soft | -10.6 | -20.8 | -35.4 | -37.4 |
| Hard | -10.4 | -20.1 | -33.3 | -36.6 |
| Fold | 11.2 | 6.3 | -38.3 | -38.3 |
| Rectify | 8.7 | 3.6 | -3.6 | -17.3 |

### Aliasing, 9999 Hz at −6 dBFS, drive 18 dB (dB under the strongest partial)

| Model | 1× | 2× | 4× | 8× |
|---|---:|---:|---:|---:|
| Tape | -3.7 | -13.0 | -17.1 | -27.7 |
| Tube | -7.8 | -17.4 | -27.3 | -43.2 |
| Transformer | -8.5 | -20.2 | -29.2 | -48.2 |
| Diode | -11.6 | -24.6 | -33.5 | -48.5 |
| Soft | -7.0 | -16.5 | -23.5 | -43.4 |
| Hard | -6.9 | -16.1 | -22.7 | -38.7 |
| Fold | 11.3 | 5.7 | -1.0 | -86.0 |
| Rectify | 9.2 | 5.3 | 0.3 | -7.5 |

### Aliasing, 9999 Hz at −6 dBFS, drive 36 dB (dB under the strongest partial)

| Model | 1× | 2× | 4× | 8× |
|---|---:|---:|---:|---:|
| Tape | -3.4 | -12.0 | -15.9 | -24.5 |
| Tube | -6.6 | -13.7 | -19.9 | -27.7 |
| Transformer | -6.8 | -15.2 | -20.9 | -29.6 |
| Diode | -9.6 | -20.1 | -26.7 | -36.7 |
| Soft | -6.4 | -14.3 | -19.5 | -27.0 |
| Hard | -6.4 | -14.2 | -19.4 | -26.9 |
| Fold | 19.3 | 16.4 | 12.4 | 10.5 |
| Rectify | 18.8 | 15.5 | 12.3 | 8.9 |

### THD, 999 Hz at −18 dBFS (dB against the fundamental), 2×

| Model | 6 dB | 12 dB | 24 dB | 36 dB |
|---|---:|---:|---:|---:|
| Tape | -23.9 | -19.8 | -16.5 | -15.6 |
| Tube | -21.1 | -15.9 | -8.7 | -6.7 |
| Transformer | -23.8 | -16.4 | -9.5 | -7.2 |
| Diode | -19.6 | -16.2 | -12.5 | -10.6 |
| Soft | -27.7 | -14.2 | -7.8 | -6.8 |
| Hard | -50.1 | -12.8 | -7.6 | -6.8 |
| Fold | -27.2 | -13.2 | 7.1 | 10.5 |
| Rectify | 42.3 | 43.8 | 39.9 | 31.6 |

## CPU

`cargo run --release -p mite_sat --example bench`: 20 s of stereo music at 48 kHz in 512-sample
blocks, one band at 12 dB of drive unless stated, % of one core of a Ryzen 7 5800X.

| Model | 1× | 2× | 4× | 8× |
|---|---:|---:|---:|---:|
| Tape | 4.23 | 5.65 | 7.30 | 10.28 |
| Tube | 0.43 | 0.87 | 1.35 | 2.22 |
| Transformer | 1.21 | 2.34 | 4.04 | 7.33 |
| Soft | 0.37 | 0.75 | 1.10 | 1.75 |
| Fold | 0.41 | 0.83 | 1.21 | 1.99 |

| Six bands at 4× | % |
|---|---:|
| Tube | 7.67 |
| Tape | 26.45 |
| Tube with dynamics | 8.01 |

The half-band filters' first stage (35 pairs) runs as a mirrored dot product in eight lanes, which
the compiler keeps in vector registers (2026-11-03: six bands of Tube at 4×, 8.9 → 7.7 %; the short
later stages stay folded, as padding them costs more than it saves). Tape and Transformer are bound
by the hysteresis core: each substep is a serial chain of exponentials and divisions. Interleaving
channels, a Langevin table (2026-11-01) and a cheaper predictor slope (2026-11-03: 7 % quicker, but
the loop then misses its accuracy bound) gave nothing worth keeping. Per-band dynamics cost about
0.06 % of a core a band.
