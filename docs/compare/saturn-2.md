# Mite Sat against FabFilter Saturn 2

Saturn 2 is the multiband saturator Mite Sat is measured against (BLUEPRINT §6.7,
docs/design/sat-brief.md). Soundtoys Decapitator is the other reference: it is not installed
here, so it appears only through its manual, in the notes below.

The question is not how many styles each has. It is whether each of our models does what the
style it is paired with does. Where Saturn 2 is ahead, it should show up below as a number. Saturn
2's side comes from the plugin itself: its parameter list as REAPER reads it, and the renders.
Ours comes from the code as of the date of the measured section.

## Features

| Feature | Saturn 2 | Mite Sat |
|---|---|---|
| Styles / models | 28: Tube ×4 (Subtle, Clean, Warm, Broken), Tape ×4 (Subtle, Clean, Warm, Old), Amp ×9 (American Tweed, American Plexi, British Rock, British Pop, Smooth, Crunchy, Lead, Screaming, Power), Saturation ×3 (Subtle, Gentle, Heavy), Transformer ×3 (Subtle, Gentle, Warm), Smudge, Breakdown, Foldback, Rectify, Destroy | 8: Tape, Tube, Transformer, Diode, Soft, Hard, Fold, Rectify |
| Tape and transformer | a style each | a **Jiles–Atherton hysteresis core** between an emphasis and its exact inverse |
| Bands | up to 6, crossover slope per band | up to 6, LR4 |
| Linear phase | yes (Processing Mode) | yes |
| Stereo | Left/Right or Mid/Side for the plugin; drive and level pan per band | per band: Stereo, Mid, Side, M/S, Left, Right; L/R and M/S balance |
| Drive | 0–100 %, level made up automatically | 0–36 dB, auto gain (a prediction from the curve, then a measured trim) |
| Bias (asymmetry) | no | yes; it follows the signal's level, so even harmonics hold at any drive |
| Tone | Bass, Mid, Treble, Presence on the processed signal | a tilt |
| Dynamics | one knob: compression to the right, expansion to the left | amount, threshold, attack, release |
| Feedback | amount and frequency per band (a ringing loop) | no |
| Oversampling | High Quality Off, Good, Superb | 1×, 2×, 4×, 8× |
| Modulation | XLFOs, envelope generators, envelope followers, MIDI sources, XY controllers, slots | 2 LFOs, an envelope (input or side chain), 9 slots |
| Delta / audition | audition per band | delta (plugin), solo and mute per band |

Decapitator (from its manual): five styles. A is an Ampex 350 tube preamp ("ultra smooth"), E
the Chandler/EMI TG channel (beefy lows, airy top), N the Neve 1057 (germanium transistors),
T and P the Culture Vulture's triode (even harmonics) and pentode (odd) settings. Around them:
Drive, Punish (+20 dB), Low Cut with Thump before the circuit, Tone (a tilt), High Cut with
Steep, Auto gain and Mix. The point it makes is the one the "Tone against drive" and "Memory"
tables measure: each style is a circuit whose response and dynamics change as it is driven,
not a curve.

## Findings (2026-11-24)

What the first run found, and why (the curves measured alone, outside the plugin, back it up):

1. **Tube is harsher than any of Saturn 2's tubes.** At 3 % THD+N its 5th harmonic and up sit
   at −17 dB against the 2nd–4th; Warm Tube's at −68 dB. Even at a whisper (−48 dBFS in, THD+N
   −48 dB) ours stays at −30 dB: the curve is tanh above zero and an exponential below, with a
   jump in curvature at zero. That joint acts like a slight rectifier at any level, so the upper
   harmonics never fall away.
2. **Tape and Transformer are harsher, and Tape aliases.** −14 to −26 dB of high harmonics at
   any level, against −40 dB (Warm Tape). Tape aliases at −44 dB at 4× against −82 dB. The
   Jiles–Atherton loop turns around with a corner at each reversal, and corners make slowly
   decaying harmonics. A real tape machine records with AC bias, which smooths those corners
   away: its distortion is mostly the 3rd harmonic.
