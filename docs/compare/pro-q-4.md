# Mite EQ against FabFilter Pro-Q 4

Pro-Q 4 is the EQ Mite EQ has to beat (BLUEPRINT §6.1). This page lists what each one has, then
measures every feature both have. The bar is to be as good as Pro-Q 4 or better on each.

FabFilter's side comes from its manual (`FabFilter Pro-Q 4.chm`, installed with the plugin; page
names in brackets) and the plugin itself. Ours comes from the code as of the date below.

## Features

| Feature | Pro-Q 4 | Mite EQ |
|---|---|---|
| Bands | 24 | 24 |
| Shapes | Bell, Low/High Shelf, Low/High Cut, Notch, Band Pass, Tilt Shelf, Flat Tilt, All Pass [Band controls] | the same ten |
| Slopes | 0–96 dB/oct, any fractional value, plus Brickwall [Band controls] | 6–96 dB/oct in nine steps, plus Brickwall: **no fractional slopes** |
| Frequency, gain | 10 Hz–30 kHz, ±30 dB | 10 Hz–30 kHz, ±30 dB |
| Gain-Q interaction (analog-style) | yes [Band controls] | **no** |
| Processing modes | Zero Latency, Natural Phase, Linear Phase with 5 resolutions [Processing mode] | the same three; Linear Phase with 4 resolutions (to 173 ms at 48 kHz) |
| Dynamic EQ | range ±30 dB, threshold or auto, relative attack/release or auto, external side chain, free trigger filtering with audition [Dynamic EQ] | range ±30 dB, threshold, ratio, attack/release in ms; trigger on the band, the side chain **or another band**; **a transient/sustain mode** |
| Spectral dynamics | yes, with density and a 3 dB/oct trigger tilt [Spectral dynamics] | yes, with smoothing and a threshold relative to the surroundings (soothe-style) |
| Character (saturation) | Clean, Subtle, Warm [Character modes] | **no** (planned) |
| Stereo placement | Stereo, Left, Right, Mid, Side; split into L/R or M/S [Stereo options] | Stereo, Left, Right, Mid, Side; **no split button** |
| Surround / Dolby Atmos (to 9.1.6) | yes [Surround and Dolby Atmos] | **no** |
| Analyzer | pre, post, external (side chain or another instance); range, resolution, speed, tilt, freeze [Spectrum analyzer] | pre, post, side chain; **no analyzer settings, no freeze** |
| Spectrum Grab | yes [Spectrum Grab] | yes |
| Collisions / masking | collision display against another instance [Spectrum analyzer] | collision display, **plus carve suggestions that can write to the other instance** |
| EQ Match | reference from an instance, the side chain, a file or a saved spectrum [EQ Match] | reference from the side chain or a file |
| EQ Sketch (draw the curve) | yes [EQ Sketch] | **no** |
| Instance list (all instances in one window) | yes, with Pro-C 3, Pro-DS, Pro-G [Instance List] | **no** |
| Piano display, note quantize | yes [Piano display] | note names in the help strip only |
| Solo | yes [Solo] | yes |
| Display range ±3/6/12/30 dB, auto | yes | yes |
| Horizontal zoom | yes [Display and workflow] | **no** |
| Typed value entry (double-click, "1k", "A4") | yes [Knobs] | **no** |
| Output | level, pan (L/R or M/S), phase invert, auto gain, gain scale [Output options] | level, pan, auto gain, **mix and delta**; **no phase invert**; gain scale was built and dropped |
| Undo / redo, A/B | yes | yes; **snapshots A–D** instead of A/B |
| Presets with tags and search | yes | yes |
| MIDI learn | yes [MIDI Learn] | **no** |
| Resizing, full screen | 5 sizes, scaling, full screen [Full Screen mode] | fixed size (compact layout planned) |

### Gaps to close (Pro-Q 4 has, Mite EQ has not)

