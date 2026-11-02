# MiteFX interaction spec

Source: the "Interaction spec" board on the design canvas (third pass, 2026-10-14). Comp sections added in pass 4 (2026-10-22); they are drawn on the `CompMain` boards. Shape sections added in pass 5 (2026-10-25), as built; they are drawn on the `ShapeMain` boards. Timings refer to `tokens.json` → `motion` (micro 80 ms, fast 140 ms, base 220 ms, slow 360 ms).

## Main display gestures (EQ, Multiband crossovers)

The Comp transfer curve has its own table under **Mite Comp** below.

| Gesture | Empty display | Band node | Spectrum peak |
|---|---|---|---|
| Hover | Help strip shows frequency, note, level | Node grows (micro); help strip names the band | Peak marker + tooltip after 300 ms |
| Click | Adds a Bell at the pointer, Q 1.0, already in hand to drag (user decision, 2026-11-01, as FabFilter); Esc closes the panel | Selects; panel opens (base, enter) | Adds a Bell at the pointer, Q 1.0 |
| Double-click | The first click's Bell stays | Resets band: 0 dB, Q 0.71 | Turns the first click's Bell into a cut on the peak, Q 3.0, −3 dB |
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

---

# Mite Comp

Comp follows everything above. This part covers only what Comp adds or changes. The layout has four parts:

- the **GR history** on the left (the hero);
- the **transfer square** on the right;
- the **stage strip** under the square;
- the **IN / GR / OUT meters** on the far right.

## Stages

A stage is one threshold with one quadrant, ratio, knee, attack, release and range. There are up to 4 stages, and they run in series.

**A higher stage never changes a lower one (user decision, 2026-10-23).** Thresholds and knees are set on the input: a stage starts where the input reaches its threshold, whatever the stages below it do. Editing a stage changes only the curve from its knee up; the stages below it, their nodes and their diamonds stay exactly where they are. A stage rides on what the stages below it leave, so editing a lower stage can move the nodes above it up or down (never sideways). A stage acting below its threshold (expand, upward compress) moves everything under it, by nature.

**Numbering and order**
- Stages are numbered by threshold, from low to high. The number always matches the order of the grips on the history and of the nodes on the curve.
- If a drag carries a threshold past a neighbour's, the two numbers swap when the threshold is released, not during the drag. While dragging, the curve is still drawn monotone.

**Quadrant**

The quadrant is chosen by which side of the diagonal the curve is pulled to, and on which side of the threshold:

| Pulled… | Below threshold | Above threshold |
|---|---|---|
| below the line | expand (downward) | compress |
| above the line | upward compress (lift quiet parts) | upward expand |

**Shading on the curve**
- Gain reduction is shaded between the curve and the diagonal in `sem.gr`.
- Upward gain is shaded in `sem.gainUp`.

**Range**
- Each stage's gain is clamped to its range.
- On the curve, this shows as the curve bending back parallel to the diagonal once the range is used up.
- No extra mark is drawn for it.

## Transfer-curve gestures

| Gesture | Empty square / on the diagonal | Stage node (on the curve) | Threshold diamond (on the diagonal) |
|---|---|---|---|
| Hover | Help strip shows IN → OUT at the pointer | Node grows (micro); help strip names the stage, quadrant, threshold and ratio | Diamond grows; help strip shows the threshold |
| Click | Clears selection | Selects; the stage panel opens (base, enter) | Selects the stage |
| Drag | **Pull the line to create a stage** (below). Lasso does not exist here. | Along the diagonal moves the threshold. Away from the line sets the ratio, from the node's distance to the curve the stages below make. | Threshold only |
| Shift-drag | — | Fine ×0.1 | Fine ×0.1 |
| Alt-drag | — | Knee: 0–24 dB over 120 px, horizontal | Knee |
| Cmd/Ctrl-drag | — | Axis lock after 6 px (threshold only or ratio only) | — |
| Double-click | Creates a 4:1 compress stage at the pointer's input level | Resets the ratio to 1:1, which leaves the stage inert. Double-click again to delete it. | Resets the threshold to −18 dB |
| Wheel | — | Ratio, using the Ratio row of the type table | Threshold 0.5 dB per notch (Shift 0.1 dB) |
| Right-click | Square menu: Reset all, Copy curve, Paste curve | Stage menu: quadrant icons, Solo, Bypass, Delete | Stage menu |
| Delete / Backspace | — | Deletes the stage (fast, exit); the curve springs back | Same |

