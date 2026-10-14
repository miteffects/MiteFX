# Brief for the second design pass

The first pass is approved as the direction: the neutrals, glass, glow, Manrope, the tick-scale knob, the EQ layout and the band panel all stay. This pass fixes the issues below and fills in the missing screens. Update `tokens` on the canvas so `docs/design/tokens.json` can be regenerated from it.

## 1. Colour system fixes

- **Band ramp vs plugin accent.** Band 1 (`#5CC8FF`) is the same colour as the EQ accent, which breaks the rule "bands never use the accent". Either rotate the ramp per plugin so no band hue matches that plugin's accent, or redesign the ramp so it never overlaps any accent. Show the result for EQ, Multiband and Sat.
- **Band 2 vs warning and solo.** Band 2 (`#FFB547`) is also `sem.warn` and the solo ring. Give solo its own treatment, for example a white ring plus a "S" badge with every other band dimmed, and keep amber for warnings only.
- **Band 3 vs Shape.** Band 3 (`#FF6FA8`) is nearly the Shape accent (`#FF6FB5`).
- **Band 4 vs ok.** Band 4 (`#8EE07A`) is the same as `sem.ok`.
- **Gain reduction vs Comp and Sat.** `sem.gr` (`#FF8A3D`) sits between the Comp (`#FF9F43`) and Sat (`#FF7A45`) accents. Gain reduction has to read as gain reduction in exactly those plugins, so move one of them.
- **Limit vs collision.** The Limit accent (`#FF5E6C`) is close to `sem.collision` (`#FF4D6A`).
- **Distinguishable without hue.** Every pair above must also differ in lightness, not only in hue. Show the colour-blind theme next to the default one on the EQ screen.
- **Untokened values.** Fold the values used in the mockup but missing from the token sheet into named tokens, or confirm them as component tokens:
  - Button: `#12161C` / `#222832`
  - Panel control: `#1E232B` / `#2E3540`
  - Knob disc: `#161A21`
  - Segment track: `#0C0F13`
  - Grid: `#151920` / `#1D232C`
  - Total curve: `#DDF3FF`
  - Post-fill gradient stop: `#3D6FD9`

## 2. Contrast

- **Help-strip hint.** The right-hand hint (`#5C6573` on `#0C0E12`) is 3.3:1. It needs at least 4.5:1.
- **Disabled text.** Disabled text (`#4A5260`, 2.35:1) is fine for disabled controls only. Never use it for information.
- **Light theme.** Check every text colour against its real background in the light theme too.

## 3. Hit targets

- Segmented buttons are 26 px tall, while the blueprint asks for 32 px or more at 100 % scale (for later iPad/AUv3 use).
- Either show a 32 px segment variant, or agree on 26 px for desktop with a touch variant, and document it.

## 4. Missing screens (Mite EQ, 1280×760 unless noted)

1. **Empty state:** no bands yet, with the "double-click to add" invitation and the analyzer running.
2. **Linear Phase mode:** resolution picker (Low / Medium / High / Max) and the latency readout in context.
3. **Collision / masking view:** sidechain or other-instance spectrum, conflict regions in `sem.collision`, and a "carve" suggestion on a region.
4. **Spectrum grab:** hover over a peak shows its frequency and note, and dragging creates a band.
5. **Spectral dynamic band and Resonance band:** how each looks on the curve and in the panel.
6. **Multi-select and lasso:** several bands selected, with a group drag and how the band panel handles more than one band.
7. **Right-click context menu** on a node and on empty space.
8. **Preset browser:** tags, search, favourites, audition-on-hover.
9. **Settings sheet:** theme, accent, UI scale, analyzer settings, MIDI learn list.
10. **Help "?" overlay:** the whole UI annotated.
11. **Compact size:** 640×400 (50 % scale or the narrow breakpoint). What collapses and what moves into menus.
12. **Light theme** applied to the full EQ screen.

## 5. Shared components still needed

- **Modulation matrix:** sources, targets and depth; shared by Delay, Sat, Filter and Mod.
- **Comp transfer-curve editor:** Nova's 4-node curve with threshold, ratio and knee handles and a live level dot.
- **Waveform / gain-reduction history view:** for Limit, Comp, Gate and Shape.
- **Spectrogram:** for Meter, DeEss and Soothe.

## 6. `interaction.md`

One written spec covering:
- **Gesture map:** click, double-click, drag, Shift-drag, Alt-drag, wheel, right-click, keyboard, plus touch equivalents.
- **State transitions:** every transition, with duration and easing token.
- **Band panel:** how it chooses between above and below the node, and its collision rules.
- **Knob drag maths:** pixels per full range, the fine-control factor, and wheel step sizes.
- **Focus order:** the keyboard path through the EQ.
- **Reduce motion:** what changes when it is on.