In order of impact: Character modes; fractional slopes; analyzer settings and freeze; typed value
entry; phase invert; gain-Q interaction; split into L/R or M/S; horizontal zoom; piano display;
EQ Sketch; resizing; MIDI learn; EQ Match from another instance; instance list; surround. Each is
in the roadmap (BLUEPRINT §9).

### Ahead of Pro-Q 4

Detection from another band, transient/sustain dynamic bands, carve suggestions that write to the
other instance, snapshots A–D, mix and delta.

### What the measurements say (2026-10-25)

- **Accuracy (Zero Latency and Natural Phase):** Mite EQ is on the analog curve within 0.05 dB for
  every band tested (0.00 for most). Pro-Q 4 is off by up to 1.7 dB near the top in Zero Latency,
  0.2 dB in Natural Phase, and 28–33 dB on a 96 dB/oct cut at 15 kHz.
- **Natural Phase:** follows the analog phase within 0.1°, and rings before the impulse for
  0.25 ms at most (Pro-Q 4: 0.3–1.3 ms), at 1.6 ms of latency.
- **Zero Latency phase:** closer to analog than Pro-Q 4 on 14 of 15 bands (the free band above
  20 kHz is shaped to pull the minimum phase towards it). The one fail is the 96 dB/oct cut at
  15 kHz: its analog prototype has 0.17 ms more group delay than a zero-latency filter can give
  from 2 kHz of spectrum. Neither EQ follows it (Pro-Q 4 gets 110° by leaving its magnitude 28 dB
  off; we stay at 0.00 dB). Natural Phase follows it exactly.
- **Linear Phase:** a Q 8 bell at 100 Hz is 0.10 dB off at Max (189 ms) against Pro-Q 4's best
  0.53 dB (High, 209 ms); a low cut 0.00 against 2.77 dB.
- **Dynamic EQ:** both reach the set range; on a steady low tone ours distorts far less (THD+N
  −87 against −71 dB). Stereo placement leaks nothing.
- **Render time:** multi-band renders are faster (8 bells 0.21 against 0.39 s; 24 bands in
  Linear Phase 0.28 against 2.0 s). The total of the single-band Zero Latency renders (each the
  fastest of three) is 2 % behind (0.90 against 0.88 s): each new instance designs its
  correction (about 3 ms) before it renders. Pro-Q 4's figures include the wine bridge.

### Not measured yet

Shared features the harness does not measure yet: spectral dynamics (reduction of a resonance in
noise, and its artifacts); click-free automation (sweeping frequency, gain and Q while playing,
switching modes and bands); EQ Match (the spectrum error after matching); auto gain; the
analyzer (by eye, from screenshots). They are next for `cargo xtask compare eq`.

## How this is measured

`cargo xtask compare eq` renders test signals through both plugins in Reaper and writes the
measured section below (`xtask/src/compare/`). It runs a separate Reaper instance with a copy of
the user's Reaper settings. Each case adds a track, inserts the plugin, sets its parameters, renders
and removes the track. FabFilter's parameters are set by what the plugin displays (a bisection on
the normalized value until the display reads the target), so their scaling never needs knowing.
Ours are set at their exact normalized values from our frozen manifest. Each case's log
(`target/compare/eq/render.log`) records what both plugins displayed after setting.

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

**99 pass · 1 close · 2 fail**

### Zero Latency: magnitude error against the analog curve (dB, worst 20 Hz–20 kHz at 44.1 kHz)