3. **There is no gentle range on hot material.** A −12 dBFS sine already sits at the curves'
   knee at 0 dB drive, and the first 6 dB of drive fade the saturation in as a mix. So Sat goes
   from untouched to 6 % THD+N in 3 dB of drive, and a light setting is a little of a
   hard-driven sound. Saturn 2 spans 0.5 % to 10 % over 60 % of its knob.
4. **Soft is fine until it clips.** Driven gently it is purer than Gentle Saturation (only the
   3rd harmonic). Past the end of its cubic it flattens into a corner and turns harsh.
5. **Not a gap:** tone against drive. Saturn 2's tubes and tapes barely change their response
   as they are driven (under 1 dB, Old Tape ±1.3 dB), so the circuit-EQ difference expected
   before measuring is not there. Level hold and CPU are ours. Latency with oversampling is not:
   76 samples at 4×, 8 at Saturn 2's Good.

## How to read it

`cargo xtask compare sat` renders a drive sweep of every style and every model in REAPER and reads
each figure at a **matched distortion**: the drive where a 100 Hz sine's THD+N is −40, −30 or −20
dB (1 %, 3 %, 10 %). Saturn 2 runs one band with Dynamics, Feedback and the tone controls at 0,
High Quality "Good"; Mite Sat runs one band, auto gain on, 4×. Styles with no Mite Sat
counterpart (the amps, Smudge, Breakdown, Destroy) are measured for reference.

Harmonics, tone and memory are **readings, not contests**: a pass or fail would only say who is
closer to Saturn 2. They show where a behaviour exists on one side and not the other, marked
**gap**. Level hold, aliasing and CPU have verdicts.

## Measured

<!-- measured:begin -->
_Generated by `cargo xtask compare` on 2026-11-24. Do not edit by hand._

**16 pass · 0 close · 4 fail**

### Reach: THD+N of a 100 Hz sine at −12 dBFS across the whole drive range

dB re the fundamental at the lowest and highest drive (Saturn 2 0–100 %, Mite Sat 0–36 dB): how clean a style starts and how far it goes. A figure below is read only where both sweeps reach that distortion.

| Saturn 2 style ↔ Mite Sat model | Saturn min | Saturn max | Mite min | Mite max |
|---|---|---|---|---|
| Subtle Tube ↔ Tube | -77.1 | -28.6 | -87.0 | -6.3 |
| Clean Tube ↔ Tube | -41.3 | -18.7 | -87.0 | -6.3 |
| Warm Tube ↔ Tube | -35.1 | -13.1 | -87.0 | -6.3 |
| Broken Tube ↔ Tube | -31.0 | -5.9 | -87.0 | -6.3 |
| Subtle Tape ↔ Tape | -86.8 | -37.7 | -87.0 | -8.6 |
| Clean Tape ↔ Tape | -46.8 | -8.3 | -87.0 | -8.6 |
| Warm Tape ↔ Tape | -45.6 | -9.6 | -87.0 | -8.6 |
| Old Tape ↔ Tape | -36.6 | -6.7 | -87.0 | -8.6 |
| American Tweed Amp ↔ — | -25.7 | -12.1 | – | – |
| American Plexi Amp ↔ — | -11.5 | -1.9 | – | – |
| British Rock Amp ↔ — | -7.9 | -1.3 | – | – |
| British Pop Amp ↔ — | -4.9 | +1.8 | – | – |
| Smooth Amp ↔ — | -37.0 | -25.6 | – | – |
| Crunchy Amp ↔ — | -32.3 | -13.3 | – | – |
| Lead Amp ↔ — | -33.8 | -20.1 | – | – |
| Screaming Amp ↔ — | -40.3 | -1.8 | – | – |
| Power Amp ↔ — | -47.8 | -8.8 | – | – |
| Subtle Saturation ↔ Soft | -79.6 | -32.4 | -87.0 | -6.5 |
| Gentle Saturation ↔ Soft | -47.4 | -8.4 | -87.0 | -6.5 |
| Heavy Saturation ↔ Hard | -29.2 | -6.5 | -87.0 | -6.5 |
| Subtle Transformer ↔ Transformer | -86.8 | -86.7 | -87.0 | +1.6 |
| Gentle Transformer ↔ Transformer | -86.7 | -11.9 | -87.0 | +1.6 |
| Warm Transformer ↔ Transformer | -86.6 | -8.4 | -87.0 | +1.6 |
| Smudge ↔ — | -50.5 | +34.0 | – | – |
| Breakdown ↔ — | -50.9 | +27.2 | – | – |
| Foldback ↔ Fold | -45.1 | +12.2 | -87.0 | +18.8 |
| Rectify ↔ Rectify | +58.1 | -1.0 | -87.0 | +5.3 |
| Destroy ↔ — | -40.6 | +11.2 | – | – |

