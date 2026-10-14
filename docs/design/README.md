# MiteFX design: first pass

**Source canvas:** https://claude.ai/artifact/W6i7s8rA3Q81U43ruRusH5 (Claude Design, 2026-10-14). The canvas is the living version. The files here are a snapshot and the implementation contract.

| File | What it is |
|---|---|
| `tokens.json` | Machine-readable tokens taken from the artboards. `mite-ui::tokens` is generated from this file (BLUEPRINT §7.4). |
| `mockups/eq-main.dc.html` | Mite EQ main screen at 1280×760: 4 bands, with dynamic band 3 selected. Interactive: band selection, snapshots, mode, channel and focus segments. |
| `mockups/components.dc.html` | Component sheet: knobs and knob states, sliders, toggle/segmented/dropdown, numeric field states, meters and loudness, band-node states, buttons, crossover bar, piano-roll strip. |
| `mockups/tokens.dc.html` | Brand, the 15 per-plugin glyphs and accents, dark and light neutrals, band ramps, semantic colours, type, spacing, radii, elevation, glow, blur, motion. |

The `.dc.html` files are Claude Design component pages. They only render inside the canvas, not on their own. Treat them as reference source: the geometry maths in their scripts (knob tick arcs, the log-frequency mapping, node and panel placement) is meant to be ported.

## Design decisions (now binding)

- **Brand:** the wordmark is `miteFX` in lower-case Manrope ExtraBold, with "FX" in the plugin's accent. The mark is a monoline mite seen from above, carrying the plugin's signal glyph.
- **Font:** **Manrope only** (OFL). Every readout uses tabular figures (`tnum`).
- **Look:** a near-black blue-grey neutral base (`#0A0C0F` window, `#0E1116` display).
  - Raised controls use an inset highlight plus a two-layer drop shadow; fields and segment tracks sit inset.
  - **Glass** floating panels: 72 % fill, 18 px backdrop blur, saturation 1.2.
  - **Glow** on the total curve, lit knob ticks, knob pointers, meters and latched toggles.
- **Knob:** a tick-scale dial with 31 ticks over 270°. Lit ticks glow in the band or accent colour, and a white pointer has a coloured glow. The value and unit sit inside the dial. Bipolar knobs light from the centre tick. Hovering scales the dial ×1.1.
- **Band node states:** idle, hover, selected, dynamic (breathing dashed ring), solo (amber ring, all other bands dimmed), bypassed (hollow and dashed), disabled.
- **EQ layout:**
  - A **header** (56 px): menu, preset browser, undo/redo, A/B/C/D, copy, Morph, analyzer quality, help, settings.
  - The **display**, with an OUT meter on the right edge.
  - A **help strip** (28 px).
  - A **footer** (56 px): MIDI, processing mode, latency, Character, channel mode, Auto Gain, Delta, Bypass, Mix knob, Output knob.
- **Band panel:**
  - Glass, 476 px wide. It floats above or below the node, never covering it.
  - Header row: type, slope, stereo placement, solo, bypass, close.
  - Knob row: FREQ, GAIN and Q, plus RANGE and THRESH when the band is dynamic.
  - Dynamic row: on/off, Detect source, Focus (Both / Transient / Sustain), live GR readout. A band that isn't dynamic shows "+ Make dynamic" and "+ Spectral" instead.
- **Typed entry** accepts units, suffixes and note names (`3.5k`, `A4`, `-3`). Enter commits, Esc reverts, double-click resets to default.

## How the mockup's CSS/SVG effects map to the Rust renderer

| Mockup effect | `mite-ui` implementation |
|---|---|
| `feGaussianBlur` + merge (curve, tick and meter glow) | WGSL bloom pass: separable Gaussian on an offscreen layer, then additive composite. Blur radius comes from `tokens.glow.*`. |
| `feDropShadow` in the accent (knob pointer) | The same glow pass, run on a single-path layer |
| `backdrop-filter: blur(18px) saturate(1.2)` (glass panel) | Kawase/dual-filter blur of the display layer, cropped to the panel's rounded rect, then a saturation matrix, then the 72 % fill |
| Multi-layer `box-shadow` (raised, inset) | Vello blurred rounded-rect primitive (`draw_blurred_rounded_rect`), one call per layer |
| `repeating-linear-gradient` meter segments | Instanced rects in one Vello path, not a gradient |
| `text-shadow` on latched buttons | Text drawn into the glow layer once |

## Review: what to fix in the next design pass

1. **Colour collisions.** These undermine the accent and semantic rules:
   - Band 1 (`#5CC8FF`) is the EQ accent, which contradicts "bands never use the accent". Fix: the ramp starts at a hue that differs from the plugin's accent, or the accent shifts per plugin.
   - Band 2 (`#FFB547`) is also `sem.warn` and the solo ring. A soloed band 2 is ambiguous.
   - Band 3 (`#FF6FA8`) is nearly Shape's accent (`#FF6FB5`).
   - Band 4 (`#8EE07A`) is `sem.ok`.
   - `sem.gr` (`#FF8A3D`) sits next to the Comp (`#FF9F43`) and Sat (`#FF7A45`) accents, so gain-reduction meters in those plugins lose their meaning.
   - The Limit accent (`#FF5E6C`) is close to `sem.collision` (`#FF4D6A`).
2. **Contrast:**
   - The help strip's right-hand hint (`#5C6573` on `#0C0E12`) is **3.3:1**, which fails 4.5:1 for 11 px text.
   - Disabled text (`#4A5260`) is 2.35:1. That's acceptable only because the control is disabled; keep it, but never use it for information.
   - The `text.lo`, label and readout colours pass (5.0–6.2:1).
3. **Hard-coded values not yet in the token sheet.** The `color.control.*` and `color.display.*` groups in `tokens.json` were taken from the mockup markup. Design should confirm them or fold them into the named neutrals.
4. **Deliverables from BLUEPRINT §7.4 still missing:**
   - EQ screens: the empty state, linear-phase mode (latency/resolution UI), collision/masking view, spectrum-grab hover, preset browser, compact size (50–75 % scale and the narrow breakpoint), and the light theme applied to the EQ.
   - Settings sheet, help "?" overlay, right-click context menu, multi-select and lasso.
   - Modulation matrix (shared component), Comp transfer-curve editor, waveform/GR history view.
   - `interaction.md`: gesture map, state transitions and timings. Currently only the help strip and the panel row hint at it.
5. **Touch readiness:**
   - Band nodes have 32 px hit targets and segment buttons are 26 px tall. BLUEPRINT §7.2 asks for ≥ 32 px at 100 % scale.
   - Segments should grow on touch or high scale, or the floor should be agreed at 26 px for desktop.