The node sits 12 dB into the active side of its threshold, on the curve its stage and those below it make. That puts it where the ratio is visible, rather than on the knee. When a higher stage starts before the node, the node keeps its place and its stage's own continuation to it is drawn dashed. The threshold diamond sits where the stage starts, on the curve the stages below it make.

## Creating and removing stages

**Creating a stage**
1. Press anywhere on the diagonal or curve, away from a node, and pull perpendicular to the line.
2. After 6 px, a ghost curve appears in the accent at 50 %. The help strip reads `New stage n · quadrant · threshold · ratio`.
   - The threshold is the input level where the drag started.
   - The quadrant comes from the drag direction and which side of the existing thresholds the drag started on.
   - The ratio comes from the drag distance: 4:1 at 48 px, ∞:1 at 160 px.
3. Release to create the stage (spring.node). The new stage gets its number in threshold order, and the strip card animates in at base.
4. Esc, or dragging back to within 6 px of the line, cancels with no undo entry.

**Limits and removing**
- A fifth stage is refused. The ghost turns `text.disabled`, and the help strip reads "Four stages is the limit."
- Delete a stage with Delete / Backspace, the bin in the stage panel, or the stage menu.
- A deleted stage can be brought back with undo.
- Setting a stage's ratio to 1:1 keeps it, but makes it inert. Its strip card dims to 35 %.

**Empty state**
- With no stages, the curve lies on the line and the plugin passes audio unchanged.
- The history still draws IN and OUT.
- Ghost grips sit at −18 and −30 dB. The only hint is the help-strip text.

## Thresholds on the history

Each stage has a horizontal line across the history at its threshold. The line is in the accent at 55 %; the selected one is at 100 % with glow. It ends in a numbered grip (34 × 22, 32 px hit area) at the right edge of the history.

| Gesture | Result |
|---|---|
| Hover the line or grip | Line goes to 100 %; help strip shows the threshold and how much of the last 6 s crossed it |
| Drag the grip, or the line anywhere | Threshold, 10 px/dB at 100 % scale; the curve updates live |
| Shift-drag | Fine ×0.1 |
| Click | Selects the stage (same as its node) |
| Double-click | Resets to −18 dB |
| Wheel on grip | 0.5 dB per notch, Shift 0.1 dB |

**What the history draws**
- IN, pre-processing, in `preSpectrum` grey.
- OUT in `curve` white with glow.
- GR hangs from the top edge, 10 px per dB, in `sem.gr`.
- Upward gain rises from the bottom edge in `sem.gainUp`.

GR and upward gain are never drawn on the same baseline.

## Stage panel

**Placement**
- Docked bottom-left over the history, 10 px above the help strip. Like the EQ band panel, it never moves.
- It stays away from the transfer square, so the curve being edited is never covered.
- Close it with ×, Esc, or a click on empty history.

**Contents**
- **Envelope scope:** the stage's real attack/release response to a burst. Drag the attack handle horizontally for attack and the release handle for release, using the time rows of the type table.
- **Quadrant:** a 2 × 2 icon radio. Changing the quadrant mirrors the curve across the diagonal or across the threshold, and keeps the ratio.
- **Detector:** a dropdown with Peak, RMS or Loudness.
- **Dials:** THRESHOLD (level), RATIO (ratio), KNEE (0–24 dB, linear), ATTACK and RELEASE (time), RANGE (0–60 dB, linear).
- **Bin:** deletes the stage.

## Character, Smart, sidechain, stereo, detector

All four live outside the main display, so the display stays the picture of the sound.