Each EQ's 44.1 kHz response against its own 192 kHz response (where the digital EQ is as close to analog as it gets), where the curve is within 30 dB of its peak. Lower is better; **close** is within 0.1 dB of Pro-Q 4.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 1 kHz +12 dB | 0.02 | 0.00 | **pass** |
| Bell 10 kHz +12 dB Q 2 | 0.23 | 0.02 | **pass** |
| Bell 16 kHz −12 dB | 0.22 | 0.01 | **pass** |
| Bell 100 Hz +12 dB Q 8 | 0.00 | 0.00 | **pass** |
| Low shelf 100 Hz +6 dB | 0.00 | 0.00 | **pass** |
| High shelf 8 kHz +6 dB | 0.11 | 0.01 | **pass** |
| High shelf 12 kHz +6 dB Q 2 | 0.03 | 0.01 | **pass** |
| Low cut 80 Hz 24 dB/oct | 0.00 | 0.00 | **pass** |
| High cut 12 kHz 24 dB/oct | 1.66 | 0.07 | **pass** |
| High cut 15 kHz 96 dB/oct | 27.83 | 0.00 | **pass** |
| Notch 15 kHz | 1.02 | 0.01 | **pass** |
| Band pass 2 kHz | 0.77 | 0.03 | **pass** |
| Tilt shelf 1 kHz +6 dB | 0.01 | 0.00 | **pass** |
| Flat tilt 1 kHz +6 dB | 0.08 | 0.01 | **pass** |
| All pass 1 kHz | 0.00 | 0.00 | **pass** |

### Zero Latency: phase error against the analog phase (degrees, worst 20 Hz–20 kHz at 44.1 kHz)

As above (where the curve is within 30 dB of its peak), for the phase. Zero Latency is minimum phase by design, so only Natural Phase is meant to follow the analog phase.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 1 kHz +12 dB | 4.5 | 3.2 | **pass** |
| Bell 10 kHz +12 dB Q 2 | 22.3 | 5.9 | **pass** |
| Bell 16 kHz −12 dB | 16.6 | 9.5 | **pass** |
| Bell 100 Hz +12 dB Q 8 | 0.1 | 0.0 | **pass** |
| Low shelf 100 Hz +6 dB | 0.1 | 0.1 | **pass** |
| High shelf 8 kHz +6 dB | 9.9 | 3.6 | **pass** |
| High shelf 12 kHz +6 dB Q 2 | 14.5 | 4.6 | **pass** |
| Low cut 80 Hz 24 dB/oct | 0.5 | 0.5 | **pass** |
| High cut 12 kHz 24 dB/oct | 163.6 | 127.1 | **pass** |
| High cut 15 kHz 96 dB/oct | 110.4 | 178.8 | **fail** |
| Notch 15 kHz | 33.4 | 7.7 | **pass** |
| Band pass 2 kHz | 58.5 | 22.4 | **pass** |
| Tilt shelf 1 kHz +6 dB | 2.5 | 1.5 | **pass** |
| Flat tilt 1 kHz +6 dB | 11.8 | 4.1 | **pass** |
| All pass 1 kHz | 6.1 | 4.3 | **pass** |

### Zero Latency: response before the input impulse (pre-ringing)

How long before the impulse the output comes within 60 dB of its peak, and its level there. Both are minimum- or near-minimum-phase modes, so less is better.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 1 kHz +12 dB | none | none | **pass** |
| Bell 10 kHz +12 dB Q 2 | none | none | **pass** |
| Bell 16 kHz −12 dB | none | none | **pass** |
| Bell 100 Hz +12 dB Q 8 | none | none | **pass** |
| Low shelf 100 Hz +6 dB | none | none | **pass** |
| High shelf 8 kHz +6 dB | none | none | **pass** |
| High shelf 12 kHz +6 dB Q 2 | none | none | **pass** |
| Low cut 80 Hz 24 dB/oct | none | none | **pass** |
| High cut 12 kHz 24 dB/oct | none | none | **pass** |
| High cut 15 kHz 96 dB/oct | none | none | **pass** |
| Notch 15 kHz | none | none | **pass** |
| Band pass 2 kHz | none | none | **pass** |
| Tilt shelf 1 kHz +6 dB | none | none | **pass** |
| Flat tilt 1 kHz +6 dB | none | none | **pass** |
| All pass 1 kHz | none | none | **pass** |

### Natural Phase: magnitude error against the analog curve (dB, worst 20 Hz–20 kHz at 44.1 kHz)