### Harmonics at matched distortion (100 Hz sine, −12 dBFS), at 3 % · 10 % THD+N

**Even/odd**: the even harmonics' power against the odd ones' (dB; positive is even-led, warmer; very negative is a symmetric, odd-led curve). **High**: the 5th harmonic and up against the 2nd–4th (dB; higher is harsher, fizzier distortion). Readings, not a contest; **harsher** marks a Mite Sat model with 10 dB or more high harmonics than the Saturn 2 style at the same distortion.

| Saturn 2 style ↔ Mite Sat | Saturn even/odd | Mite even/odd | Saturn high | Mite high |  |
|---|---|---|---|---|---|
| Subtle Tube ↔ Tube | -8.3 · – | -2.2 · -4.1 | -15.0 · – | -16.6 · -15.4 |  |
| Clean Tube ↔ Tube | -2.3 · -3.4 | -2.2 · -4.1 | -15.6 · -12.0 | -16.6 · -15.4 |  |
| Warm Tube ↔ Tube | +18.3 · -4.1 | -2.2 · -4.1 | -68.1 · -18.3 | -16.6 · -15.4 | **harsher** |
| Broken Tube ↔ Tube | +22.8 · +5.2 | -2.2 · -4.1 | -73.7 · -19.1 | -16.6 · -15.4 | **harsher** |
| Subtle Tape ↔ Tape | – · – | -91.7 · -106.1 | – · – | -15.0 · -13.8 |  |
| Clean Tape ↔ Tape | -88.1 · -98.8 | -91.7 · -106.1 | -36.5 · -29.9 | -15.0 · -13.8 | **harsher** |
| Warm Tape ↔ Tape | -90.0 · -100.6 | -91.7 · -106.1 | -40.4 · -41.6 | -15.0 · -13.8 | **harsher** |
| Old Tape ↔ Tape | -91.5 · -101.9 | -91.7 · -106.1 | -16.7 · -16.2 | -15.0 · -13.8 |  |
| American Tweed Amp ↔ — | – · -10.5 | — | – · -12.6 | — |  |
| American Plexi Amp ↔ — | – · – | — | – · – | — |  |
| British Rock Amp ↔ — | – · – | — | – · – | — |  |
| British Pop Amp ↔ — | – · – | — | – · – | — |  |
| Smooth Amp ↔ — | +4.1 · – | — | -28.4 · – | — |  |
| Crunchy Amp ↔ — | -7.3 · -11.6 | — | -22.4 · -13.1 | — |  |
| Lead Amp ↔ — | +0.8 · – | — | -31.0 · – | — |  |
| Screaming Amp ↔ — | +55.5 · +11.7 | — | -74.1 · -25.9 | — |  |
| Power Amp ↔ — | +7.2 · +5.3 | — | -31.4 · -21.2 | — |  |
| Subtle Saturation ↔ Soft | – · – | -92.6 · -106.5 | – · – | -70.3 · -45.1 |  |
| Gentle Saturation ↔ Soft | -88.2 · -98.8 | -92.6 · -106.5 | -43.0 · -45.2 | -70.3 · -45.1 |  |
| Heavy Saturation ↔ Hard | – · -98.6 | -90.3 · -105.5 | – · -10.8 | -11.2 · -11.8 |  |
| Subtle Transformer ↔ Transformer | – · – | -83.1 · -98.7 | – · – | -5.4 · -5.0 |  |
| Gentle Transformer ↔ Transformer | -2.5 · -12.0 | -83.1 · -98.7 | -13.2 · -16.2 | -5.4 · -5.0 | **harsher** |
| Warm Transformer ↔ Transformer | -4.6 · -13.0 | -83.1 · -98.7 | -13.8 · -15.0 | -5.4 · -5.0 |  |
| Smudge ↔ — | -30.2 · -15.5 | — | -27.9 · -21.1 | — |  |
| Breakdown ↔ — | -42.5 · -33.6 | — | -47.3 · -42.3 | — |  |
| Foldback ↔ Fold | -95.1 · -105.2 | -92.6 · -106.6 | -36.2 · -29.9 | -30.2 · -28.7 |  |
| Rectify ↔ Rectify | – · – | +50.6 · +58.5 | – · – | -12.8 · -13.7 |  |
| Destroy ↔ — | -82.0 · -88.6 | — | -39.9 · -25.6 | — |  |

