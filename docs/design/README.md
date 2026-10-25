# MiteFX design

**Source canvas:** https://claude.ai/artifact/W6i7s8rA3Q81U43ruRusH5 (Claude Design). The canvas is the living version. The files here are a snapshot and the implementation contract.

**Status:** fifth pass (Mite Shape) received and built on 2026-10-25, answering `shape-brief.md`:
- the Shape accent moved from blush to **leaf** (`#6BA208` dark, with its own text tint `accent.shape.text`), far from both `gainUp` and `gr`;
- frequency bands (Shape, Multiband) are drawn in the plugin accent with their number, never in the ramp (`color.rules.frequencyBands`), and the attack/sustain rendering rule (`color.rules.shaping`);
- layout numbers in `tokens.json` → `shape`; the boards `ShapeMain` (16 variants × 4 themes), `ShapeComponents`, `ShapeCompact`.

**Design's answers (canvas note `shapeAnswers`):** the history is the hero; one scope per band, always visible, in cards under the crossover rail (the selected one wide); one band is the same screen with a 36 px rail and ghost grips at both ends; detector filter hidden in multiband; clip later, as a footer toggle and a ceiling on the level lane; merging by dragging keeps the growing band, by double-click the selected one, otherwise the lower.

**Engineering deviations (Shape):** as Comp's stages, crossover and band slots never swap (crossover slot N owns band N + 1, bands numbered by frequency on screen). The clip stage (footer toggle, ceiling on the level lane) and sustain length (a LENGTH dial and a scope handle; the design left it unplaced) are built; the compact layout and help mode come later.

Fourth pass (Mite Comp, 2026-10-22), still binding, answering `comp-brief.md`:
- the Comp accent moved from azure to **ember** (`#E44A08` dark; your decision, ΔE 15.1 from `gr`, so OUT stays white and ember marks only what you touch);
- a new semantic colour, **`gainUp`** (`#B5C0F8` dark), for upward gain, which rises from the bottom while `gr` hangs from the top;
- the Comp boards (`CompMain` in 16 variants × 4 themes, components, compact) and the "Mite Comp" part of `interaction.md`.

**Design's answers to the brief (canvas note `compAnswers`):** the GR history is the hero (6 s across the left 776 px), with the 384 px transfer square, the stage strip under it, then the IN/GR/OUT meters. Character and Smart are footer dropdowns; the header holds sidechain, stereo/link and detector & output. Quadrants read from the side of the diagonal, the shading (`gr` vs `gainUp`), the node's glyph and the threshold diamond. Range shows as the curve bending back parallel to the diagonal. Unlinked stereo draws two GR traces (solid L/M, dashed R/S) and a split GR meter.

**Engineering deviation (Comp):** stages are *numbered* by threshold, as drawn, but the host parameters are four fixed slots (`s1_*` … `s4_*`) that never swap values: when thresholds cross, only the displayed numbers re-rank, so automation stays on its stage.

Third pass (2026-10-14), still binding:
- **`tokens.json`** is now the board's own token object (`mitefx-tokens/3`), extracted verbatim:
  - one fixed band ramp;
  - semantic colours, including `danger` and `masking`;
  - a 15-plugin accent table, with checked colour distances;
  - dark, light, high-contrast and colour-blind themes.
- **`interaction.md`** holds the gesture, step-size, focus, touch and reduce-motion spec.
- **New canvas boards:** a high-contrast EQ and the solo state.

The "Third pass" list below is kept as history; the design addressed all of it.