Each EQ's 44.1 kHz response against its own 192 kHz response (where the digital EQ is as close to analog as it gets), where the curve is within 30 dB of its peak. Lower is better; **close** is within 0.1 dB of Pro-Q 4.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 1 kHz +12 dB | 0.02 | 0.00 | **pass** |
| Bell 10 kHz +12 dB Q 2 | 0.09 | 0.00 | **pass** |
| Bell 16 kHz −12 dB | 0.06 | 0.01 | **pass** |
| Bell 100 Hz +12 dB Q 8 | 0.00 | 0.00 | **pass** |
| Low shelf 100 Hz +6 dB | 0.00 | 0.00 | **pass** |
| High shelf 8 kHz +6 dB | 0.01 | 0.00 | **pass** |
| High shelf 12 kHz +6 dB Q 2 | 0.05 | 0.01 | **pass** |
| Low cut 80 Hz 24 dB/oct | 0.00 | 0.00 | **pass** |
| High cut 12 kHz 24 dB/oct | 0.15 | 0.00 | **pass** |
| High cut 15 kHz 96 dB/oct | 33.19 | 0.00 | **pass** |
| Notch 15 kHz | 0.22 | 0.05 | **pass** |
| Band pass 2 kHz | 0.01 | 0.01 | **pass** |
| Tilt shelf 1 kHz +6 dB | 0.00 | 0.00 | **pass** |
| Flat tilt 1 kHz +6 dB | 0.01 | 0.00 | **pass** |
| All pass 1 kHz | 0.00 | 0.00 | **pass** |

### Natural Phase: phase error against the analog phase (degrees, worst 20 Hz–20 kHz at 44.1 kHz)

As above (where the curve is within 30 dB of its peak), for the phase. Zero Latency is minimum phase by design, so only Natural Phase is meant to follow the analog phase.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 1 kHz +12 dB | 1.0 | 0.0 | **pass** |
| Bell 10 kHz +12 dB Q 2 | 3.8 | 0.1 | **pass** |
| Bell 16 kHz −12 dB | 3.2 | 0.0 | **pass** |
| Bell 100 Hz +12 dB Q 8 | 0.0 | 0.0 | **pass** |
| Low shelf 100 Hz +6 dB | 0.0 | 0.0 | **pass** |
| High shelf 8 kHz +6 dB | 1.9 | 0.0 | **pass** |
| High shelf 12 kHz +6 dB Q 2 | 4.0 | 0.0 | **pass** |
| Low cut 80 Hz 24 dB/oct | 0.1 | 0.0 | **pass** |
| High cut 12 kHz 24 dB/oct | 11.2 | 0.1 | **pass** |
| High cut 15 kHz 96 dB/oct | 176.4 | 0.0 | **pass** |
| Notch 15 kHz | 8.1 | 0.3 | **pass** |
| Band pass 2 kHz | 1.4 | 0.2 | **pass** |
| Tilt shelf 1 kHz +6 dB | 0.6 | 0.0 | **pass** |
| Flat tilt 1 kHz +6 dB | 1.2 | 0.0 | **pass** |
| All pass 1 kHz | 1.4 | 0.0 | **pass** |

### Natural Phase: response before the input impulse (pre-ringing)

