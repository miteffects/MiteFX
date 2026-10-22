# Brief: Mite Comp (design pass 4)

**For:** Claude Design, on the MiteFX canvas (https://claude.ai/artifact/W6i7s8rA3Q81U43ruRusH5).
**From:** MiteFX engineering, 2026-10-22.
**Goal:** every screen and component Mite Comp needs, in the system you already built for Mite EQ, so that engineering can build it without guessing.

This brief is self-contained. Where it says "as in the EQ", the EQ boards on the canvas are the reference, *with the corrections in §2* (the shipped EQ moved on from some boards).

---

## 1. What Mite Comp is

MiteFX is an open-source (GPL) suite of pro audio plugins (CLAP, VST3, later AU), written in Rust and drawn on the GPU (Vello vector paths, plus real WGSL bloom and frosted-glass blur passes). Mite EQ is built and shipped. Mite Comp is the second plugin.

**Rival:** FabFilter Pro-C 3. The bar is to *look and feel better than FabFilter*, not to copy it.

**What it does, in one paragraph:** a compressor whose transfer curve is drawn and edited directly. The curve is made of up to **four stages in series** (like compressors patched one into the next). Each stage works in one of four quadrants: compress above the threshold, expand (gate) below it, upward-compress below it, or upward-expand above it. So one curve can be a leveller under a limiter, or a gate under a compressor. A **Character** reshapes how the stages move (attack and release behaviour, program dependence, how hard the ratio gets when driven), from clean VCA to Vari-Mu. **Smart** modes adapt the compressor to the material. The detector can listen to the track or to an external **sidechain**, filtered by its own small EQ.

---

## 2. The design system as built (binding)

Use the tokens, components and rules already on the canvas (`mitefx-tokens/3`). These points are settled. Some differ from the second-pass boards because the user corrected them while the EQ was built.

### 2.1 The user's taste (the most important section)

The user rejected each of these in turn on the EQ, mostly by screenshot. Treat them as hard rules.

1. **Direct manipulation over switches.** Things are made by pulling: grips on the graph, a hidden ring around a dial. Avoid "+ Add feature" buttons and mode toggles where a gesture can say it. (EQ example: a band becomes dynamic by dragging a grip above or below its node; dragging it back to zero makes it static. There is no "Dynamic" switch.)
2. **Visual over numeric. No explanatory words in controls.** No "gain now" readouts, no captions like "QUIET / LOUD" or "DUCKS WHEN LOUD". Dials carry a short name and their value; everything else is said by the **help strip** (one line under the display, which is the UI's only tooltip).
3. **Every curve is smooth.** No corners, kinks, needles or V-shaped dips: transfer curves, envelopes, meters, crossfades. Transitions are S-shaped, never linear ramps.
4. **Honest visuals.** Every graph is computed from the real DSP. A control never shows more than it does. (So the transfer curve you draw must be a monotone function of input level, and the GR display must be what the gain cell applies.)
5. **Dials have feel.** Common values get more travel (custom dial curves); no stepping or snapping while dragging.
6. **Menus:** the current choice is highlighted in the owner's colour (here the accent), never a checkmark. Options have icons.
7. **No crowding.** Controls never overlap; keep room around close, bin and power buttons.
8. **Quality before features.** The UI runs at the display refresh rate, at a comfortable default size. Don't hide cost by shrinking the design.

### 2.2 Shared chrome, as shipped in the EQ

- **Window:** 1280 × 760 design units at 100 %, freely resizable with the aspect kept. Compact layout at 640 × 400.
- **Header (left to right):** the mite mark and "Mite Comp", then ‹ preset name ›. The arrows step through presets; clicking the name opens the list (Save as…, FACTORY, YOURS; the current one lit; a dot after the name when the sound has moved on). Then undo and redo (dim when empty), then the snapshot letters **A B C D** (the edited one lit, empty ones dim; click to switch, drag a letter onto another to copy it). **There is no morph slider:** morphing was built and dropped as overkill. Plugin-specific header buttons go after the letters (the EQ has its Match button there). All header controls are 32 px icon buttons.
- **Display:** the big interactive area.
- **Help strip** (28 px) under the display: one line describing what is under the pointer, with values in fixed-width slots so the words don't jump.
- **Footer (left to right):** plugin-specific mode segments, then a latency readout; on the right, three icon toggles (auto gain: a boost brought back to the line; delta: Δ; bypass: power), then MIX, PAN and OUTPUT dials with name and value *beside* each dial.
- **Floating panel:** glass, docked at the bottom of the display, placed away from the thing being edited; it grows into the display instead of moving the controls under the pointer. Close (×) and remove (bin) buttons sit at its right edge.
- **Icon buttons:** one look for all. On hover the border lights in the owner's colour and the glyph brightens; when on, it is also tinted and the glyph takes the colour.
- **Glass and glow are real now:** panels, menus and the preset list blur what lies under them (18 px, saturate 1.2); glowing strokes get true bloom. You can rely on both.

### 2.3 Colours for Comp

- **Accent:** `comp` = azure: dark `#54B3FD`, light `#15466A`, hc `#86C5FA`, cb `#238EA9`. Modulation and selection use the accent.
- **Gain reduction** is always `gr` (`#F88A3D` dark) and always hangs from the top, or from the 0 dB line.
- **Sidechain** is `sidechain` (`#A35DE4` dark).
- **Band ramp** (sidechain EQ bands): one colour per stereo placement, as in the EQ.
- Upward gain (upward compression or expansion) needs a colour that isn't `gr`. **Decide one** (a token, e.g. `gain-up`), distinct from the ramp, the accent and the semantic colours, in all four themes.

### 2.4 Components you already drew that Comp reuses

From `shared.dc.html`: the **4-node transfer curve** (input −60…0 → output), the **waveform/GR history** (scrolls right to left over 6 s; pausing keeps 30 s), and the rule that transfer-curve nodes reuse the EQ node states (idle, hover, selected, disabled, plus solo). From the EQ: the tick-scale dial, the level meter, dropdowns, the band panel, the dynamics scope.

---

## 3. What the user can control

These are the parameters engineering plans. Ranges are for your scales and readouts; mappings follow `interaction.md` → "Per parameter type". **Bold** ones belong in the main view; the rest can live in panels or sheets.

### 3.1 Per stage (four stages; stage 1 is always on)

| Control | Range | Default (stage 1) | Type |
|---|---|---|---|
| **On** | off / on | on | toggle (stages 2–4 start off) |
| **Quadrant** | Compress · Expand · Upward compress · Upward expand | Compress | enum, **should follow from where the stage is pulled** (see §4.2) |
| **Threshold** | −60 … 0 dB | −18 dB | level |
| **Ratio** | 1:1 … ∞:1 | 4:1 | ratio |
| Knee | 0 … 24 dB | 6 dB | level |
| **Attack** | 0.05 … 250 ms | 10 ms | time |
| **Release** | 5 … 2500 ms | 120 ms | time |
| Range | 0 … 60 dB (down), 0 … 24 dB (up) | 60 / 12 | level (how far the stage may move the gain) |
| Detector | Peak · RMS · Loudness | RMS | enum (per stage) |

Defaults for stages 2–4 when first made: a leveller (upward compress at −32 dB, 2:1, slow), a gate (expand at −48 dB, 4:1, fast), a limiter (compress at −6 dB, 20:1, very fast).

### 3.2 Global

| Control | Range | Default | Notes |
|---|---|---|---|
| **Character** | Clean · Punch · FET · Opto · Bus · Vari-Mu · Custom | Clean | 7 choices, each needs a **glyph** |
| Custom character | attack ×0.1–4, release ×0.1–4, detector drive −6…+12 dB, slow tail %, program dependence %, ratio bend % | as Clean | only with Custom; six dials |
| **Smart** | Off · Adaptive · Vocal · Master | Off | glyphs needed; Adaptive bends the release to the material, Vocal and Master also slide the whole curve to hold a target |
| Target GR | 0 … 24 dB | 6 dB | Vocal and Master only: the average gain reduction Smart holds |
| Smart amount | 0 … 100 % | 70 % | |
| Sibilance guard | 0 … 100 % | 50 % | Smart: eases the detector off esses |
| Hold | 0 … 500 ms | 0 | |
| Lookahead | 0 … 5 ms | 0 | adds latency (shown in the footer) |
| Topology | Feed-forward · Feedback | Feed-forward | glyphs |
| Stereo | L/R · Mid/Side, with **link** 0 … 100 % | L/R, 100 % | continuous link |
| Sidechain source | Internal · External | Internal | external = the plugin's sidechain input |
| Sidechain EQ | up to 6 bands (Bell, shelves, cuts, tilt), as EQ bands | none | plus **Listen** (hear the detector signal) |
| Makeup | 0 … +24 dB | 0 | **auto makeup** toggle beside it: follows the curve, never pumps |
| Mix, Pan, Output | as the EQ footer | 100 %, 0, 0 dB | |
| Delta, Bypass | toggles | off | as the EQ footer |
| Oversampling | Off · 2× · 4× · 8× | Off | for the gain cell's saturation (Character colouring) |

### 3.3 What the plugin shows (telemetry, live at the display rate)

Input and output level (peak and RMS) per channel; gain reduction per channel and per stage; the detector level (the dot on the transfer curve); Smart's current threshold offset; the latency.

---

## 4. Screens and interaction to design

### 4.1 Main view (the heart of it)

The display must answer three questions at a glance: *what is coming in, what is the compressor doing to it, and what shape is the curve.* A proposal to react to (improve or replace it, and say why):

- **History** fills most of the display: the input level as a filled waveform envelope (dim), the output on top (lit), and gain reduction hanging from the top in `gr`, all scrolling right to left. Upward gain rises from the bottom (or from the 0 line) in the upward colour. Each active stage's threshold is a horizontal line across the history that can be dragged up and down, in the accent. Pro-C does this; ours should be smoother, with real glow and no stepped meters.
- **Transfer curve** on the right as a square (−60…0 dB in and out), with the 1:1 diagonal, the curve, a node per stage (EQ node states), knees drawn as true curves, and a live dot riding the curve at the detector level, with a faint trail. The curve area shades between the curve and the diagonal: compression in `gr`, upward gain in the upward colour.
- **Meters:** input and output on the right edge (as the EQ's OUT meter), plus a GR meter that hangs from the top. No numbers on them. Peak values belong in the help strip.

### 4.2 Making and editing stages (direct manipulation)

The quadrant should come from **where** the user pulls, not from a menu. Proposed rule: on the transfer curve, grabbing the curve and dragging it below the diagonal at a level above the existing thresholds makes a compress stage there; below the diagonal at a lower level makes an expand (gate) stage; above the diagonal makes upward stages (upward compress below the threshold, upward expand above it). A stage's node then moves along x for threshold and away from the diagonal for ratio; Alt-drag sets the knee; the wheel sets the ratio. Double-clicking a node resets it, and pulling a stage back onto the diagonal (ratio 1:1) removes it. **Design the affordance:** how does the user discover that the curve can be pulled (ghost grips on hover, as the EQ's ghost grips above and below a node)? How are four stages told apart (number on the node, as EQ bands)? What happens when two thresholds cross (they are in series, so the order on the level axis is the order of the stages)?

### 4.3 Stage panel

Selecting a stage opens the docked glass panel: threshold, ratio, knee, attack, release, range and detector for that stage, with a quadrant indicator (icon) and remove (bin). Attack and release should feel as they do in the EQ's dynamics row (readable values, custom dial curves). Consider a small **envelope scope** like the EQ's transient scope: a synthetic hit run through the stage's real detector, drawn as it rises and falls, with attack and release draggable on it.

### 4.4 Character

Seven characters, each with a glyph (Clean, Punch, FET, Opto, Bus, Vari-Mu, Custom). Where does the picker live? Options: footer segments, a header dropdown, or a strip in the display. The choice changes the curve's motion, not its shape, so the history is where you see it; consider showing the attack and release "lines" the character draws. Custom opens its six dials (§3.2).

### 4.5 Smart

Off / Adaptive / Vocal / Master with glyphs, plus Target GR, amount and sibilance guard. When Vocal or Master is on, the whole curve slides as it tracks; show the slide honestly (the curve and its thresholds move, perhaps with a ghost of where the user set them). Target GR could be a line on the GR meter or history that can be dragged.

### 4.6 Sidechain

A sidechain sheet or panel: the source (internal or external; a dropdown with icons like the EQ's detection-source menu), **Listen**, and the sidechain EQ as a small EQ display reusing the EQ's nodes, curve and spectrum (in `sidechain` colour), with up to 6 bands. While Listen is on, the plugin outputs the detector signal; make that state impossible to miss.

### 4.7 Stereo

L/R or Mid/Side with a continuous link. With link below 100 %, the two channels' GR differ: show both (two GR traces, or the meter split), labelled by icons (L/R or M/S), never by words.

### 4.8 Footer and header

Apply §2.2. The footer's left side holds Comp's mode controls (your choice among Character, Smart, topology and stereo, by importance); the latency readout appears when lookahead or oversampling adds latency. Delta and Bypass as in the EQ. The header gets the shared controls, then any Comp-specific buttons (e.g. sidechain).

---

## 5. Deliverables

Into the canvas, then exported to `docs/design/` as before:

1. **`comp-main.dc.html`** with props `theme` = `dark` · `light` · `cb` · `hc` and `variant` =
   - `default` (one compress stage, audio running, GR moving),
   - `stages` (four stages: leveller, gate, compressor, limiter),
   - `stage-selected` (panel open, envelope scope),
   - `create` (pulling a new stage out of the curve: ghost grips, the stage forming),
   - `character` (picker open; and Custom with its six dials),
   - `smart` (Vocal or Master tracking: the curve slid, target shown),
   - `sidechain` (external source, sidechain EQ with 3 bands, Listen off; and a Listen-on state),
   - `stereo` (Mid/Side, link 50 %, two GR traces),
   - `lookahead` (latency shown in the footer),
   - `delta` (Delta on),
   - `empty` (Init: the curve on the diagonal; the help strip says it passes audio unchanged, and the plugin nulls there),
   - `presets`, `help` (help mode with numbered call-outs), `menu` (any dropdown open).
2. **`comp-compact.dc.html`** at 640 × 400.
3. **Component additions** to the component sheet: GR meter (vertical and in the history), stage node states including the four quadrant glyphs, the 7 Character glyphs, the 4 Smart glyphs, topology and detector glyphs, the envelope scope, and the threshold line with its grip.
4. **Tokens:** the upward-gain colour (§2.3) in all four themes, and anything else new, with contrast checked against the display, the accent, `gr` and the ramp (ΔE ≥ 15 from ramp and semantic colours, as for the accents).
5. **`interaction.md` additions:** the transfer-curve gestures in the main-display table (the "Comp transfer curve" column), dragging thresholds on the history, creating and removing stages, the Character, Smart and sidechain interactions, keyboard focus order for Comp, and what reduce-motion changes (the history keeps scrolling; does the trail stay?).

## 6. Technical constraints (unchanged)

- Everything is vector paths, gradients, and blur or glow passes. Raster only for subtle noise.
- Manrope only (OFL). Tabular figures for readouts.
- Animations are property tweens with the motion tokens (micro 80, fast 140, base 220, slow 360 ms; standard, enter, exit, spring.node).
- Hit targets 32 px; menu rows 30 px; touch-ready (nothing hover-only).
- Everything the history and curve draw must be computable at 120 fps from per-block telemetry: no per-sample drawing of long windows. The history is a level envelope, not a raw waveform.

## 7. Questions to answer in your notes

1. History plus curve side by side, or one layered over the other? Which is the hero at 640 × 400?
2. Where do Character and Smart live: footer, header or display?
3. How are the four quadrants recognisable without words: glyphs, curve shading, node shapes?
4. How does the user see that a stage is limited by its range (the curve flattening where the range runs out)?
5. Two-channel GR (stereo link < 100 %): two traces, or one with a spread?
