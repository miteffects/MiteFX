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

## 2b. Slots and gear (2026-11-11, user: "limit it to only that type of gear", "tweak gain and
output of a pedal … set its eq")

- The Pedal slot takes pedals (and outboard); the Amp slot amps and full rigs; the Cab slot IRs.
  From TONE3000 by the tone's gear (only the fitting Load buttons show; Explore opens filtered to
  the slot's gear), from files by NAM's `gear_type` (refused with where it goes; files without
  one are trusted). A full rig in the Amp slot rests the Cab (its cabinet is in the capture and
  can't be swapped): the Amp card says FULL RIG · CAB INCLUDED, the Cab card dims and says why,
  and **Use anyway** (`cab_on_rig`, state 3) stacks the IR after the rig's own cabinet.
- Each capture slot: Gain into it, Level out of it, and its own Bass/Middle/Treble (the pedal's
  after the pedal; the amp's are BASS/MIDDLE/TREBLE). The Cab: Level, Low Cut, High Cut. A slot
  switched off takes its gain, level and tone with it. Click a card: the strip and the curve's
  grips are that slot's.

## 3. Parameters (state 2; 1 → 2 appended pedal_gain, pedal_level, pedal_bass, pedal_middle,
pedal_treble, amp_gain, amp_level, cab_level, all neutral)

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

All inside the plugin (user, 2026-11-11: "make it usable just like ToneX"); only the one-time
sign-in opens TONE3000's page (its device flow: the plugin shows a code, the user confirms it on
the site; the session is kept in the settings folder). The panel covers the display:

- Search (as you pause typing; Enter at once), signed-in user, sign out, close.
- Left: Explore, Favorites, Downloaded, My Tones; Explore's gear filter (amps, pedals, full rigs,
  cabs/IRs, outboard). Above the cards: order (trending, most downloaded, newest, best match) and
  A2/A1.
- Cards: the cover image (JPEG/PNG, fetched by four workers, scaled to 480 px), gear and A2
  chips, title, maker and creator, file count, favourites, downloads, license, a heart that
  favourites on the site. Pages of 24 load as you scroll ("Showing N of M").
- Detail (a card picked): cover, title, maker, creator, license and tags, favourite, a link to
  the tone's page, its files (A2 first) each with Pedal/Amp buttons (IRs: Cab) that download once
  (kept under `TONE3000/<tone>/` with `about-tone3000.json`) and load into the slot while the rig
  keeps playing, and the description.

MiteFX's publishable key is built in (2026-11-11; overridable by `MITEFX_TONE3000_CLIENT_ID` or
`<settings>/tone3000.json`). Free tier = non-commercial (this is GPL and free); the search
endpoint is outside the free tier's listed endpoints in TONE3000's terms but answers this key —
ask support@tone3000.com if they object.

## 7. Open

- A full sign-in and download against the live API, by the user.
- Slimmable WaveNets at sizes below full (containers' sizes work).
- A faster A2 Lite kernel; a mono convolver for long cabinets (the stereo one runs one side idle).
- Stereo cabinets (two IRs, or a stereo IR kept stereo).