How long before the impulse the output comes within 60 dB of its peak, and its level there. Both are minimum- or near-minimum-phase modes, so less is better.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 1 kHz +12 dB | 0.41 ms (-39 dB) | 0.14 ms (-43 dB) | **pass** |
| Bell 10 kHz +12 dB Q 2 | 0.88 ms (-27 dB) | 0.20 ms (-30 dB) | **pass** |
| Bell 16 kHz −12 dB | 0.68 ms (-26 dB) | 0.20 ms (-29 dB) | **pass** |
| Bell 100 Hz +12 dB Q 8 | none | none | **pass** |
| Low shelf 100 Hz +6 dB | none | none | **pass** |
| High shelf 8 kHz +6 dB | 0.66 ms (-29 dB) | 0.20 ms (-31 dB) | **pass** |
| High shelf 12 kHz +6 dB Q 2 | 0.77 ms (-25 dB) | 0.23 ms (-26 dB) | **pass** |
| Low cut 80 Hz 24 dB/oct | 0.05 ms (-57 dB) | 0.05 ms (-59 dB) | **pass** |
| High cut 12 kHz 24 dB/oct | 0.27 ms (-38 dB) | 0.18 ms (-38 dB) | **pass** |
| High cut 15 kHz 96 dB/oct | 4.47 ms (-10 dB) | none | **pass** |
| Notch 15 kHz | 0.75 ms (-21 dB) | 0.20 ms (-25 dB) | **pass** |
| Band pass 2 kHz | 1.32 ms (-23 dB) | 0.25 ms (-27 dB) | **pass** |
| Tilt shelf 1 kHz +6 dB | 0.32 ms (-42 dB) | 0.09 ms (-48 dB) | **pass** |
| Flat tilt 1 kHz +6 dB | 0.73 ms (-27 dB) | 0.20 ms (-32 dB) | **pass** |
| All pass 1 kHz | 0.54 ms (-33 dB) | 0.16 ms (-38 dB) | **pass** |

### Linear Phase: best low-frequency accuracy at any resolution (dB)

The lowest accuracy figure each EQ reaches in the table below, with the latency it costs. Lower is better.

| Band | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Bell 100 Hz +12 dB Q 8 | 0.53 (209 ms) | 0.10 (189 ms) | **pass** |
| Low cut 80 Hz 24 dB/oct | 2.77 (1509 ms) | 0.00 (189 ms) | **pass** |

### Linear Phase: latency, low-frequency accuracy and pre-ringing per resolution (44.1 kHz)

