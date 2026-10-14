# MiteFX interaction spec

Source: the "Interaction spec" board on the design canvas (third pass, 2026-10-14). Timings refer to `tokens.json` → `motion` (micro 80 ms, fast 140 ms, base 220 ms, slow 360 ms).

## Main display gestures (EQ, Multiband crossovers, Comp transfer curve)

| Gesture | Empty display | Band node | Spectrum peak |
|---|---|---|---|
| Hover | Help strip shows frequency, note, level | Node grows (micro); help strip names the band | Peak marker + tooltip after 300 ms |
| Click | Clears selection | Selects; panel opens (base, enter) | — |
| Double-click | Adds a Bell at the pointer, Q 1.0 | Resets band: 0 dB, Q 0.71 | Adds a Bell at the peak, Q 3.0, −3 dB |
| Drag | Lasso select | x = frequency, y = gain (cut/shelf: x only) | Ghost Bell follows; release commits, Esc cancels |
| Shift-drag | Adds to selection | Fine ×0.1 | Fine ×0.1 |
| Alt-drag | — | Q: 1 octave of width per 120 px | — |
| Cmd/Ctrl-drag | — | Axis lock after 6 px | — |
| Wheel | Zoom frequency axis (Shift pans) | Q ×2^(1/6) per notch, Shift ×2^(1/24) | — |
| Right-click | Display menu | Band menu | — |
| S held while dragging | — | Snap to nearest note | — |

## Per parameter type

Drag = vertical pixels for the full range at 100 % scale. Shift is always ×0.1. No acceleration.

| Type | Range | Mapping | Drag | Wheel / Shift-wheel | Arrows | PgUp/PgDn |
|---|---|---|---|---|---|---|
| Frequency | 10 Hz – 30 kHz | log | 200 px (≈17 px/oct) | 1/6 oct / 1/48 oct | 1 semitone | 1 octave |
| Gain, bipolar | ±30 dB | linear | 200 px (0.3 dB/px) | 0.5 dB / 0.1 dB | 0.1 dB | 3 dB |
| Level, unipolar | −60 … 0 dB | linear | 200 px | 0.5 dB / 0.1 dB | 0.1 dB | 3 dB |
| Q / width | 0.025 – 40 | log | 200 px | ×2^(1/6) / ×2^(1/24) | ×2^(1/12) | ×2 |
| Time | 0.01 – 5000 ms | log | 200 px | ×2^(1/4) / ×2^(1/16) | ×2^(1/8) | ×2 |
| Synced time | 1/64 … 4 bars | stepped | 24 px per step | 1 step | 1 step | 4 steps |
| Ratio | 1:1 – ∞:1 | linear in 1/ratio | 200 px | 0.1, 0.5 or 1, by range | as wheel | ×2 |
| Percent | 0 – 100/200 % | linear | 200 px | 1 % / 0.1 % | 1 % | 10 % |
| Bipolar % / pan | L100 – R100 | linear | 200 px, 6 px detent at 0 | 1 / 0.1 | 1 | 10 |
| Enum | slope, mode | stepped | 24 px per step | 1 step | 1 step | first / last |

## State transitions

| Component | Transition | Duration | Easing |
|---|---|---|---|
| Button, segment | rest → hover | micro | standard |
| Button, segment | press (inset) | fast | standard |
| Toggle | off ↔ on, glow fades in | fast | standard |
| Dial | hover: scale 1.1 · drag: no change | fast | standard |
| Band node | idle → hover / selected | micro / fast | standard / spring.node |
| Band node | create / delete | base / fast | spring.node / exit |
| Band panel | open / close / move | base / fast / 60 ms lag | enter / exit / standard |
| Menu | open (fade + 4 px drop) | fast | enter |
| Sheet | open / close + scrim | slow / base | enter / exit |
| Theme change | whole-window crossfade | slow | standard |
| Meters, dynamic ring | audio rate; meter release 20 dB/s, peak hold 1.5 s | 0 / 300 ms | — |

## Band panel placement

The band panel is docked at the bottom centre of the plot, 10 px above the help strip, and never moves (user decision, 2026-10-14). A panel that followed the node covered the curve around the band being edited. Docking it where FabFilter does keeps that area visible.

## Band colours

One colour per stereo placement, from the band ramp in placement order: Stereo, Left, Right, Mid, Side (user decision, 2026-10-14). Every band of the same placement shares a colour, so the colour says what the band processes; the node number tells bands apart. This replaces one colour per band number.

## Wheel on a node

- Cut bands (low cut, high cut): the wheel steps the slope; up is steeper.
- Every other type: the wheel changes Q.

## Keyboard focus order

- **Header:** menu → presets ‹ › → undo → redo → snapshots A–D → copy → morph → analyzer → ? → settings.
- **Display:** bands in frequency order; arrow keys move between them.
- **Band panel:** type → slope → stereo → solo → bypass → close → dials left to right → dynamic row.
- **Help strip:** skipped; it is read-only and announced politely.
- **Footer:** MIDI → mode → resolution → character → channel → Auto Gain → Delta → Bypass → Mix → Output.
- **Focus ring:** 2 px white with a 2 px offset (3 px in high contrast). It follows the node circle.

## Keys

Esc steps back one level: typed entry → menu → panel → selection → help mode.

| Key | In the display | On a focused dial / field |
|---|---|---|
| ← → | Previous / next band | 1/100 of range; Shift 1/1000 |
| ↑ ↓ | Gain ±0.5 dB (Shift ±0.1) | Step per the type table |
| PgUp / PgDn | Q ×2^(±1/6) | Per the type table |
| Home / End | First / last band | Min / max |
| N · Del · S · B · D | Add · delete · solo · bypass · dynamic | — |
| Enter | Open band panel | Typed entry |
| ? | Toggle help mode | — |
| Cmd/Ctrl-Z, Shift-Cmd/Ctrl-Z | UI undo / redo (separate from host automation) | — |

## Touch (AUv3 / iPad and touch screens)

Targets are 44 px. Under 720 px wide, the band panel becomes a bottom sheet.

| Gesture | Result |
|---|---|
| Tap / double-tap | Same as click / double-click |
| Drag | Same as drag; starting a drag on a node needs 4 px of movement |
| Two-finger pinch on the selected node | Q |
| Two-finger vertical drag on the selected node | Fine gain ×0.1 |
| Long-press 500 ms | Context menu, with a light haptic tick |
| Two-finger tap / three-finger tap | Undo / redo |

## Reduce motion

- Transitions longer than micro (80 ms) become instant; hover colour stays.
- Springs become `ease.standard` at fast, with no overshoot.
- The dynamic ring and node breathing show the current value only.
- The spectrum and meters keep their ballistics: they are data, not decoration.
- Help callouts appear together. The dial's hover scale becomes a tick ring 1 px brighter.