### Tone against drive (pink noise): the response's lows and highs, dB re 1 kHz, at 3 % → 10 % THD+N

The linear part of each style's response (what passes through, harmonics aside). A curve alone keeps it flat at any drive; a modelled circuit moves it as it is pushed (a tube stage loses highs, tape thickens the lows). **gap** marks a style whose lows or highs move by 1 dB or more between 3 % and 10 % while its Mite Sat pair moves by less than half as much: a behaviour we do not have.

| Saturn 2 style ↔ Mite Sat | Saturn lows | Saturn highs | Mite lows | Mite highs |  |
|---|---|---|---|---|---|
| Subtle Tube ↔ Tube | +0.1 → – | +0.1 → – | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Clean Tube ↔ Tube | +0.2 → +0.1 | +0.7 → +0.7 | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Warm Tube ↔ Tube | +0.2 → +0.2 | -0.2 → -0.2 | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Broken Tube ↔ Tube | -0.6 → -1.0 | +0.1 → +0.0 | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Subtle Tape ↔ Tape | – → – | – → – | -0.3 → -0.6 | -0.2 → -0.3 |  |
| Clean Tape ↔ Tape | -0.0 → -0.0 | +0.0 → +0.0 | -0.3 → -0.6 | -0.2 → -0.3 |  |
| Warm Tape ↔ Tape | +0.6 → +0.6 | -0.2 → -0.1 | -0.3 → -0.6 | -0.2 → -0.3 |  |
| Old Tape ↔ Tape | +1.4 → +1.3 | -1.3 → -1.3 | -0.3 → -0.6 | -0.2 → -0.3 |  |
| American Tweed Amp ↔ — | – → +3.3 | – → -13.5 | — | — |  |
| American Plexi Amp ↔ — | – → – | – → – | — | — |  |
| British Rock Amp ↔ — | – → – | – → – | — | — |  |
| British Pop Amp ↔ — | – → – | – → – | — | — |  |
| Smooth Amp ↔ — | +4.0 → – | +4.1 → – | — | — |  |
| Crunchy Amp ↔ — | -6.0 → -6.1 | +10.2 → +10.1 | — | — |  |
| Lead Amp ↔ — | +7.4 → – | -15.0 → – | — | — |  |
| Screaming Amp ↔ — | -12.6 → -12.6 | -16.3 → -16.3 | — | — |  |
| Power Amp ↔ — | -2.8 → -2.8 | -44.9 → -44.9 | — | — |  |
| Subtle Saturation ↔ Soft | – → – | – → – | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Gentle Saturation ↔ Soft | -0.0 → -0.0 | +0.0 → +0.0 | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Heavy Saturation ↔ Hard | – → -0.0 | – → +0.1 | -0.0 → +0.0 | +0.0 → +0.0 |  |
| Subtle Transformer ↔ Transformer | – → – | – → – | -0.0 → -0.0 | -0.0 → -0.0 |  |
| Gentle Transformer ↔ Transformer | +0.1 → +0.1 | +0.3 → +0.3 | -0.0 → -0.0 | -0.0 → -0.0 |  |
| Warm Transformer ↔ Transformer | +0.6 → +0.6 | +0.5 → +0.5 | -0.0 → -0.0 | -0.0 → -0.0 |  |
| Smudge ↔ — | +0.0 → +0.1 | +0.1 → +0.1 | — | — |  |
| Breakdown ↔ — | -0.1 → -0.1 | -1.1 → -1.6 | — | — |  |
| Foldback ↔ Fold | -0.0 → -0.0 | +0.0 → +0.0 | -0.0 → -0.0 | +0.0 → +0.0 |  |
| Rectify ↔ Rectify | – → – | – → – | +0.0 → +0.0 | -0.1 → -0.1 |  |
| Destroy ↔ — | +1.6 → +5.1 | -11.0 → -18.3 | — | — |  |

