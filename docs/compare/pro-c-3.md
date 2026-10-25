# Mite Comp against FabFilter Pro-C 3

Pro-C 3 is the compressor Mite Comp has to beat (BLUEPRINT §6.2). This page lists what each one
has, then measures every feature both have. The bar is to be as good as Pro-C 3 or better on each.

FabFilter's side comes from its manual (`FabFilter Pro-C 3.chm`, installed with the plugin; page
names in brackets) and the plugin itself. Ours comes from the code as of the date below.

## Features

| Feature | Pro-C 3 | Mite Comp |
|---|---|---|
| Curve | one stage: threshold, ratio (1:1–∞), knee 0–72 dB, range [Dynamics controls] | **up to four stages**, each compress, expand, upward compress or upward expand; knee 0–24 dB, range per stage |
| Detector | per style | **per stage: Peak, RMS or K-weighted loudness** |
| Styles / Characters | 14 styles: Clean, Versatile, Smooth, Punch, Upward, TTM, Op-El, Vari-Mu, Classic, Opto, Vocal, Mastering, Bus, Pumping [Style and character] | 6 Characters (Clean, Punch, FET, Opto, Bus, Vari-Mu) **plus Custom** (the six values by hand) |
| Saturation | Character: Tube, Diode, Bright; drive; before or after compression [Style and character] | **no** (planned) |
| Attack, release | 0.005–250 ms; release program-dependent; auto release [Time controls] | 0.05–250 ms; release 5–2500 ms; program dependence per Character |
| Lookahead, hold | lookahead 0–20 ms with a maximum that sets the latency; hold [Time controls] | lookahead 0–20 ms (the latency follows it); hold |
| Auto threshold | yes (level-independent) [Dynamics controls] | **Smart Vocal / Master**: the thresholds slide to hold a target reduction |
| Adaptive release | auto release [Time controls] | Smart Adaptive (crest factor) |
| Topology | per style (Classic and Vari-Mu feedback) | feed-forward or feedback, any Character |
| Makeup, mix | wet gain with M/S pan, auto gain, dry gain, mix 0–200 % [Time controls, Input and output options] | makeup, auto makeup, mix 0–100 %, **delta** |
| Input level / pan | yes | **no** |
| Stereo | link 0–100 %, then mid or side only; Mid, Side, M>S, S>M modes [Side chain section] | L/R or M/S with a continuous link |
| Side chain input | internal, external, **host tempo pulse, MIDI notes** [Side chain section] | internal, external |
| Side chain EQ | up to 6 bands, every Pro-Q shape, mid/side per band [Side chain section] | 3 bands (low cut, bell, high shelf by default) |
| Listen to the side chain | audition the trigger signal, and audition triggering [Side chain section] | Listen |
| Oversampling | up to 32× [Oversampling] | 2× and 4× (linear-phase half-bands, latency reported) |
| Displays | level history, knee display, peak and loudness meters, meter scale 9–90 dB [Displays and metering] | level history with GR and lift, transfer square, IN/GR/OUT meters, level range 24–60 dB |
| Stage solo | n/a | **yes** |
| Surround / Dolby Atmos | to 9.1.6 with channel linking [Surround and Dolby Atmos] | **no** |
| Instance list (in Pro-Q 4) | yes [Instance list] | **no** |
| Undo / redo, A/B | yes | yes; **snapshots A–D** |
| MIDI learn | yes | **no** |
| Resizing, full screen, compact | yes [Full Screen mode] | fixed size (compact planned) |

### Gaps to close (Pro-C 3 has, Mite Comp has not)

In order of impact: Character saturation; oversampling above 4×; more styles (Versatile, Smooth, Upward,
TTM, Op-El, Classic, Vocal, Mastering, Pumping); a 6-band side chain EQ with every shape and M/S
bands; M>S and S>M stereo modes; auto release; input level; mix above 100 %; knee to 72 dB;
attack down to 0.005 ms; host-tempo and MIDI triggering; loudness meters; compact layout and
resizing; MIDI learn; instance list; surround. Each is in the roadmap (BLUEPRINT §9).

### Ahead of Pro-C 3

Four stages on one curve (upward and downward, compression and expansion), a detector per stage,
Custom Character, feedback for any Character, Smart threshold tracking with a target, stage solo,
delta, snapshots A–D.

### What the measurements say (2026-10-25)

Every shared feature measured is level with Pro-C 3 or better (14 pass).

- **Static curve:** ours lands on the set threshold and ratio (0.1 dB, 4.00:1). Pro-C 3's curve
  sits 1.8 dB from its threshold, at 3.68:1, because its detector reads differently from a sine's
  peak.
