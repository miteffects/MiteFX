# Mite Limit: true-peak compliance and CPU (2026-11-23)

Measured on the engine itself (`plugins/mite-limit/src/dsp.rs`), 48 kHz. Regenerate with

    cargo run --release -p mite_limit --example truepeak
    cargo run --release -p mite_limit --example bench

Against Pro-L 2 in a real host, see `docs/compare/pro-l-2.md`.

## Compliance

Nine hard signals (sines near Nyquist, the BS.2217-style true-peak signals, square waves, clipped
noise, drums and bass) pushed 20 dB into a −1 dB ceiling. The figure is the worst over the
battery, dB over the ceiling, read by our BS.1770-4 meter (4×) and by a 32× reference
interpolator.

| Setting | BS.1770 | 32× reference | Worst signal |
|---|---|---|---|
| 1×, true peak on | +0.005 | +0.085 | drums and bass |
| 2×, true peak on | +0.000 | +0.037 | sine 0.45 fs |
| 4×, true peak on | +0.000 | +0.037 | sine 0.45 fs |
| 8×, true peak on | +0.000 | +0.022 | sine 0.45 fs |
| 16×, true peak on | +0.000 | +0.007 | sine 0.45 fs |
| 2×, true peak off | +0.137 | +0.137 | clipped noise |
| 2×, clipper Hard at the ceiling | +0.020 | +0.087 | clipped noise |
| 2×, protect 100 % | +0.000 | +0.058 | clipped noise |

- With true peak on, the standard's meter never reads more than 0.005 dB over at any factor. The
  2× detector interpolates to 8× the base rate (it read +0.151 dB with protection on a tone near
  fs/4 before that); a final guard after the down filter catches what the filter's ripple lifts.
- The 32× reference reads a little higher: it keeps content up to Nyquist that the standard's
  filter attenuates. Higher factors close that gap (16×: +0.007 dB).
- A hard clipper at the ceiling above 1× rings through the down filter (+0.020 dB); the limiter
  after it still holds the standard's figure within the tolerance.

## CPU

20 s of stereo music at 48 kHz, 9 dB into the ceiling, % of one core (AMD Ryzen 7 5800X).

| Oversampling | True peak on | True peak off |
|---|---|---|
| 1× | 2.70 % | 1.52 % |
| 2× | 4.63 % | 2.29 % |
| 4× | 5.11 % | 3.04 % |
| 8× | 7.57 % | 4.59 % |
| 16× | 12.49 % | 7.71 % |

Each later stage at 2× with true peak on (4.63 % alone):

| Stage | CPU |
|---|---|
| Clipper (Soft) | 4.73 % |
| Transient protection 100 % | 5.59 % |
| Unity gain | 4.69 % |
| Dither 16 bit, strong shaping | 4.72 % |
| DC filter | 4.65 % |
| All of them | 5.74 % |

True peak costs about as much as the limiter itself: its detector runs at 4× the base rate at
every factor. Pro-L 2 renders the same work in about half the time (`docs/compare/pro-l-2.md`),
so this is the next thing to cut: the release's per-sample dB math (noted in slice 1) and the
detector's interpolation are the candidates.