### Memory at 10 % THD+N (100 Hz, −6 / −24 dBFS every 250 ms)

The gain at the start of a loud stretch against its end (**attack**), and of a quiet stretch after a loud one against its end (**recovery**), dB. 0 is a device without state: the same level always gets the same gain. A swing that settles is bias shift, supply sag or a magnetic core remembering. **gap** marks a style with a swing of 0.5 dB or more whose Mite Sat pair swings less than half as much.

| Saturn 2 style ↔ Mite Sat | Saturn attack | Saturn recovery | Mite attack | Mite recovery |  |
|---|---|---|---|---|---|
| Subtle Tube ↔ Tube | – | – | -0.05 | -0.40 |  |
| Clean Tube ↔ Tube | -0.00 | +1.02 | -0.05 | -0.40 | **gap** |
| Warm Tube ↔ Tube | +0.00 | +0.34 | -0.05 | -0.40 |  |
| Broken Tube ↔ Tube | +0.00 | +0.55 | -0.05 | -0.40 |  |
| Subtle Tape ↔ Tape | – | – | -0.05 | -0.39 |  |
| Clean Tape ↔ Tape | -0.00 | +0.48 | -0.05 | -0.39 |  |
| Warm Tape ↔ Tape | +0.00 | +0.51 | -0.05 | -0.39 |  |
| Old Tape ↔ Tape | -0.00 | +0.47 | -0.05 | -0.39 |  |
| American Tweed Amp ↔ — | -0.00 | +0.15 | — | — |  |
| American Plexi Amp ↔ — | – | – | — | — |  |
| British Rock Amp ↔ — | – | – | — | — |  |
| British Pop Amp ↔ — | – | – | — | — |  |
| Smooth Amp ↔ — | – | – | — | — |  |
| Crunchy Amp ↔ — | -0.00 | -0.00 | — | — |  |
| Lead Amp ↔ — | – | – | — | — |  |
| Screaming Amp ↔ — | -0.00 | +0.91 | — | — |  |
| Power Amp ↔ — | -0.00 | +0.00 | — | — |  |
| Subtle Saturation ↔ Soft | – | – | -0.05 | -0.39 |  |
| Gentle Saturation ↔ Soft | +0.00 | +0.82 | -0.05 | -0.39 | **gap** |
| Heavy Saturation ↔ Hard | -0.00 | +0.40 | -0.03 | -0.22 |  |
| Subtle Transformer ↔ Transformer | – | – | -0.00 | -0.00 |  |
| Gentle Transformer ↔ Transformer | -0.00 | +1.51 | -0.00 | -0.00 | **gap** |
| Warm Transformer ↔ Transformer | -0.00 | +0.43 | -0.00 | -0.00 |  |
| Smudge ↔ — | -0.08 | +0.00 | — | — |  |
| Breakdown ↔ — | -0.37 | -0.39 | — | — |  |
| Foldback ↔ Fold | +0.00 | +0.00 | -0.04 | -0.35 |  |
| Rectify ↔ Rectify | – | – | +0.00 | +0.00 |  |
| Destroy ↔ — | +0.00 | +0.26 | — | — |  |

### Level against drive (pink noise, −18 dBFS RMS): output − input, dB