- **Times:** our attack and release land on the set 10 / 100 ms (10.2 / 111 ms). Pro-C 3's Clean
  reads 5.7 / 62 ms for the same settings.
- **Lookahead:** ours catches drum hits 4 dB better at 5 ms, and reports only the 5 ms it uses (240
  samples). Pro-C 3 reports its 20 ms maximum (960 samples).
- **Aliasing:** without oversampling ours is 7.6 dB cleaner (the Peak detector reads the peak
  between samples, so a tone's reading no longer wobbles with where the samples fall); at 4× both
  reach −86.6 dB.
- **Render time:** level (the fastest of three renders each), with Pro-C 3 paying for the wine
  bridge: indicative only.
- **Link and range:** both correct.

### Not measured yet

Shared features the harness does not measure yet: hold; overshoot on drum transients without
lookahead at a normal attack; program-dependent release; the side chain EQ's response and
external-key ducking; the gain-reduction meter against the measured reduction; click-free
automation of threshold and ratio. They are next for `cargo xtask compare comp`.

## How this is measured

`cargo xtask compare comp` renders test signals through both plugins in Reaper and writes the
measured section below (`xtask/src/compare/`). It runs a separate Reaper instance with a copy of
the user's Reaper settings. Each case adds a track, inserts the plugin, sets its parameters, renders
and removes the track. FabFilter's parameters are set by what the plugin displays (a bisection on
the normalized value until the display reads the target), so their scaling never needs knowing.
Ours are set at their exact normalized values from our frozen manifest. Each case's log
(`target/compare/comp/render.log`) records what both plugins displayed after setting.

- Both plugins load as VST3. FabFilter's runs under wine through yabridge, so render times are
  indicative only (they include the bridge).
- Reaper's delay compensation is on: a correctly reported latency leaves the output lined up with
  the input, and the tables check that.
- Each case settles for a second before rendering, so background work a setting starts (a
  linear-phase kernel being designed) is done.
- Verdicts: **pass** means equal to FabFilter or better, **close** means within the stated
  tolerance, **fail** means worse. Rows without a verdict are readings.
- Requirements: Reaper on PATH, the FabFilter plugin, and ours installed (`cargo xtask install`).
  Run `--analyze-only` to re-measure the last renders without rendering again.

## Measured

<!-- measured:begin -->
_Generated by `cargo xtask compare` on 2026-10-25. Do not edit by hand._

**14 pass · 0 close · 0 fail**

### Static curve: threshold −20 dB, ratio 4:1 (Clean)

A tone held at every level from −60 to 0 dBFS (peak). Threshold and ratio come from the curve's asymptote over the top 12 dB (the line out = T + (in − T)/R), so a soft knee is not counted against it. Errors are against the set values (−20 dB, 4:1); lower is better, and the verdict weighs a ratio error of 0.5 like 1 dB of threshold. Mite Comp uses its Peak detector here; a sine's RMS is 3 dB under its peak, so a detector that reads RMS acts 3 dB later.

| Knee · plugin | Threshold | Ratio | Threshold error | Ratio error | Verdict |
|---|---|---|---|---|---|
| hard knee · Pro-C 3 | -18.2 dB | 3.68:1 | 1.8 dB | 0.32 |  |
| hard knee · Mite Comp | -20.0 dB | 4.00:1 | 0.0 dB | 0.00 | **pass** |
| 12 dB knee · Pro-C 3 | -18.1 dB | 3.91:1 | 1.9 dB | 0.09 |  |
| 12 dB knee · Mite Comp | -20.0 dB | 4.00:1 | 0.0 dB | 0.00 | **pass** |

### Static curve: gain at each input level (dB)

The measured gain at a few input levels, hard knee and 12 dB knee.

| Input (dBFS) | Pro-C 3 hard | Mite hard | Pro-C 3 12 dB knee | Mite 12 dB knee |
|---|---|---|---|---|
| -30 | -0.01 | 0.00 | -0.02 | 0.00 |
| -24 | -0.01 | 0.00 | -0.05 | -0.12 |
| -20 | -0.01 | 0.00 | -0.60 | -1.11 |
| -16 | -1.80 | -2.97 | -2.11 | -3.10 |
| -12 | -4.48 | -5.97 | -4.58 | -5.97 |
| -6 | -8.80 | -10.47 | -8.94 | -10.47 |
| 0 | -13.23 | -14.97 | -13.51 | -14.97 |

### Range: threshold −40 dB, ratio 20:1, range 6 dB

The most reduction at 0 dBFS input (34 dB past the threshold, where the ratio alone would take 32 dB). It must stop at the range: the error against 6 dB, lower is better.

