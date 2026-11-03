# Mite Multiband: design and build spec (in-house, 2026-11-03)

**From:** MiteFX engineering, designed here (user decision, 2026-11-03, as Sat, Limit and Gate).
Rival: FabFilter Pro-MB. The suite's bar: every pixel from the DSP, direct manipulation, no words
in the display, no corners, honest lines (`mite_ui::widgets::history`), nothing glowing outside
its plot (`DrawCx::begin_clip`).

**Accent:** mint (tokens.json `multiband`; user, 2026-11-03, "if it's not too similar to eq":
CIEDE2000 to EQ's cyan 21.2 dark, 19.6 high contrast, 13.4 light; the light greens are crowded,
any greener value trades EQ for Gate or Shape at ~13–14).

**Scope (user, 2026-11-03): Pro-MB baseline plus sidechain.** Linear- and dynamic-phase crossovers
later (appended parameters, state 2).

---

## 1. What it is

Up to **6 bands** on the shared LR4 crossover (`mite_dsp::crossover`: bands sum to an allpass;
crossover slot k owns band slot k + 1; band 1 is always the lowest region), each with **Comp's
dynamics** (`mite_dsp::dynamics`: one stage per band, any quadrant: compress, expand, upward
compress, upward expand; threshold on the band's input, ratio, knee, range, attack, release,
Peak/RMS/loudness detector), a band gain, bypass, mute, solo and Delta.

- **Detector:** the band's own signal, or (band switch `external`) the external sidechain split by
  the same crossovers after the **sidechain EQ** (`mite_ui::widgets::key_eq`, six bands with
  slopes; the key's spectrum under it).
- **Lookahead** (global, 0…20 ms) delays the audio of every band alike; reported as latency.
- **Mix** is against the dry signal **through the same crossover** (the allpass sum of the bands),
  so mix 50 % does not comb (BLUEPRINT §4.2). Global Delta: what the dynamics changed. Global
  bypass: the input itself, delayed by the latency.
- **Neutral:** every band inert (stage off) → the allpass sum, delayed; one band (no crossover) →
  the input delayed bit for bit (tested).
- **Bands appearing or merging:** `Split::restructure` carries the filters' states, so the sum stays
  continuous; a new band's dynamics start from rest. *Slice 1 limitation:* with lookahead on, a
  new band's region is silent for the lookahead's length.

## 2. Main view (1280 × 760): Sat's and Shape's frame

Spectrum on top (input dim, output lit, each band's region tinted, and the **dynamic response**:
the curve of what the bands' gains do to the spectrum right now, from the crossover's weights and
each band's live gain), the crossover rail (`mite_ui::widgets::rail`: grips, ghosts, merging,
the band menu), and a card per band: the selected one wide (mode glyphs, a transfer square with
the band's curve from `curve_db` and a live dot, the dials), the others narrow. Footer: the
sidechain source and EQ button, latency, Δ, bypass, MIX, OUTPUT. Meters IN, OUT.

## 3. Parameters (frozen with the first release; `tests/golden/mite_multiband.params.txt`)

- Globals: `lookahead`, `link`, `mix`, `out_gain`, `delta`, `bypass`, `sc_source`, `listen`,
  `k1…k6_{on,type,freq,gain,q,slope}` (Gate's).
- Crossovers `x1…x5_on`, `x1…x5_freq` (defaults 120, 500, 2000, 6000, 12000 Hz).
- Bands `b1…b6_`: `on`, `quad`, `thresh`, `ratio`, `knee`, `range`, `attack`, `release`, `det`,
  `gain`, `bypass`, `mute`, `solo`, `delta`, `external`.

## 4. Slices

1. (done 2026-11-03) `mite_dsp::multiband` (split, per-band dynamics, sum, mix through the allpass, sidechain split)
   with its tests; the plugin with every parameter; the editor: spectrum with the dynamic
   response, rail, cards, footer, meters.
2. The sidechain panel (shared `KeyEq`, the key's spectrum) and per-band external keys.
3. Each band's honest history on its card (the selected band's levels and gain over time).

   Built: every parameter (the sidechain EQ's lines Gate's, tested), the engine (records of each
   band's input peak and gain change, the input and output spectra), the editor (the dynamic
   response from `Layout::weights` and the bands' eased gains; one band's card across the
   display, several: the selected one wide, the others narrow; quadrant buttons turn a band on,
   the lit one again turns it off; the square: across threshold, up ratio, one undo step), the
   header's controls shifted 36 px for the longer title. 120 fps at 2× DPI (the response curve
   first read the whole host per point: 3.95 ms a scene, now 0.4 ms). clap-validator 34/0.
