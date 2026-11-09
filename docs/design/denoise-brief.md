# Mite DeNoise: design and build spec (in-house, 2026-11-09)

**From:** the user, 2026-11-09: "a new plugin DeNoise that has the functionality of Acon
Digital's DeNoise but apply our design … do it without prompts, and if you have any open
questions we'll do them after you do the initial version." Rival: Acon Digital DeNoise.
Accent **linen** (tokens.json `color.accent.denoise`: dark `#EDE3D2`, light `#4F463A`, hc
`#F2E9DA`; picked by CIEDE2000 against the ramp and the semantics: 17.9 / 21.4 / 19.1, the rule
is 15).

## 1. What it does (`mite_dsp::denoise`)

A gate per frequency on an STFT (frames of 2048 samples at 44.1/48 kHz, 4096 to 96 kHz, 8192
above, so bins stay ~23 Hz apart; 75 % overlap; latency one frame, 42.7 ms at 48 kHz).

- **The noise floor**, from one of two sources:
  - **Learn:** click LEARN, play a stretch of noise only, click again; the mean power per bin
    is the floor. Kept as a density per Hz at 512 log-spaced points, saved with the session
    (state field `noise_profile`), so it carries across sample rates. Double-click LEARN forgets
    it.
  - **Adaptive** (on by default): minimum statistics. The power is smoothed fast down (0.4)
    and slow up (0.8) per frame; its minimum over 1.5 s (four sub-windows) is lifted by the
    bin's own mean over minimum, at most ×20. That lift is ~1 for a steady hum and large for
    random noise, so the floor lands on the true level of both. A fixed lift put the floor
    11 dB over a hum. Smoothing at 0.8 both ways took a voice with 0.23 s pauses for noise.
- **Threshold:** dB above the floor that a sound must reach to pass untouched. **Emphasis**
  tilts it about 1 kHz (dB at 10 kHz, minus that at 100 Hz).
- **The gain:**
  - The detector is the power over two frames, averaged across ±w bins (**Smoothing**: up to a
    third of an octave).
  - Under the threshold, the reduction grows over a 6 dB knee (smoothstep) to the full
    **Reduction**.
  - It is smoothed across frequency, then in time: **Attack** as a frequency opens, **Release**
    as it closes.
- **Linked:** one gain per frequency for both channels (the image holds).
- **Noise only** (Listen): `(1 − g) X`, what is taken (it and the output sum to the input).
- **Bypass:** gain 1, so the input comes back exactly, at the same latency.

**Measured (2026-11-09, tests):**

| Case | Result |
|---|---|
| White noise, learned floor, 12 dB reduction | −11.0 dB |
| Same, adaptive floor | −10.8 dB |
| Tracked floor against the learned one | 0.2 dB apart |
| A 1 kHz tone 23 dB over noise | kept within 0.5 dB; the noise around it −8 dB or more |
| A 50 Hz hum 30 dB over noise | the floor 0.2 dB from it |
| Voice harmonics with 0.23 s pauses | the floor 3.1 dB over the noise beside them (it rode the voice before the fix) |

CPU: 0.8 % of a core at 48 kHz, 1.6 % at 96 kHz (`examples/bench`). Editor: 120 fps at 2×
(2560×1520), 0.9 ms of work a frame.

## 2. The view (1280 × 760, the suite's frame)

- **The plot:** the denoiser's own picture, at the frequencies it works on and on its own scale
  (dB, a full-scale sine on a bin at 0), never an analyzer's guess.
  - The input dim and the output lit.
  - The floor in use in the accent, lightly filled under.
  - The threshold dashed over it, with two grips: at 1 kHz THRESHOLD, at 10 kHz EMPHASIS
    (drag, wheel, double-click resets).
  - From the top, how far each frequency is turned down (GR colour, 3 px/dB).
  - While learning, the plot's edge breathes in the accent.
- **Under the plot:** LEARN (a dot under it while a profile is held) and ADAPTIVE as big icon
  buttons; dials REDUCTION, THRESHOLD, ATTACK, RELEASE, SMOOTHING, EMPHASIS.
- **Right:** IN and OUT peak meters.
- **Footer:** latency, NOISE ONLY (headphones), BYPASS, OUTPUT.
- **Help strip:** says what is under the pointer, and what to do when there is no floor yet.

## 3. Parameters (state 1, `tests/golden/mite_denoise.params.txt`)

| Parameter | Range | Default |
|---|---|---|
| `threshold` | −10…30 dB | 6 |
| `reduction` | 0…60 dB | 12 |
| `attack` | 1…200 ms | 10 |
| `release` | 10…2000 ms | 150 |
| `smoothing` | 0…100 % | 40 |
| `emphasis` | ±12 dB | 0 |
| `adaptive` | on/off | on |
| `listen` | on/off | off |
| `out_gain` | ±24 dB | 0 |
| `bypass` | on/off | off |

Monitoring (never in presets or snapshots): `listen`, `bypass`. LEARN is an action, not a
parameter (not automatable, not in undo). The profile is the state field `noise_profile`.

Factory presets: Init, Gentle Hiss, Voice Cleanup, Strong, Hum and Rumble, Room Tone.

## 4. Open questions (for the user, after the first version)

1. **Acon's other modes.** Acon DeNoise also has an "emphasis" curve you draw freely, and
   separate tools in its restoration suite (DeHum, DeClick, DeClip). Should DeNoise grow a
   drawn threshold curve (points on the dashed line) instead of one tilt? Should DeHum (notches
   on the mains harmonics) be part of it or a plugin of its own?
2. **Latency.** 43 ms at 48 kHz. Fine for mixing, too much for tracking. Should there be a
   low-latency mode (1024-sample frames, 21 ms, coarser bins)?
3. **Learning.** Click to start and click to stop. Should it rather learn for a fixed time
   (say 2 s) after one click, or learn only while the transport plays?
4. **Adaptive by default.** It works out of the box, but a learned profile is better on steady
   noise. Should learning switch ADAPTIVE off by itself when it ends?
5. **The linen accent** reads close to the output line's colour on the plot. Keep it, or pick a
   colour that stands further from the curves?
6. **Not yet heard** in REAPER on real material. Tuning the knee (6 dB) and the defaults by ear
   is the next step.
