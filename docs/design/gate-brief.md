# Mite Gate: design and build spec (in-house, 2026-11-03)

**From:** MiteFX engineering, designed here (user decision, 2026-11-03, as Sat and Limit). This
file is the design and the build spec; `interaction.md` "Mite Gate" holds the gestures once built.

Rival: FabFilter Pro-G. The suite's bar: every pixel from the DSP, direct manipulation, no words
in the display, no corners, and honest lines: where the drawn signal crosses the threshold, the
gate acts (user, 2026-11-03; `mite_ui::widgets::history`).

Scope (user, 2026-11-03): **Pro-G parity baseline**: gate, expander and ducker; hysteresis, hold
and lookahead; an external sidechain with the sidechain EQ shared with Comp; MIDI trigger output.
The transient trigger (Shape's detector) and the spectral denoise gate come later; their
parameters are not declared yet (they will be appended, state 2).

---

## 1. What it is

### 1.1 The detector

The key is the input (or the external sidechain), through up to three EQ bands (Comp's key
bands: `mite_dsp::eq::design`), linked across the channels (0…100 %), measured as **Peak**
(instant attack, 10 ms release, so zero crossings are not silence) or **RMS** (10 ms). Lookahead
delays the audio, not the key, so the gate opens before a transient.

### 1.2 The three modes

With `L` the key's level (dB), `T` the threshold, `H` the hysteresis, `R` the range (≤ 0 dB):

- **Gate.** Two thresholds: it opens when `L ≥ T` and closes when `L < T − H`; between them it
  keeps its state (no chatter on a level hovering at the threshold). Open: 0 dB; closed: `R`.
  Hold keeps it open that long after the key last reached `T − H`.
- **Expander.** No state: below `T` the gain is `(L − T)·(ratio − 1)`, capped at `R`, with a soft
  knee as wide as `H` (centred on `T − H/2`, so it ends at `T` and the gain above `T` is exactly
  0 dB). Hold delays the release.
- **Ducker.** The gate inverted: it ducks (to `R`) when `L ≥ T`, and lets go when `L < T − H`
  (after the hold). Usually keyed from the sidechain.

### 1.3 The motion

The target gain moves through **two one-pole stages in series** (each at half the time), so
every opening and closing is S-shaped, never a corner: attack towards open (Ducker: towards the
duck), release the other way. The gain the stages produce is what the history draws.

### 1.4 Output

Mix (against the delayed dry signal), Delta (what the gate takes away), Listen (the key as the
detector hears it), output gain, bypass (latency kept; an ordinary parameter, never the host's
bypass: REAPER stops processing a host-bypassed plugin). Neutral settings null: range 0 dB, or mix
0 %, or bypass give the input delayed by the latency, bit for bit.

### 1.5 MIDI trigger (slice 3)

A note on each opening and a note-off on each closing (Gate and Ducker; the expander when its
gain crosses half the range), sample-accurate, on the chosen note and channel; velocity fixed or
from the key's level at the opening.

---

## 2. Colour

Accent `gate` = **pale green** (tokens.json). The gate's gain is drawn in the `gr` role, as Comp's;
the hysteresis band between the two thresholds is shaded in the accent.

---

## 3. Main view (1280 × 760): Comp's frame (user, 2026-11-03)

```
┌ header ──────────────────────────────────────────────────────────────────────┐
│ history (x 0–776): input peaks, output lit, gain from the top,  │ transfer   │IN│
│ the threshold line + grip, the close threshold (T − H) dashed   │ square     │GR│
│ + its grip, the detector dashed when it is not the input's peak │ (384)      │OUT│
│                                                                  │ mode, dials│  │
├ help ─────────────────────────────────────────────────────────────────────────┤
└ footer: sidechain source · listen · latency · Δ · ⏻ · MIX · OUTPUT ──────────┘
```

- **History** (shared): honest peaks; the threshold grip (drag: threshold), the close threshold
  dashed with its own grip (drag: hysteresis); the detector's level dashed when it is RMS or the
  key is filtered or external.
- **Transfer square** (`L` in → out, −84…0 dB): the mode's static curve from the DSP's own
  function; for Gate and Ducker both branches of the hysteresis loop, the band between them
  shaded; a live dot at the key's level. Drag the threshold node along the diagonal: threshold;
  drag the floor up or down: range.
- **Strip** under the square: the mode glyphs (each a drawing of its curve), the detector
  (Peak/RMS), and the dials: RANGE, RATIO (Expander), HYSTERESIS, ATTACK, HOLD, RELEASE,
  LOOKAHEAD, LINK.
- **Sidechain panel** (slice 2): Comp's, moved to `mite_ui` and shared.

---

## 4. Parameters (frozen with the first release; `tests/golden/mite_gate.params.txt`)

- `mode` (Gate, Expander, Ducker), `threshold` (−80…0 dB, −40), `range` (−80…0 dB, −80),
  `ratio` (1…10:1, 4; Expander), `hysteresis` (0…24 dB, 4), `attack` (0.01…100 ms, 0.5), `hold`
  (0…1000 ms, 30), `release` (1…2500 ms, 100), `lookahead` (0…10 ms, 0; not smoothed),
  `detector` (Peak, RMS; Peak), `link` (0…100 %, 100).
- Sidechain: `sc_source` (Internal, External), `listen`, `k1…k3_on/type/freq/gain/q` (Comp's).
- Output: `mix` (100 %), `out_gain` (0 dB), `delta`, `bypass`.
- MIDI: `midi_on` (off), `midi_note` (0…127, 36), `midi_channel` (1…16), `midi_velocity`
  (Fixed, Level), `midi_vel_fixed` (1…127, 100).

---

## 5. Slices

1. (done 2026-11-03) The gate: `mite_dsp::gate` (detector, modes, hysteresis, hold, lookahead, the two-stage
   motion), the plugin with every parameter, external sidechain routing and the key EQ in the
   engine; the editor: history with the threshold and hysteresis grips, the transfer square,
   strip, meters, footer.
2. (done 2026-11-03) The sidechain EQ shared with Comp: `mite_ui::widgets::key_eq` (`KeyEq`:
   the well, the summed response from `analog_response_db`, a node per band; drag a node for its
   frequency and gain, turning it on, as one undo step; double-click switches the band). Comp
   draws through it (its panel equal within GPU noise). Past ±16 dB the curve now bends smoothly
   (tanh) towards a level just beyond the well's edge: the old hard clamp at ±20 dB drew a flat
   floor with a corner under a low cut. In Gate the EQ button beside the sidechain source opens
   the panel over the history's bottom, short of the grips' column; Esc closes it.
3. MIDI trigger output.

   Built: every parameter (state 1, the key bands Comp's ids and ranges, tested), the engine
   (mix, Delta, bypass against the delayed dry; a dip on a lookahead change), the editor as
   above; 120 fps at 1× and 2× DPI. The sidechain EQ's bands work in the engine and in presets;
   their panel comes in slice 2.

**Round 2 (user, 2026-11-03):** "green line in the background blurred … not acceptable"; "in
sidechain i cant see … where the signal is mostly dense"; "make it flexible so that user can
make custom eq … all shapes"; "buttons are too small".
- The haze was the glow layer: it is composited over the whole scene, so the threshold line's
  glow (and the waveform's) shone through the panel. `DrawCx::begin_glow_hole` masks the glow
  under a panel; Gate's EQ panel is opaque now, and Comp's panels get the same mask.
- The key's spectrum (before the EQ, mono; `GateShared::key_spectrum`, `key_eq::KeySpectrum`)
  under the EQ curve, in the sidechain colour.
- The well is an editor (`KeyEq::event`): click an empty spot to make a band (the first that is
  off, a bell); drag; wheel: Q, or a cut's slope; right-click: every EQ type with its icon, or
  Remove; double-click: on/off. Six bands with slopes (state 2: `k1…k3_slope`, `k4…k6_*`,
  appended; a version 1 state keeps its 24 dB/oct and sounds as before). The type and slope
  icons moved from Mite EQ to `mite_ui::widgets::eq_icons`.
- The panel is taller (310 px); the square 336 px, the mode and detector buttons 52 × 44, the
  dials 52 px.
