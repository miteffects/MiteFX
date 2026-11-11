# Mite Rig — brief

A guitar and bass signal chain (user, 2026-11-11): "re-visualize this plugin, rename it Mite
Rig … drive pedals from TONE3000, amps from TONE3000, cabs too … our pedals … something like
ToneX but with freedom to put it in chain wherever user wants … parallel connections or even
triple fork … simple view for existing pedals but also … all existing UI". Replaces Mite Amp
(never released); reuses its NAM engine (`crates/mite-nam`), its bracket, cabinet convolution,
gate, tone stack and TONE3000 browser.

## 1. The board

- A chain read left to right, IN → … → OUT.
- **Split**: forks the chain into 2 or 3 parallel **lanes** that merge again. Each lane has a
  level and a pan at the merge; a lane may be empty (a dry path). Splits do not nest (a lane is a
  plain chain); the main chain can hold several splits one after another.
- **Blocks**: one effect each, with a bypass (click-free) and, for captures, the file it plays.
- Lanes are time-aligned: the shorter lanes are delayed to the longest one, and the board's
  latency (reported) is its longest path.
- Mono in by default (a guitar), stereo from the first stereo block on (delay, reverb, chorus,
  pan at a merge); the output is always stereo.

Decisions (user): lanes with splits, not a free node graph; no drive pedals of our own (drives
come from TONE3000 captures); Mite Amp is replaced.

## 2. Blocks

| Kind | Source | Slice |
|---|---|---|
| Capture (pedal) | NAM, files or TONE3000 (pedal/outboard gear only) | 1 |
| Capture (amp / full rig) | NAM, files or TONE3000 (amp, full rig) | 1 |
| Cab | IR, files or TONE3000 | 1 |
| Gate | NAM-style gate (Mite Amp's) | 1 |
| Tone | bass/middle/treble + low/high cut (Mite Amp's) | 1 |
| Boost / Volume | clean gain, volume with taper | 1 |
| Comp | `mite_dsp::dynamics` (Mite Comp) | 2 |
| EQ | `mite_dsp::eq` (Mite EQ) | 2 |
| Delay | `mite_dsp::delay` (Mite Delay) | 2 |
| Reverb | `mite_dsp::reverb` (Mite Verb) | 2 |
| Mod | `mite_dsp::modulation` (Mite Mod) + a new tremolo mode | 2 |
| Doubler | `mite_dsp::doubler` (Mite Double) | 2 |
| Tuner | `mite_dsp::pitch` (Mite Tuner), mutes while tuning | 2 |
| Wah / auto-wah | new | 4 |
| Octaver / pitch | new (`mite_dsp::modulation` micro pitch as a start) | 4 |

## 3. Views

- **Board** (always): the chain as tiles; a selected block's **simple view** below it (its few
  main knobs, pedal-style).
- **Full view** (slice 3): a block from one of our plugins opens that plugin's own editor inside
  the Rig, bound to the block's parameters.

## 4. Parameters, state, automation

- Pedal settings live in the Rig's state (`board` field: the chain, each block's kind, values,
  bypass, file and TONE3000 source), with its own undo and board presets; a plugin's host
  parameters are a fixed list, a board is not.
- Host parameters: input, output, mix, bypass, and (slice 5) 8 macro knobs and 4 macro switches,
  each mappable to any block parameter.

## 5. Engine

- The board compiles (worker thread) into a graph of boxed blocks; it arrives by `Handover` and
  replaces the running one with a crossfade. Knob moves never rebuild: each block reads its
  values from shared atomics (smoothed inside).
- The audio thread never allocates; every block's buffers are sized at build.

## 6. Slices

1. Rename, board model and engine (chain, splits, lane alignment), Capture/Cab/Gate/Tone/Boost
   blocks, board editor (add, remove, move, bypass, select), simple views, TONE3000 into the
   selected block.
2. Our pedals with simple views.
3. Full views.
4. New pedals.
5. Macros and board presets.