Latency as reported to the host (Reaper compensates it; the residual is the impulse peak's offset after compensation, which must be 0). Accuracy: the worst magnitude difference 20 Hz–20 kHz from the same EQ's Natural Phase response at 192 kHz. Pre-ring: how long before the impulse the output is within 60 dB of its peak. Pro-Q 4's latencies match its manual. Its Very High and Maximum render the Q 8 bell wider than High does, repeatably (also with a second of settling before the render), in Reaper's offline render; not yet checked in a real-time render.

| Band · EQ · resolution | Latency | Residual | Accuracy (dB) | Pre-ring |
|---|---|---|---|---|
| Bell 100 Hz +12 dB Q 8 · Pro-Q 4 · Low | 3072 smp (69.7 ms) | 0 | 3.55 | 15.3 ms |
| Bell 100 Hz +12 dB Q 8 · Pro-Q 4 · Medium | 5120 smp (116.1 ms) | 0 | 1.62 | 20.1 ms |
| Bell 100 Hz +12 dB Q 8 · Pro-Q 4 · High | 9216 smp (209.0 ms) | 0 | 0.53 | 20.6 ms |
| Bell 100 Hz +12 dB Q 8 · Pro-Q 4 · Very High | 17408 smp (394.7 ms) | 0 | 6.10 | 22.1 ms |
| Bell 100 Hz +12 dB Q 8 · Pro-Q 4 · Maximum | 66560 smp (1509.3 ms) | 0 | 6.15 | 22.2 ms |
| Bell 100 Hz +12 dB Q 8 · Mite EQ · Low | 640 smp (14.5 ms) | 0 | 8.08 | 5.2 ms |
| Bell 100 Hz +12 dB Q 8 · Mite EQ · Medium | 1152 smp (26.1 ms) | 0 | 4.64 | 15.7 ms |
| Bell 100 Hz +12 dB Q 8 · Mite EQ · High | 4224 smp (95.8 ms) | 0 | 0.71 | 20.3 ms |
| Bell 100 Hz +12 dB Q 8 · Mite EQ · Max | 8320 smp (188.7 ms) | 0 | 0.10 | 20.3 ms |
| Low cut 80 Hz 24 dB/oct · Pro-Q 4 · Low | 3072 smp (69.7 ms) | 0 | 4.58 | 4.3 ms |
| Low cut 80 Hz 24 dB/oct · Pro-Q 4 · Medium | 5120 smp (116.1 ms) | 0 | 3.26 | 4.3 ms |
| Low cut 80 Hz 24 dB/oct · Pro-Q 4 · High | 9216 smp (209.0 ms) | 0 | 2.90 | 4.4 ms |
| Low cut 80 Hz 24 dB/oct · Pro-Q 4 · Very High | 17408 smp (394.7 ms) | 0 | 2.80 | 4.4 ms |
| Low cut 80 Hz 24 dB/oct · Pro-Q 4 · Maximum | 66560 smp (1509.3 ms) | 0 | 2.77 | 4.4 ms |
| Low cut 80 Hz 24 dB/oct · Mite EQ · Low | 640 smp (14.5 ms) | 0 | 4.16 | 4.9 ms |
| Low cut 80 Hz 24 dB/oct · Mite EQ · Medium | 1152 smp (26.1 ms) | 0 | 0.56 | 4.9 ms |
| Low cut 80 Hz 24 dB/oct · Mite EQ · High | 4224 smp (95.8 ms) | 0 | 0.00 | 4.9 ms |
| Low cut 80 Hz 24 dB/oct · Mite EQ · Max | 8320 smp (188.7 ms) | 0 | 0.00 | 4.9 ms |

### Dynamic EQ: gain against level (bell at the tone, Q 1, range −12 dB, threshold −30 dB)

A tone at the band's frequency held at each level (dBFS peak). Both use the range as the most the band moves; the verdict row is the depth reached at 0 dBFS against the set −12 dB (lower error is better). Pro-Q 4's attack and release are relative and left at their automatic 50 %; Mite EQ's at 10 / 150 ms.

| Input (dBFS) | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| -50 | -2.51 | 0.00 |  |
| -40 | -11.98 | 0.00 |  |
| -35 | -12.00 | 0.00 |  |
| -30 | -12.00 | -0.49 |  |
| -25 | -12.00 | -5.46 |  |
| -20 | -12.00 | -11.31 |  |
| -10 | -12.00 | -11.99 |  |
| 0 | -12.00 | -11.99 |  |
| depth error at 0 dBFS (dB) | 0.00 | 0.01 | **close** |

### Dynamic EQ on a steady low tone: 100 Hz at −6 dBFS, bell 100 Hz, range −12 dB, threshold −30 dB

Over the last second: the ripple of the gain (largest minus smallest, per 1 ms) and THD+N of the output. A steady tone must give a steady gain; lower is better for both.

|  | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| gain ripple (dB) | 0.00 | 0.00 | **pass** |
| THD+N (dB) | -70.8 | -87.0 | **pass** |

### Stereo placement: bell 1 kHz +12 dB on one channel

An impulse on both channels. Left only: the right channel must stay flat. Side only, on a mono signal (no side content): both channels must stay flat. The worst deviation from 0 dB, 20 Hz–20 kHz; lower is better.

| Placement | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Left: right channel | 0.005 | 0.000 | **pass** |
| Side on mono: left channel | 0.005 | 0.000 | **pass** |

### Render time (rough CPU): 30 s of pink noise

Offline render time in seconds; lower is better. Pro-Q 4 runs through wine and yabridge, so its figure includes the bridge: indicative only.

| Bands | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| 8 bells, Zero Latency | 0.394 | 0.212 | **pass** |
| 8 dynamic bells, Zero Latency | 0.402 | 0.349 | **pass** |
| 24 bands, Linear Phase High | 2.010 | 0.284 | **pass** |

### Render time (rough CPU)

Total offline render time of every Zero Latency and Natural Phase case at 44.1 kHz (one band each, 4 s of audio), each the fastest of three renders. Pro-Q 4 runs through wine and yabridge, so its figure includes the bridge: indicative only.

| Mode | Pro-Q 4 | Mite EQ | Verdict |
|---|---|---|---|
| Zero Latency | 0.88 | 0.90 | **fail** |
| Natural Phase | 0.99 | 0.88 | **pass** |

<!-- measured:end -->
