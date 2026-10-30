# Brief: Mite Shape (design pass 5)

**For:** Claude Design, on the MiteFX canvas (https://claude.ai/artifact/W6i7s8rA3Q81U43ruRusH5).
**From:** MiteFX engineering, 2026-10-25.
**Goal:** every screen and component Mite Shape needs, **single-band and multiband**, in the system you already built for Mite EQ and Mite Comp, so that engineering can build it without guessing.

This brief is self-contained. Where it says "as in the EQ" or "as in Comp", the shipped plugins and the `EqMain` / `CompMain` boards are the reference, *with the corrections in §2*.

---

## 1. What Mite Shape is

MiteFX is an open-source (GPL) suite of pro audio plugins (CLAP, VST3, later AU), written in Rust and drawn on the GPU (Vello vector paths, plus real WGSL bloom and frosted-glass blur passes). Mite EQ and Mite Comp are built. Mite Shape is the transient designer.

**Rivals:** SPL Transient Designer, Kilohearts Transient Shaper, and the transient modes in Pro-MB and Pro-Q 4. FabFilter has no dedicated one, but the bar is still to *look and feel better than FabFilter*.

**What it does, in one paragraph:** Mite Shape changes a sound's envelope, not its level. It turns up or down the **attack** (the first few milliseconds of every hit) and the **sustain** (what rings on after it), with **no threshold**. A ghost note is shaped exactly like a rimshot, because the detector compares the signal's envelope with itself, never with a fixed level. **Multiband** splits the signal into up to **5 bands** at up to 4 crossovers. Each band has its own attack, sustain and timing, so you can sharpen a kick's click without touching its boom, or shorten a snare's ring without dulling its crack. The bands sum back flat: with every amount at zero the plugin nulls.

### 1.1 How the detector works (so the visuals stay honest)

This is the detector the EQ's transient bands already ship with (`mite_dsp::eq::ShapeDetector`). Mite Shape reuses it, and the EQ's transient scope is drawn from it.

- A **hit** starts when a fast envelope pulls 3 dB ahead of a slow one.
- **Attack** follows the fast envelope's lead over a slow-rising one, so it exists only during an onset: a bigger rise gets more of the amount, and it fades as the slow envelope catches up. The **transient time** (5–200 ms, default 25 ms) sets that speed: by then attack is down to about a twentieth.
- **Sustain** follows a slow-falling envelope's lead over the fast one, so it exists only during a decay and grows as the sound dies away, reaching the amount about 6–10 dB down. The **transition** (0–200 % of the transient time) sets how gradually it comes in.
- **A steady signal gets no gain at all**, however high the amounts, held notes included. The fast envelope holds its peak for 20 ms, so a held bass tone isn't distorted.
- The gain is `attack·w + sustain·v·(1 − w)` with w and v in 0…1, so it never goes past either amount. (Revised 2026-10-30: this replaced an attack phase and a flat sustain until the next hit, which behaved like a gate, not a transient designer.)
- Below −80 dBFS the detector fades out, because silence has no shape.

The consequence for design: the gain over time is **a step-like S curve per hit**, computable for a synthetic hit at any setting. Engineering can give you that function, and the EQ already draws it.

---

## 2. The design system as built (binding)

Use the tokens, components and rules already on the canvas (`mitefx-tokens/3`, plus pass 4's `gainUp`). Everything in **comp-brief.md §2.1 and §2.2** still holds: the user's taste rules and the shared chrome. The short form:

1. **Direct manipulation over switches.** Make things by pulling. No "+ Add band" buttons and no "Bands: 1 2 3 4 5" selector if a gesture can say it.
2. **Visual over numeric. No explanatory words in controls.** Dials carry a short name and their value. The help strip says everything else.
3. **Every curve is smooth:** envelopes, crossovers, gain traces, crossfades. S-shaped transitions, never linear ramps or corners.
4. **Honest visuals.** Every graph is computed from the real DSP. A control never shows more than it does.
5. **Dials have feel:** custom travel curves, no snapping while dragging.
6. **Menus:** the current choice is lit in the owner's colour, never ticked. Options have icons.
7. **No crowding.** Controls never overlap, and there is room around the close, bin and power buttons.
8. **Quality before features.** 120 fps at the default size. Don't hide cost by shrinking the design.

### 2.1 What the shipped Comp changed from its boards (apply here too)

- **No words in the display.** The Δ and Listen pills became glowing icons. The empty-state hint moved to the help strip.
- **Host parameters are fixed slots that never swap.** Stages are numbered by threshold on screen, but the slots stay put, so automation stays on its stage. Shape does the same with bands: four crossover slots and five band slots. Bands are numbered low to high on screen.
- **Hovering a menu choice auditions it** through a preview path, never by writing the host parameter.
- **The history scrolls with the clock between host blocks.** It records one entry per 5 ms (`mite_core::HistoryTap`), so it is smooth even with 20 ms host blocks.

### 2.2 Shared chrome

As in Comp:

- **Window:** 1280 × 760 at 100 %, resizable with the aspect kept. The compact layout is 640 × 400; draw it, but engineering builds it later, together with the EQ's and Comp's.
- **Header:** mark and "Mite Shape", ‹ preset ›, undo and redo, then A B C D. Shape-specific buttons go after the letters.
- **Display**, then the help strip (28 px), then the footer: mode controls on the left, the latency readout, auto gain, Δ and bypass, then MIX, PAN and OUTPUT.
- Glass floating panel docked at the bottom of the display, 32 px icon buttons, real blur and bloom.

### 2.3 Colours

- **Accent:** `shape` = **blush**: dark `#FEBCCF`, light `#763F51`, hc `#FEBCCF`, cb `#238EA9`. It is already in the accent table.
- Shaping gain has a sign, like Comp's: **boost rises in `gainUp`, cut hangs in `gr`**. Check that this reads well next to blush. If attack and sustain need telling apart in the history (§4.1), propose how, without a new semantic colour if possible.
- **Band colours in multiband: decide.** The EQ's ramp means *stereo placement* (Stereo, L, R, M, S), not band number, and Shape's bands are frequency regions. Options: all bands in the accent, with their number on them; a lightness step of the accent per band; or the ramp by index, which would break the EQ's meaning. Say which, and why.

### 2.4 Components you already drew that Shape reuses

- **Waveform/GR history** (`shared.dc.html`, as shipped in Comp): IN dim, OUT lit, gain from the 0 line.
- The EQ's **transient scope**: a synthetic hit run through the band's real detector, on a square-root time axis. The attack part and the tail can be dragged, a handover line marks the transient time, and the transition is shaded.
- The EQ's **spectrum** (analyzer: 1/6-octave smoothing, eased rise and fall) and the **multiband crossover bar** from the component inventory (BLUEPRINT §7.3; `interaction.md` already names "Multiband crossovers" in its gesture table, but no board draws the bar yet).
- Tick-scale dial, IN/OUT meters, dropdowns, the docked band panel, and the footer of the EQ and Comp.

Mite Multiband (Pro-MB's rival) comes later and will reuse whatever crossover design you make here. **Design the crossover once, for both.**

---

## 3. What the user can control

The engineering plan. **MVP** is what the first build ships; *later* is designed now so that the layout has room for it, but built afterwards. Ranges are for your scales and readouts.

### 3.1 Per band (5 band slots; band 1 is always there)

| Control | Range | Default | When |
|---|---|---|---|
| **Attack** | −18 … +18 dB | 0 dB | MVP |
| **Sustain** | −18 … +18 dB | 0 dB | MVP |
| **Transient time** (how long attack lasts) | 5 … 200 ms | 25 ms | MVP |
| Transition (how gradually sustain comes in) | 0 … 200 % of the transient time | 100 % | MVP |
| Solo, Mute | toggles | off | MVP (multiband only) |
| Band mix (its own dry/wet) | 0 … 100 % | 100 % | MVP |
| Band output (its fader in the sum) | −24 … +24 dB | 0 dB | MVP |
| Sustain length (how long sustain acts before the gain returns to 0; ∞ = as long as the decay) | 10 ms … ∞ | ∞ | *later* (roster "beyond") |

### 3.2 Bands and crossovers

| Control | Range | Default | When |
|---|---|---|---|
| **Crossover 1–4** | 20 Hz … 20 kHz, each one on or off | off; when made: 120 Hz, 500 Hz, 2 kHz, 8 kHz | MVP |
| Crossover type | LR4 minimum phase (allpass-compensated, sums flat) | LR4 | MVP; *later* linear phase and dynamic phase (shared with Multiband) |

The band count follows from how many crossovers are on (1 to 5 bands). Single-band Shape is simply Shape with no crossovers. **It is the same screen**, not a mode.

### 3.3 Global

| Control | Range | Default | When |
|---|---|---|---|
| Detector filter (single band only: what the detector hears) | high-pass 20 Hz … 2 kHz, low-pass 200 Hz … 20 kHz | open | MVP |
| Listen (hear what the detector hears) | toggle | off | MVP |
| Lookahead (keeps a boosted attack's leading edge) | 0 … 5 ms | 0 | MVP; adds latency (footer) |
| Stereo | L/R · Mid/Side, with **link** 0 … 100 % | L/R, 100 % | MVP |
| Floor (below this a tail is never lifted out of the noise) | −80 … −20 dBFS | −60 | MVP |
| Clip (a soft-clip / soft-limit stage after shaping, so boosted attacks don't overload) | off · on, ceiling −12 … 0 dB, softness | off | *later* (roster "beyond") |
| Mix, Pan, Output, Auto gain, Delta, Bypass | as the Comp footer | 100 %, 0, 0 dB, off | MVP |

### 3.4 What the plugin shows (telemetry, live at the display rate)

Input and output levels (peak and RMS). Per band: the applied gain (signed, per channel), the attack weight w (0…1), and whether a hit is under way. A history record every 5 ms (input, output and gain, per band). The spectrum of the input. The crossover frequencies. The latency.

---

## 4. Screens and interaction to design

### 4.1 Main view

The display must answer three questions at a glance: *where are the hits, what is Shape doing to each one, and (multiband) which part of the spectrum is being shaped.* A proposal to react to (improve or replace it, and say why):

- **History** as the hero: the input as a dim envelope and the output lit on top, scrolling right to left over 6 s. The applied gain is drawn from the 0 line: boosts up in `gainUp`, cuts down in `gr`. A boosted attack is then a bright spike at each hit, and a cut sustain is a shelf hanging after it. Unlike Comp, the gain here *is* the effect, so it may deserve more weight than Comp's GR. Consider marking each detected hit, and how much of the trace is attack versus sustain, so the user sees what the detector decided.
- **Spectrum with the crossover bar** (multiband): a strip, or a layer, showing the input spectrum split into the bands. Each band shows its live shaping, for example its region glowing or breathing with its gain. Crossovers are dragged sideways directly on it.
- **Transient scope per band:** the EQ's scope, the editing surface for attack, sustain, transient time and transition. Drag the attack part up or down for attack, the tail for sustain, the handover line sideways for transient time; Alt-drag the shaded crossfade for transition. **Where does it live?** In the docked band panel, as in the EQ? Or permanently on screen (it is the plugin's core control), one per band side by side, under each band's region of the spectrum?
- **Meters:** IN and OUT on the right edge, as in Comp, with no numbers.

With one band, the spectrum strip may be pointless: then the scope and the history are the whole UI. Show how the screen grows when the first crossover is made.

### 4.2 Making and removing bands (direct manipulation)

There is no band-count control. Proposed rule:

- **Making a band:** double-click the spectrum strip, or pull a ghost crossover in from either edge (ghost grips on hover, as the EQ's ghost dynamics grips). This makes a crossover there, and the band it split keeps its settings on both sides. A sixth band is refused (as Comp refuses a fifth stage), and the help strip says why.
- **Removing a band:** drag a crossover onto a neighbour or off the end, or double-click it. The two bands merge; which settings survive?
- **Crossing:** crossovers can't pass each other. They are pushed apart, with a minimum gap (say ⅓ octave), and the help strip shows the frequency.
- Selecting a band (click its region) opens its panel or focuses its scope. Solo and mute live on the band's region or its strip, as icon buttons. Solo and mute must be impossible to miss, as Listen is in Comp.

Design the affordances: how does the user discover that the spectrum can be split? How are bands told apart (§2.3)? What does a muted band look like in the history and in the spectrum?

### 4.3 Band panel (or band strip)

ATTACK, SUSTAIN, TIME and TRANSITION dials, plus MIX and OUTPUT for the band, solo, mute, and remove (bin, multiband only). The scope sits beside the dials, as in the EQ's transient row. Attack and sustain are bipolar dials (centre 0). The value moves continuously, and the dials have a custom feel: ±0–6 dB gets the most travel.

### 4.4 Detector and stereo

The detector filter, Listen, floor and lookahead go in a sheet or popover, as Comp's "Detector & output". Stereo works as in Comp: L/R or Mid/Side with continuous link. Below 100 % link the two channels shape differently, so draw two gain traces (solid L/M, dashed R/S), as Comp does. In multiband the crossovers do the detector filter's job, so the filter is dimmed or hidden there. Say which.

### 4.5 Clip stage (later, but draw it)

An output clipper after shaping, with a ceiling and softness, so big attack boosts don't clip the bus. Where does it live (footer, header, or the history's top edge as a draggable ceiling line)? How does the history show what was clipped?

### 4.6 Footer and header

Apply §2.2. Footer left: the crossover type (one choice for the MVP, so maybe nothing yet) and the latency readout, when lookahead adds latency. Delta: the output is what Shape added or removed, so for a pure attack boost you hear only the clicks.

---

## 5. Deliverables

Into the canvas, then exported to `docs/design/` as before:

1. **`shape-main.dc.html`**, with props `theme` = `dark` · `light` · `cb` · `hc` and `variant` =
   - `default`: one band, a drum loop running, attack +6, sustain −4; the history spiking at hits and dipping after them,
   - `multiband`: 3 bands (kick, snare body, cymbals), each with different settings, so the spectrum breathes per band,
   - `band-selected`: the panel or scope focused on band 2,
   - `split`: making a crossover (ghost grip, the band forming),
   - `merge`: dragging a crossover onto its neighbour,
   - `solo` and `mute`: one band soloed; another muted,
   - `stereo`: Mid/Side at 50 % link, with two traces,
   - `detector`: the detector sheet open, with Listen on,
   - `lookahead`: latency shown,
   - `delta`: Delta on,
   - `clip`: later, the clip stage on and clipping,
   - `empty`: Init, all amounts 0. The help strip says it passes audio unchanged, and the plugin nulls there,
   - `presets`, `help` (numbered call-outs), `menu`.
2. **`shape-compact.dc.html`** at 640 × 400.
3. **Component additions:**
   - the crossover bar (also for Mite Multiband): its grip states, ghost grip, solo and mute,
   - the band region states (idle, hover, selected, soloed, muted),
   - the per-band transient scope at its Shape size,
   - the bipolar ATTACK and SUSTAIN dials,
   - the signed gain trace in the history,
   - the hit marker, if you use one.
4. **Tokens:** the band colouring (§2.3), anything new, contrast-checked as before (ΔE ≥ 15 from the ramp and the semantic colours).
5. **`interaction.md` additions:**
   - a **"Mite Shape"** part with the crossover gestures (filling the Multiband column of the main gesture table), the scope gestures, making and removing bands, solo and mute,
   - keyboard focus order,
   - touch,
   - reduce motion: does the per-band breathing on the spectrum stop?

## 6. Technical constraints (unchanged)

- Everything is vector paths, gradients, and blur or glow passes. Raster only for subtle noise.
- Manrope only, with tabular figures.
- Animations are property tweens with the motion tokens.
- Hit targets 32 px; menu rows 30 px; nothing hover-only.
- Everything must be computable at 120 fps from per-block telemetry. The history is a level envelope, not a raw waveform. The scope is a synthetic hit through the real detector (cheap: a few hundred samples per frame per band).

## 7. Questions to answer in your notes

1. What is the hero: the history (*what happened to the hits*), the scope (*what the shaping is*), or the spectrum (*where*, in multiband)? And what is the hero at 640 × 400?
2. One scope in a docked panel, or one scope per band always visible?
3. How do bands get their colour, without breaking the EQ's "colour = stereo placement" rule?
4. How does the history tell attack gain from sustain gain, and mark the hits?
5. How does the single-band screen become the multiband one, without a mode switch?
