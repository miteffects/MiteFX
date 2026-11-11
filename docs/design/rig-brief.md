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
| Pedal | NAM, files or TONE3000 (pedals only) | 1 |
| Amp (amp, or combo / full rig) | NAM, files or TONE3000 (amps and combos only) | 1 |
| Cab | IR, files or TONE3000 | 1 |
| Gate | NAM-style gate (Mite Amp's) | 1 |
| Tone | bass/middle/treble + low/high cut (Mite Amp's) | 1 |
| Boost / Volume | clean gain, volume with taper | 1 |
| Comp | `mite_dsp::dynamics` (Mite Comp) | 2 |
| EQ | a seven-band graphic EQ (`mite_dsp::eq` bells) | 2 |
| Tremolo | new: sine, square, harmonic, stereo spread (a block of its own: Mite Mod's state stays) | 2 |
| Delay | `mite_dsp::delay` (Mite Delay) | 2 |
| Reverb | `mite_dsp::reverb` (Mite Verb) | 2 |
| Mod | `mite_dsp::modulation` (Mite Mod) | 2 |
| Doubler | `mite_dsp::doubler` (Mite Double) | 2 |
| Tuner | `mite_dsp::pitch` (Mite Tuner), mutes while tuning | 2 |
| Wah / auto-wah | new: a TPT state-variable band-pass swept heel→toe (pedal, or Auto on the envelope); built 2026-11-11 | 4 |
| Octaver / pitch | new: −1, −2, +1 octave and a free interval (±24 st), each with its level, over the dry; Poly (a shifter splicing where the signal matches best, `pedals::Splicer`) or Mono (flip-flop sub-octaves and a rectified octave up, zero latency); built 2026-11-11 | 4 |

**Built (2026-11-11):** slice 1 and slice 2 (every block of the table but wah and octaver; the
Mod pedal offers Chorus/Ensemble/Flanger/Phaser/Vibrato/Rotary). Each block is drawn as what it
is (stompboxes with their sign, an amp head, a combo for full rigs, a 2×2 cabinet, a volume
rocker); the add menu has two columns.

## 3. Views

- **Board** (always): the chain as tiles; a selected block's **simple view** below it (its few
  main knobs, pedal-style).
- **Full view** (slice 3): a block from one of our plugins opens that plugin's own editor inside
  the Rig, bound to the block's parameters.

**Built (2026-11-11, slice 3):** Comp, Delay, Reverb, Mod, Double and Tuner blocks *are* those
plugins. Each plugin crate is split in two: `plugins/mite-<name>/core` (`mite_<name>_core`:
engine, parameters, editor, presets) and the plugin crate (entry points, nih-plug params), so the
Rig links the cores without a second set of exports. A block holds the plugin's whole parameter set
by its frozen ids, runs the plugin's own engine with its own telemetry, and its **Full view**
(button, or double-click the block) draws the plugin's editor in the plugin's colour, with ‹ Rig
(or Esc) back to the board. The simple view shows a few of the same parameters. The plugins'
settings functions allocate, so settings are computed off the audio thread (editor tick, loader
thread, and again when the host's tempo moves) and the unit copies the newest under a try-lock that
never waits. Boards saved by slice 2 load with their pedal values renamed to the plugins' ids (the
delay's tone and the doubler's width and level had no counterpart and go). Not yet: a plugin's
snapshots A–D inside a block are not saved with the board; its user presets are the plugin's own.

- **Splits** (2026-11-11, user): a Mix (the lanes against the dry input, in time) and a routing:
  Parallel, L / R (lane 1 the left, lane 2 the right) or M / S (lane 1 the mid, lane 2 the side);
  a third lane always gets the whole signal. A routing change dips for 5 ms (no click).
- **Palette** (2026-11-11, user): one row in a guitarist's order, no group headings (the user
  dropped them: they cost the icons room), each icon as large as its slot allows; the + menu in
  the same order, in three columns. Under the simple view, every block (and a split) as an icon,
  always there: drag one onto a + in the chain, or click to add it after the selected block. The +
  menu shows the same icons. Each kind wears its plugin's accent (Comp's, Delay's, …; captures and
  cabinets the Rig's), and a capture or cabinet loaded from TONE3000 shows the tone's cover photo
  on its block (saved with its source; files loaded before this have none until loaded again).

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
