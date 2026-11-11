# Mite Amp — brief

A Neural Amp Modeler player. Rivals: the NAM plugin and the TONE3000 plugin. Built 2026-11-11.

## 1. What it is

Pedal → Amp → Cab. Two NAM captures in a row (a drive pedal into an amp, or any two captures) and a
cabinet impulse response, with a gate, a tone stack, normalisation, input calibration, and
TONE3000's library inside the plugin.

## 2. Signal path

1. Input summed to mono, **Input** gain (plus calibration, below).
2. The gate's detector: the input's power over 10 ms against **Gate Threshold**; opens in 5 ms
   after a 10 ms hold, closes in 50 ms (NAM's gate). Its gain lands on the captures' output
   (their hiss goes too), delayed to match them.
3. **Pedal** capture, then **Amp** capture. Empty or switched-off slots pass the signal, in time.
   With **Calibrate**, the first capture is hit at `interface level − capture input level` dB, and
   pedal → amp at `pedal output level − amp input level` dB (when the files say).
4. **Normalize**: the last capture plays at −18 dB loudness (NAM's convention), from its metadata.
5. Tone: Bass shelf 150 Hz, Middle bell 425 Hz (Q 0.7), Treble shelf 1.8 kHz (NAM's points), Low
   Cut and High Cut (12 dB/oct, off at their ends). Exactly neutral when flat.
6. **Cab**: zero-latency convolution (the first 64 taps direct, the rest in an FFT convolver whose
   block delay is where they belong). Responses are resampled to the host's rate, stereo summed,
   cut to 500 ms, and normalised to 0 dB on average over 100 Hz–5 kHz (log scale), so cabinets
   swap without level jumps and sound the same at every rate.
7. **Output**, **Mix** against the delayed dry, **Bypass** (in time).

Captures trained at another rate run inside a resampling bracket (32-tap windowed sincs, linear
phase): a pure delay of `⌈16·(1 + host/model)⌉ + 1` samples (32 at 44.1 kHz for a 48 kHz capture),
reported as latency. At the capture's own rate: no latency.

Captures and cabinets load on a worker thread and arrive by `Handover`; a swap crossfades over
30 ms, or, when the latency changes, happens behind a 10 ms dip. The audio thread never allocates
or frees (tested under `assert_no_alloc` with mid-stream swaps).

## 3. Parameters (state 1)

input, gate, gate_threshold, calibrate, interface_level, pedal, amp, cab, size (Full/Lite: A2
containers' members), normalize, bass, middle, treble, low_cut, high_cut, mix, out_gain, bypass.
The rig (each slot's file and, for TONE3000 files, tone, creator, license, URL) is the state's
`rig` field. Presets hold the settings, not the files; interface level and calibration describe
the studio and stay out of presets.

## 4. Editor

Three cards in signal order (switch, name, generation · architecture · size · rate, who made it,
open file / TONE3000 / empty), the response plot (the cabinet's measured response dim, the tone
curve in the accent with grips — bands drag up and down, cuts sideways —, the two together lit),
the strip (Gate, Normalize, Size, Calibrate; dials Input, Gate, Bass, Middle, Treble, Low Cut,
High Cut, Interface), IN and OUT meters. Accent: tube glow (`#FFD4BF`).

## 5. Engine: `crates/mite-nam`

Pure Rust. Reads `.nam` 0.5–0.7: WaveNet (A1, and A2's per-layer kernels, bottleneck, grouped
convolutions, gating and blending, FiLM, head 1×1, head convolution, post-stack head, condition
model), LSTM, ConvNet, Linear, SlimmableContainer (A2 Full/Lite), Sequential. Matches
NeuralAmpModelerCore sample by sample on its example models (worst 4e-6 of peak; tests in
`crates/mite-nam/tests`). Fast tanh as NAM's plugins run it. One hot kernel, registers per output
width, AVX2/FMA chosen at run time: A2 Full 5.4 % of a core at 48 kHz (C++ 5.0 %), A1 standard
4.0 % (C++ 6.0 %), A2 Lite 3.3 % (C++'s hand-tuned path 0.9 %).

## 6. TONE3000

Device sign-in (the plugin shows a code; the user enters it at tone3000.com/activate), the
session kept in the settings folder. Lists: Favorites, Downloaded, My Tones, Trending, Latest; a
tone's files (A2 first, then the rest; IRs for the Cab); a click downloads into the slot, under
the settings folder's `TONE3000/<tone>/`, with an `about-tone3000.json` of creator and license.
Branded as their terms ask ("Powered by TONE3000", a way to the full site).

MiteFX's publishable key is built in (2026-11-11; overridable by `MITEFX_TONE3000_CLIENT_ID` or
`<settings>/tone3000.json`). Free tier = non-commercial (this is GPL and free).

## 7. Open

- A full sign-in and download against the live API, by the user.
- Slimmable WaveNets at sizes below full (containers' sizes work).
- A faster A2 Lite kernel; a mono convolver for long cabinets (the stereo one runs one side idle).
- Stereo cabinets (two IRs, or a stereo IR kept stereo).