**Character** (footer dropdown, glyph + name)
- Options: Clean · Punch · FET · Opto · Bus · Vari-Mu · Custom.
- Character changes how stages *move*, not the static curve: attack shape, release curve, program dependence and drive.
- Hovering a menu row auditions it. Leaving the menu without clicking restores the previous character.
- The current choice is tinted in the accent; there is no checkmark.
- **Custom** opens a wide docked panel with six dials: ATTACK ×, RELEASE ×, DRIVE, TAIL, PROGRAM, BEND. A "from Opto" note shows which character it started from.
- Changing any of the six dials while another character is active switches to Custom and keeps the values.

**Smart** (footer dropdown)
- Options: Off · Adaptive · Vocal · Master.
- When on, Smart slides every threshold to hold a target GR.
  - The thresholds you set stay as dashed lines.
  - The live, slid thresholds are drawn solid.
  - The GR meter gets a target tick in `sem.gr`.
- Dragging a threshold while Smart is on moves the dashed line you set. Smart keeps sliding relative to it.
- The help strip shows the slide in dB.

**Sidechain** (header key button)
- Opens the docked sidechain panel. It contains:
  - a source dropdown, Internal or External;
  - **Listen** (headphones, drawn in `sem.sidechain` when on);
  - a 3-band sidechain EQ that uses the band ramp and the EQ's node gestures.
- **Listen** replaces the plugin output with the filtered detector signal. While it is on:
  - the display frame glows in `sem.sidechain`;
  - a "Detector" pill sits over the history;
  - Esc or a second click turns it off.
- Listen is never saved in presets.

**Stereo** (header link button)
- A popover with L/R or M/S icons and a LINK dial (0–100 %, Percent row).
- When unlinked (link below 100 %), the history draws two GR traces: solid for L or M, dashed for R or S. The GR meter splits in two, with L/R or M/S icons under it.

**Detector & output** (header sliders button)
- A wide docked panel containing:
  - the topology icons, Feed-forward or Feedback;
  - oversampling (1× / 2× / 4×);
  - LOOKAHEAD (0–20 ms, time);
  - HOLD (time);
  - MAKEUP, with its auto toggle.
- Lookahead and oversampling change LATENCY in the footer at once, and the host is re-told on release.

**Footer toggles**
- **Auto makeup:** while on, the MAKEUP dial follows and shows its value but cannot be dragged. Dragging it turns auto off.
- **Δ (delta):**
  - Outputs input minus output, so you hear only what the compressor removes.
  - The history's OUT fill rises to 30 %, and a "Removed signal" pill shows.
  - Δ is never saved in presets.

## Comp keyboard focus order

- **Header:**
  1. Menu.
  2. Presets ‹ ›.
  3. Undo, then redo.
  4. Snapshots A–D.
  5. Sidechain, then stereo, then detector & output.
  6. ?, then settings.
- **Display:**
  1. Threshold grips on the history, low to high.
  2. Stage nodes on the curve, in the same order. Arrow keys move between stages.
  3. The empty square, where Enter creates a stage at −18 dB.
- **Stage strip:** cards 1–4. Enter opens the stage panel.
- **Stage panel:**
  1. Quadrant radio, then the detector dropdown.
  2. The envelope scope's attack, then release handles.
  3. The dials, left to right and top to bottom.
  4. Bin, then ×.
- **Help strip:** skipped.
- **Footer:**
  1. Character, then Smart.
  2. Auto makeup, then Δ, then bypass.
  3. Mix, then pan, then output.

**Keys on a focused stage**

| Key | Action |
|---|---|
| ↑ ↓ | Threshold ±0.5 dB (Shift ±0.1) |
| PgUp / PgDn | Ratio, Ratio row of the type table |
| ← → | Previous / next stage |
| Q | Cycles the quadrant |
| N | New stage |
| Del | Delete |
| S | Solo |
| B | Bypass |

**Solo**
- You hear only that stage's gain change.
- Other stages draw at 35 %.
- The soloed node gets a white ring and an S badge.