| File | What it is |
|---|---|
| `tokens.json` | Machine-readable tokens. `mite-ui::tokens` is generated from this file. Theme colours come from the second pass (see the note in its `source` field). |
| `mockups/eq-main.dc.html` | **The EQ screen in every state.** Props: `theme` = `dark` · `light` · `cb`, and `variant` = `bands` · `empty` · `linear` · `collision` · `grab` · `spectral` · `multi` · `menu` · `presets` · `settings` · `help`. Each EQ artboard on the canvas is this file with one combination of the two. |
| `mockups/eq-compact.dc.html` | Compact layout, 640×400. |
| `mockups/comp-main.dc.html` | **The Comp screen in every state.** Props: `theme` = `dark` · `light` · `cb` · `hc`, and `variant` = `default` · `stages` · `stage-selected` · `create` · `character` · `custom` · `smart` · `sidechain` · `listen` · `stereo` · `lookahead` · `delta` · `empty` · `presets` · `help` · `menu`. Its script holds the curve maths, the glyph paths (`G`) and the help-strip strings. |
| `mockups/comp-components.dc.html` | Comp component sheet: stage nodes and quadrant glyphs, Character and Smart glyphs, GR meter, envelope scope, threshold grip. |
| `mockups/comp-compact.dc.html` | Comp compact layout, 640×400 (built later, with the EQ's). |
| `comp-brief.md` | The brief this pass answers. |
| `shape-brief.md` | The brief pass 5 answers. |
| `mockups/shape-main.dc.html` | **The Shape screen in every state.** Props: `theme` = `dark` · `light` · `cb` · `hc`, and `variant` = `default` · `multiband` · `band-selected` · `split` · `merge` · `solo` · `mute` · `stereo` · `detector` · `lookahead` · `delta` · `clip` · `empty` · `presets` · `help` · `menu`. Its script holds the scope and rail maths and the help-strip strings. |
| `mockups/shape-components.dc.html` | Shape component sheet: crossover grips, band regions, the scope at its sizes, bipolar dials, the signed gain trace, hit markers. |
| `mockups/shape-compact.dc.html` | Shape compact layout, 640×400 (built later, with the EQ's and Comp's). |
| `mockups/shared.dc.html` | Shared components: modulation matrix, 4-node transfer curve, waveform/GR history, spectrogram, and their usage rules. |
| `mockups/components.dc.html` | Component sheet. Still **first pass**: knobs, sliders, fields, meters, node states, buttons, crossover, piano strip. |
| `mockups/tokens.dc.html` | Brand and token sheet. Still **first pass** and stale; `tokens.json` supersedes it. |
| `mockups/canvas.json` | Canvas index: board list, titles, layout. |

The `.dc.html` files only render inside the canvas. Treat them as reference source: the geometry maths in their scripts is meant to be ported.

## Binding design decisions

### Brand and type

- The wordmark is `miteFX` in lower-case Manrope ExtraBold, with "FX" in the plugin's accent.
- The mark is a monoline mite seen from above, carrying the plugin's signal glyph.
- Manrope is the only font (OFL), and every readout uses tabular figures.

### Themes

- **Dark** is the default. **Light** and **Colour-blind** are fully specified in `tokens.json`.
- **High contrast** appears in the settings list but has no palette yet.
- The colour-blind theme draws band numbers on every node and dashes the sidechain trace, so neither depends on hue alone.

### Look

- Near-black blue-grey base.
- Raised controls (an inset highlight plus a two-layer shadow); inset wells for fields and segment tracks.
- Glass panels: 18 px backdrop blur, 22 px for dialogs.
- Glow on the curve, ticks, pointers, meters and latched toggles.

### Band ramp

- The ramp is 7 hues and is independent of the EQ accent (`#63CBFE`).
- In the dark theme every band colour has at least 6.4:1 contrast on the display; in the light theme, at least 4.4:1.

### Semantic colours

| Token | Dark | Meaning |
|---|---|---|
| `gr` | `#FC8731` | Gain reduction |
| `warn` | `#FEDD4D` | Warning |
| `clip` | `#ED3F4B` | Masking, clipping and destructive actions |
| `ok` | `#65E287` | OK |
| `sidechain` | `#A161E0` | Sidechain |

Amber is no longer reused for bands.

### Hit targets

- Every control is 32 px: header, footer, segments, panel buttons, settings, presets.
- Menu items are 30 px.

### Settings

- Settings are **global per user**, shared by every MiteFX plugin and not stored in the project.
- They cover: theme, UI scale (75–200 %), accent swatch, reduce motion, meter ballistics, dial drag (vertical or circular), fine-adjust factor (×0.1), double-click reset, analyzer resolution and tilt, and renderer (GPU or CPU).

### Shared-component rules (from `shared.dc.html`)

- Modulation is drawn in the plugin accent, never in a semantic hue.
- Gain reduction is always `gr` and always hangs from the top or the 0 dB line.
- Transfer-curve nodes reuse the EQ node states.
- History scrolls right to left over a 6 s window; pausing keeps the last 30 s.
- Spectrograms use one sequential ramp from the display colour to near-white (never a rainbow), with detections overlaid in a semantic colour.
- Every chart has a text alternative that reads out its current values.

## What the screens add to the engineering scope

These come from the second pass and are now part of the Mite EQ spec (BLUEPRINT §8):

- **Empty state:** an `Init` preset with no bands. The help strip says "passing audio unchanged", so the plugin must null in this state (it is tested).
- **Spectrum grab:** hovering a peak shows its frequency, note and cents; dragging sets gain, the wheel sets Q, releasing creates a Bell and Esc cancels.
- **Collision panel:**
  - Shows the name of the other instance ("Bass DI") and offers three actions: **Carve**, **Carve on Bass** and **Ignore**.
  - "Carve on Bass" writes to *another* instance, so the instance registry needs a **command channel**, not only spectrum sharing.
- **Resonance band panel:** DEPTH, SHARP, SELECT, ATTACK, RELEASE, a frequency range, and Delta. It uses the same engine as Mite Soothe.
- **Multi-select panel:**
  - Relative controls: GAIN ×% (this is `gain_scale` applied to the selection), FREQ ±semitones and Q ×%.
  - Bulk actions: Bypass all, Make dynamic, Delete.
- **Linear Phase:** a resolution picker in the footer and a latency readout (High ≈ 46 ms). Brickwall slopes are offered only in Linear Phase.
- **Preset browser:**
  - Library categories with counts, tag filters, search, and a curve thumbnail per preset.
  - Hovering a preset auditions it on the live signal. This needs a safe preview path: crossfade into the preset's state without touching undo or the host's parameters.
  - Import and Save current.
- **Help overlay:** numbered call-outs and a "Help mode" pill; Esc or ? exits.
- **Compact (640×400):**
  - Header: preset dropdown, A/B, and a "more" menu holding undo, redo, C/D, morph, settings and help.
  - The band panel collapses to an inline strip (FREQ, GAIN, Q, More).
  - Footer: mode dropdown, latency, Δ, Mix, Out, Bypass.

## How the mockup's CSS/SVG effects map to the Rust renderer

| Mockup effect | `mite-ui` implementation |
|---|---|
| `feGaussianBlur` + merge (curve, tick and meter glow) | WGSL bloom pass: separable Gaussian on an offscreen layer, then additive composite |
| `feDropShadow` in the accent (knob pointer) | The same glow pass on a single-path layer |
| `backdrop-filter: blur() saturate()` (glass, dialogs, menus) | Dual-filter blur of the layers below, cropped to the rounded rect, then a saturation matrix, then the fill |
| Multi-layer `box-shadow` | Vello blurred rounded rect, one call per layer |
| Dashed meter segments, hatch pattern (masking) | Instanced rects and line strips in one path, not gradients or patterns |
| Spectrogram heat cells | GPU texture, 6-step LUT sampled in a fragment shader |

## Third pass: remaining issues

1. **The plugin accent table is broken by the new ramp.** Only four accents were revised: EQ `#63CBFE`, Comp `#6095FA`, Limit `#B9E855`, Delay `#DB76D3`.
   - Three of those four *are* band-ramp colours (bands 1, 2 and 5). Comp has a sidechain EQ and Delay has per-tap EQ, so both plugins draw bands and the original clash comes back.
   - The 11 accents that weren't revised also clash:
     - Verb `#6F8CFF` is close to band 1.
     - Gate `#9BE15D` is close to band 2 and `ok`.
     - Shape `#FF6FB5` is close to band 3.
     - Soothe `#7FE3C0` is close to band 4.
     - Multiband `#B98BFF` is close to band 7.
     - Sat `#FF7A45` is close to `gr`.
   - **Needed:** a full 15-plugin accent table, disjoint from the ramp and the semantic colours, for all three themes.
2. **Band 6 vs the total curve.** Band 6 (`#D4DCE5`, near-white) has only 1.2:1 contrast against the total curve (`#DDF3FF`), so a selected band 6 reads as part of the curve. In the light theme it's 2.3:1.
3. **Band 2 vs `ok`.** Band 2 (`#B9E855`) and `ok` (`#65E287`) have the same lightness (1.15:1).
4. **Colour-blind theme overloads one colour.** Band 3, `ok`, `sidechain` and the main text all use `#E6EAF0`, and `warn` is pure white. The dash on the sidechain trace helps, but `ok` and `warn` have nothing else to tell them apart.
5. **Token naming.** `collision` was renamed `clip` and is also used for "Delete band". Split it into `masking` and `danger` so that a future clip indicator and the masking overlay can differ.
6. **`sem.mod` was dropped.** The new rule says modulation is drawn in the plugin accent, but the component sheet's modulated knob still uses `#C9A7FF`. Confirm the rule and update the sheet.
7. **Stale sheets.** The Tokens and Components artboards are still first pass: they show the old ramp, the old semantic colours, the amber solo ring and 26 px segments. Regenerate both from the new theme objects.
8. **Solo has no new treatment.** The brief asked for something other than an amber ring, but no screen shows solo. Add it to the node-state row and show it on the EQ.
9. **High-contrast theme:** listed in settings, but no palette.
10. **`interaction.md` was not delivered.** Pieces of it are scattered through the help strings, settings and shared rules. Still needed:
    - Knob drag pixels per full range, and wheel step sizes.
    - Band-panel collision rules.
    - Keyboard focus order.
    - What changes under reduce motion.
    - Touch gestures.
11. **Small accessibility fixes in the mockups:**
    - Compact Δ button: add `aria-label="Delta"`.
    - Menu items: make them focusable (`role` on a `div` alone isn't).
    - Help overlay: the callout boxes are `aria-hidden`, so expose the same text to screen readers in help mode.

## Earlier review (first pass), now resolved

- **Band 1 matched the EQ accent:** fixed with a new ramp.
- **Amber was used for both band 2 and warnings:** fixed (warn is now yellow, and no band uses it).
- **Help-strip hint contrast was 3.3:1:** now 5.25:1.
- **Segments were 26 px:** now 32 px.
- **All 12 missing EQ screens and all 4 shared components:** delivered.