|  | Pro-C 3 | Mite Comp | Verdict |
|---|---|---|---|
| reduction at 0 dBFS | 5.95 dB | 6.00 dB |  |
| error against 6 dB | 0.05 | 0.00 | **pass** |

### Auto makeup: threshold −20 dB, ratio 4:1

The level change with auto makeup on, at a few input levels (0 dB: the output matches the input). Both are fixed estimates from the settings, not measured loudness (Pro-C 3's manual calls its own an educated guess); Mite Comp cancels its curve at −12 dBFS. Informational: no verdict.

| Input (dBFS) | Pro-C 3 | Mite Comp |
|---|---|---|
| -30 | +4.85 | +6.00 |
| -24 | +4.85 | +6.00 |
| -18 | +4.21 | +4.53 |
| -12 | +0.38 | +0.03 |
| -6 | -3.94 | -4.47 |
| 0 | -8.36 | -8.97 |

### Attack and release: set 10 ms / 100 ms, threshold −30 dB, ratio 10:1

The tone steps from −40 to −10 dBFS and back. Attack: time to 63 % of the reduction; release: time to recover 63 %. Styles and Characters scale the times on purpose, so only Clean has a verdict (error against the set times, lower is better).

| Style ↔ Character | Pro-C 3 attack / release | Mite attack / release | Pro-C 3 GR | Mite GR | Verdict |
|---|---|---|---|---|---|
| Clean ↔ Clean | 5.7 / 62 ms | 10.2 / 111 ms | 15.8 dB | 17.9 dB | **pass** |
| Punch ↔ Punch | 6.8 / 60 ms | 5.7 / 77 ms | 16.9 dB | 17.9 dB |  |
| Opto ↔ Opto | 6.8 / 66 ms | 26.1 / 361 ms | 17.2 dB | 19.9 dB |  |
| Bus ↔ Bus | 4.5 / 342 ms | 14.7 / 94 ms | 18.0 dB | 17.9 dB |  |
| Vari-Mu ↔ Vari-Mu | 4.5 / 25 ms | 18.1 / 138 ms | 11.6 dB | 20.3 dB |  |

### Lookahead: drum hits at −3 dBFS, threshold −30 dB, ratio 20:1, attack 0.05 ms

The highest output peak (lower is better: the hits are caught), with no lookahead and with 5 ms, and the latency each plugin reports with 5 ms.

| Lookahead | Pro-C 3 peak | Mite peak | Verdict |
|---|---|---|---|
| off | -11.2 | -12.8 | **pass** |
| 5 ms | -24.5 | -28.4 | **pass** |
| reported latency (5 ms) | 960 smp | 240 smp |  |
| output delay after compensation (5 ms) | 0 smp | 0 smp | **pass** |

### Distortion: 50 Hz at −6 dBFS, threshold −30 dB, ratio 4:1, attack 0.05 ms, release 10 ms

THD+N of the output (20 Hz–20 kHz, dB re the fundamental; lower is cleaner). Settings this fast make the gain follow the waveform: these harmonics are the compressor working, not a fault, and oversampling does not remove them. Both without and with 4× oversampling.

| Oversampling | Pro-C 3 | Mite Comp | Verdict |
|---|---|---|---|
| off | -24.8 | -27.3 | **pass** |
| 4× | -24.8 | -27.3 | **pass** |

### Aliasing: 5 kHz at −6 dBFS, same settings

Energy away from the fundamental and its harmonics (what folded back from above Nyquist), dB re the fundamental, 20 Hz–20 kHz; lower is better. This is what oversampling is for.

| Oversampling | Pro-C 3 | Mite Comp | Verdict |
|---|---|---|---|
| off | -74.0 | -81.6 | **pass** |
| 4× | -86.6 | -86.6 | **pass** |

### Stereo link: left at −10 dBFS, right at −40 dBFS, threshold −30 dB, ratio 4:1

The gains of the left and right channels (the right is under the threshold on its own). Fully linked, the right must take the left's reduction; unlinked it must be untouched. The verdict is on the error: |right − left| linked, |right| unlinked.

| Link | Pro-C 3 left / right | Mite left / right | Verdict |
|---|---|---|---|
| 100 % | -13.2 / -13.2 dB | -15.0 / -15.0 dB | **pass** |
| 0 % | -13.2 / -0.0 dB | -15.0 / 0.0 dB | **pass** |

### Render time (rough CPU): 20 s of pink noise, one stage

Offline render time, the fastest of three renders. Pro-C 3 runs through wine and yabridge, so its figure includes the bridge: indicative only. **close** is within 5 % of FabFilter (a render swings more than that).

|  | Pro-C 3 | Mite Comp | Verdict |
|---|---|---|---|
| seconds | 0.149 | 0.149 | **pass** |

<!-- measured:end -->
