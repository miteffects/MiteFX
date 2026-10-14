# Brief for the third design pass

The second pass is approved. All screens, themes and shared components stay as drawn. This pass only tidies up the system so it can be generated into code. Details and numbers are in `README.md` under "Third pass".

1. **Plugin accent table.** Deliver all 15 plugins × 3 themes (dark, light, colour-blind).
   - No accent may be within ΔE ≈ 15 of any band-ramp colour or semantic colour.
   - Comp `#6095FA`, Limit `#B9E855` and Delay `#DB76D3` are currently band-ramp colours (bands 1, 2 and 5). Comp and Delay draw EQ bands, so they must change.
2. **Band ramp fixes.**
   - Band 6 `#D4DCE5` has 1.2:1 contrast against the total curve `#DDF3FF`. Replace it with a hue.
   - Band 2 `#B9E855` has the same lightness as `ok` `#65E287`. Separate them.
3. **Colour-blind theme.**
   - Stop reusing `#E6EAF0` for band 3, `ok`, `sidechain` and text.
   - Give `ok` and `warn` a non-colour cue (an icon or pattern).
4. **Semantic token names.**
   - Split `clip` into `masking` (hatch overlay) and `danger` (delete, destructive actions).
   - Confirm that `sem.mod` is gone and that modulation always uses the accent.
5. **Regenerate the Tokens and Components artboards** from the new theme objects: new ramp, new semantic colours, 32 px segments.
6. **Solo state.** Add it to the node-state row and show a soloed band on the EQ (it no longer uses an amber ring).
7. **High-contrast palette.** It appears in settings, so it needs one.
8. **`interaction.md`** (still missing). It must cover:
   - Knob drag: pixels per full range, the fine factor, and wheel steps per parameter type.
   - Band-panel placement and collision rules.
   - Keyboard focus order.
   - Reduce-motion behaviour.
   - Touch gestures.
9. **Mockup accessibility fixes.**
   - Compact Δ button needs `aria-label`.
   - Menu items must be focusable.
   - The help-overlay text must be readable by screen readers.