Both make up their level as they drive (Saturn 2 always, Mite Sat with auto gain). The worse of the readings at 1 % and 10 % THD+N, nearer 0 dB is better; **close** is within 1 dB of Saturn 2.

| Saturn 2 style ↔ Mite Sat | Saturn 3 % · 10 % | Mite 3 % · 10 % | Verdict |
|---|---|---|---|
| Subtle Tube ↔ Tube | +1.6 · – | -0.1 · -0.1 | **pass** |
| Clean Tube ↔ Tube | +0.6 · +3.2 | -0.1 · -0.1 | **pass** |
| Warm Tube ↔ Tube | +0.1 · +2.6 | -0.1 · -0.1 | **pass** |
| Broken Tube ↔ Tube | -1.3 · -2.1 | -0.1 · -0.1 | **pass** |
| Subtle Tape ↔ Tape | – · – | -0.1 · +0.1 |  |
| Clean Tape ↔ Tape | +1.1 · +1.4 | -0.1 · +0.1 | **pass** |
| Warm Tape ↔ Tape | +0.6 · +0.9 | -0.1 · +0.1 | **pass** |
| Old Tape ↔ Tape | -1.6 · -2.6 | -0.1 · +0.1 | **pass** |
| Subtle Saturation ↔ Soft | – · – | -0.1 · -0.2 |  |
| Gentle Saturation ↔ Soft | +1.1 · +1.3 | -0.1 · -0.2 | **pass** |
| Heavy Saturation ↔ Hard | – · +2.5 | -0.1 · -0.1 | **pass** |
| Subtle Transformer ↔ Transformer | – · – | -0.0 · -0.1 |  |
| Gentle Transformer ↔ Transformer | -0.0 · +0.5 | -0.0 · -0.1 | **pass** |
| Warm Transformer ↔ Transformer | -1.0 · -0.7 | -0.0 · -0.1 | **pass** |
| Foldback ↔ Fold | +1.2 · +1.3 | -0.1 · -0.2 | **pass** |
| Rectify ↔ Rectify | – · – | -0.5 · -0.6 |  |

### Aliasing: a 4987 Hz sine at −6 dBFS, at the drive giving 10 % THD+N on the 100 Hz sine

Energy away from the tone and its harmonics (what folded back from above Nyquist), dB re the fundamental; lower is cleaner. Without oversampling (Saturn 2 High Quality Off, Mite Sat 1×) and with it (Good, 4×). **close** is within 3 dB.

| Saturn 2 style ↔ Mite Sat | Saturn off | Mite 1× | Saturn Good | Mite 4× | Verdict (oversampled) |
|---|---|---|---|---|---|
| Warm Tube ↔ Tube | -23.0 | -26.2 | -79.7 | -75.4 | **fail** |
| Warm Tape ↔ Tape | -26.0 | -17.3 | -81.6 | -43.8 | **fail** |
| Warm Transformer ↔ Transformer | -18.4 | -37.6 | -66.7 | -84.3 | **pass** |
| Gentle Saturation ↔ Soft | -22.3 | -22.5 | -79.5 | -70.6 | **fail** |
| Heavy Saturation ↔ Hard | -25.4 | -23.5 | -79.7 | -56.7 | **fail** |
| Foldback ↔ Fold | -11.6 | -14.1 | -14.6 | -63.0 | **pass** |
| Rectify ↔ Rectify | – | -33.9 | – | -59.1 |  |

### Render time (rough CPU) and latency: 20 s of pink noise, Warm Tube ↔ Tube, mid drive

Offline render time, the fastest of three; Saturn 2 runs through wine and yabridge, so its figure includes the bridge: indicative only. **close** is within 10 %. Latency as each reports it.

|  | Saturn 2 | Mite Sat | Verdict |
|---|---|---|---|
| seconds, Off ↔ 1× | 0.19 | 0.17 | **pass** |
| latency, Off ↔ 1× | 0 smp | 0 smp |  |
| seconds, Good ↔ 4× | 0.46 | 0.33 | **pass** |
| latency, Good ↔ 4× | 8 smp | 76 smp |  |

<!-- measured:end -->