## Comp touch

| Gesture | Result |
|---|---|
| Pinch on the selected node | Knee |
| Two-finger vertical drag on a grip | Fine threshold ×0.1 |
| Long-press on the diagonal | Square menu |

Under 720 px wide (and in the 640 × 400 compact layout):
- The transfer square becomes a 132 px inset over the history.
- The stage panel becomes a bottom strip of THR · RATIO · ATK · REL dials, plus **More**.

## Comp reduce motion

- **History:** it keeps scrolling. It is data, like the meters. The scroll is continuous, not stepped.
- **Live dot:** the dot on the curve stays and moves at audio rate.
- **Trail:** the dotted trail behind the dot is dropped, because it reads as a motion effect. The dot alone shows where the signal is on the curve.
- **Smart:** the threshold slide jumps to the current value at the UI frame rate, with no easing. The dashed set-point lines stay.
- **Create/delete:** the springs become `ease.standard` at fast.
- **Listen frame glow:** static, with no pulse.

# Mite Shape

As built (2026-10-25) from the `ShapeMain` boards and the canvas note `shapeAnswers`. One band is the same screen as several: a 36 px crossover rail with ghost grips at both ends, which grows to 80 px (base, 220 ms) once a crossover is made.

## Crossover rail

| Gesture | Empty rail (in a band) | Crossover grip | Ghost grip (an end) |
|---|---|---|---|
| Hover | Band highlighted; help strip names it, its range, attack and sustain | Grip lit; help strip shows its frequency | Ghost at full opacity; help strip says pull to split |
| Click | Selects the band (its card goes wide) | — | — |
| Double-click | Splits there (a sixth band is refused); both sides keep the band's settings | Merges: the selected band survives, otherwise the lower | — |
| Drag | — | Sideways; kept ⅓ octave clear of its neighbours. Onto a neighbour (within 6 px) or 10 px off the end: release merges, the growing band survives | Pull in: a new crossover where it is released (past 6 px) |
| Shift-drag | — | Fine ×0.1 | — |
| Wheel | — | Frequency ×2^(1/6), Shift ×2^(1/48) | — |
| Right-click | Band menu: solo, mute, copy, paste, reset, merge down or up, remove | — | — |

## Band cards and scopes

- One card per band, always visible, numbered by frequency; the selected one is 644 px wide with six dials (ATTACK, SUSTAIN, TIME, TRANSITION, MIX, OUTPUT) and the scope's handles; the others have the scope and two small dials (ATTACK, SUSTAIN). Curved funnels tie each rail region to its card.
- Header buttons: S (solo: white ring, other bands at 35 %), mute (hatch, trace hidden), bin (wide card only: the band's region joins the one below; the lowest band's joins the one above, which takes its place).
- Scope (a test hit through the band's real detector, square-root time 0–400 ms): drag before the handover line for attack, after it for sustain (vertical, Shift fine); drag the pill on the line sideways for transient time; Alt-drag sideways for transition; double-click a part resets it.
- Attack and sustain dials: ±6 dB takes 60 % of the sweep. Time: 10–50 ms takes half. Length: 50–500 ms takes most of it; the top is endless (∞).
- Sustain length: a pill at the scope's top where the length runs out (a dashed line marks the release); drag it sideways. Past the time axis, or endless, it sits at the right edge; dragging off the edge is endless.

## Clip

- Footer toggle (a curve easing into a ceiling). While on: the ceiling is a line across the level lane with a grip at its right end; drag it up or down (Shift fine), Alt-drag sideways for softness (0–100 % over 200 px), double-click resets both. What the clipper takes off is filled in `gr` between the level going in and the output.

## Keys (Shape)

| Key | Does |
|---|---|
| ← → | Previous / next band (by frequency) |
| ↑ ↓ | Attack of the selected band ±0.5 dB (Shift ±0.1) |
| S · M | Solo · mute the selected band |
| Del | Remove the selected band (several bands) |
| Esc | Closes, in order: menu, a crossover being pulled in, Listen, the open sheet, solo |

## Header and footer

- Header after the snapshots: stereo (L/R or M/S and link, lit when M/S or unlinked), detector (lit in the sidechain colour while Listening).
- Detector sheet: Listen, the detector filter (one band only; drag its nodes, double-click opens a node again) over the input spectrum with what the detector hears in the sidechain colour, FLOOR and LOOKAHEAD dials.
- Footer: LR4 (several bands), latency, auto gain, clip, delta, bypass, MIX, PAN, OUTPUT.

## Shape reduce motion

The history keeps scrolling and the meters and spectrum keep their ballistics (they are data). The rail's growth and the regions' breathing are motion: with reduce motion they should be instant and still (not yet wired: the setting is not read by any plugin yet).


## Mite Sat (in-house design, 2026-10-26; docs/design/sat-brief.md)

### Transfer square

| Gesture | Does |
|---|---|
| Hover | Help strip: drive and bias in fixed slots |
| Drag up / down | Drive, 0.12 dB/px (Shift ×0.2) |
| Drag sideways | Bias, 0.7 %/px (Shift ×0.2) |
| Alt-drag up / down | Tone, 0.06 dB/px |
| Wheel | Drive, 0.5 dB a notch (Shift 0.1) |
| Double-click | Drive and bias back to 0 |

A drag is one undo step. The curve is `mite_dsp::sat::transfer` at the live operating point
(Tube's drift) times auto gain; it is lit from −peak to +peak of the band's input (60 ms up,
300 ms down), with a dot at each end. Tape and Transformer: lit is the loop a sine at the band's
peak traces through the real core (`magnet_loop`, at the band's centre frequency), with a dot
where it peaks either way.

### Card

- Model icons: one 44×36 icon button per built model, each drawing its own curve. The current one
  is lit in the accent. Hovering one draws its curve as a ghost in the square (never writes the
  parameter); clicking picks it (a 20 ms crossfade between the curves).
- Power button (top right): the band passes clean but is still heard (`b{N}_bypass`).
- Stereo button (beside power): a list of Stereo, Mid, Side and M/S. Mid or Side saturates that part and leaves the other clean; M/S saturates each on its own. A narrow card shows the mode in the corner of its square when it is not Stereo. A change crossfades.
- Harmonics: H2–H9 bars, read only. Hover: the help strip gives the partial's level against the
  strongest and whether it is even, odd or above Nyquist (hollow). Clicking selects the band.
- Dials: DRIVE (0–12 dB takes half the travel), BIAS, TONE, MIX, OUTPUT, and the dynamics row:
  DYNAMICS (left compresses, right lifts the hits), ATTACK, RELEASE; the threshold is automatic (the
  band's average level). Narrow cards: DRIVE and MIX only. While DYNAMICS is not 0 the square draws
  that live threshold dashed at ±T.

### Footer

Oversampling dropdown (1×/2×/4×/8×, each with as many waves as it oversamples; the help strip says
the latency each adds), latency, auto gain, Δ, bypass, MIX, PAN, OUTPUT.

### Bands (Sat)

The rail, its gestures and the band menu are Shape's (`mite_ui::widgets::rail`): pull a ghost grip
in or double-click the strip to split (the new band copies the band it splits: model, dials,
stereo, bypass, mute); drag a crossover sideways (Shift fine, wheel steps), onto its neighbour or
off an end to merge, or double-click it; right-click a region or card for solo, mute, copy,
paste, reset, merge down or up, and remove. Click a card or region to edit that band; dragging
any card's square edits its band and selects it. The layout eases from one band to several over
220 ms (the spectrum gives the rail 28 px).

### Keys (Sat)

| Key | Does |
|---|---|
| S / M | Solo / mute the edited band |
| Delete | Removes the edited band (its region joins the band below) |
| ↑ ↓ | Drive ±0.5 dB (Shift ±0.1) |
| ← → | Previous / next model |
| Esc | Closes a list or menu, cancels a pull, else lets go of solo |
