# Mite Delay: design and build spec (in-house, 2026-11-04)

**From:** MiteFX engineering, designed here, as Verb, Sat, Limit, Gate and Multiband. Rival:
FabFilter Timeless 3. The suite's bar: every pixel from the DSP, direct manipulation, no words in
the display, no corners, honest lines, nothing glowing outside its plot.

**Accent:** orchid tint (tokens.json `delay`, dark `#FACCFC`, light `#69426B`; already in the
token set and checked by `accents_clear_the_ramp_and_semantics`).

**Scope of the baseline (BLUEPRINT §6.6):** up to four taps with tempo sync, ping-pong, Digital,
Tape and BBD machines, an EQ and diffusion inside the feedback loop, ducking. The modulation
matrix (shared with Sat, Filter and Mod; deferred by the user 2026-11-03), the granular/pitch
smear and per-tap EQ come later; their parameters are appended (state 2 onwards).

The free EQ follows the user's word on Verb (2026-11-04: "fully flexible eq like eq plugin …
dont make default bands … remove default low cut and high cuts"): the loop EQ is Mite EQ's bands
with none on by default.

---

## 1. What it is (`mite_dsp::delay`)

**Four taps, each its own stereo echo line with its own feedback** (not taps on one line: a tap's
repeats never feed another's). A tap has a time (10 ms…5 s; synced: a note of the host's tempo,
1/64…2/1 with dotted and triplet values, resolved by the plugin), the first echo's level
(−48…+6 dB), feedback (0…100 %), pan and ping-pong.

Per tap and side, each sample:

1. **Read** the line at the tap's time less the diffusion's own delay, cubic Lagrange, with the
   machine's modulation added (never closer than 40 samples to the write: the loop runs in blocks
   of 32). Echo `k` of an impulse lands at exactly `k × time` (tested).
2. **The loop EQ** (six of `mite_dsp::eq::BandProcessor`, the same on both sides, a type or
   on/off change faded over 30 ms because every echo hears it), run on the 32-sample block.
3. **The machine:** a highpass and up to two lowpass poles (one-pole each), then saturation
   `tanh(k x) / k` (unity for quiet signals, so the loop's small-signal gain is the EQ's and the
   losses' alone), `k` = the machine's hardness + 6 × DRIVE.
   - **Digital:** no losses, no saturation at DRIVE 0; a new time crossfades to a second read
     head over 40 ms (equal power).
   - **Tape:** 35 Hz highpass, 9 kHz lowpass, hardness 1; wow (smoothed random motion at RATE)
     and flutter (9 × RATE), up to 2.5 ms × MOD; a new time glides like a motor with inertia:
     critically damped (50 ms), its speed held under 0.5 sample per sample (a fifth up, an
     octave down) and its acceleration so the pitch never jumps.
   - **BBD:** 80 Hz highpass, two lowpass poles at 0.2 × 8192 / time (a bucket brigade's clock
     falls as its time grows: 3.3 kHz at 500 ms, 330 Hz at 5 s), hardness 1.6, a triangle chorus
     up to 4 ms × MOD; glides as Tape.
   The machines are weights that glide (20 ms), so switching never clicks; Digital's sine chorus
   reaches 3 ms × MOD.
4. **Diffusion:** four Schroeder allpasses per side (0.7…2.5 ms, 6 ms in all, taken off the
   read), coefficient 0.65 × DIFFUSION: each pass smears the echo a little more.
5. **Safety:** each side held under 2 (soft, transparent under 1): with full feedback through a
   +24 dB loop boost and full drive nothing blows up (tested).
6. **Write** the input (gated by the tap's on, gliding) plus feedback × the echo; ping-pong
   crosses the feedback and feeds the input summed to mono into the left line only.
7. **Out:** the echo × level, placed by pan (the mid panned at constant power, unity at the
   centre, the side kept).

The taps' sum then gets **width** (M/S, 0…150 %) and **ducking** (as Verb's: from THRESHOLD up 1
dB per dB, at most DEPTH, letting go over RELEASE, through a 3 ms one-pole). A tap turned off
fades out, then stops running and forgets its echoes.

**Output** (the plugin): mix against the dry input (no latency: mix 0 % and bypass give the input
bit for bit), output gain, bypass (an ordinary parameter, never the host's).

**Control rate:** every glide's target once per 32 samples, counted across blocks; every gain
lerped per sample within a step. Output identical for any block size. The modulation's depth
glides over 300 ms (a fast depth change is a pitch blip).

**What the editor draws** comes from the same code: `loop_gain(s, tap, f)` (the loop EQ's analog
response and the machine's filters at the tap's time) and `echoes(s, tap, f)` (each echo's level
per side: level × loop^k × feedback^(k−1), pan and ping-pong in it). Measured echo 1 matches
`loop_gain` within 0.5 dB at 100 Hz…8 kHz in every machine.

**CPU** (48 kHz, `examples/delay_cpu`): 0.4 % of a core with the default tap; 2.2 % with four
taps, six loop bands, modulation, diffusion and drive, in any machine.

## 2. Main view (1280 × 760)

- **The echo plot** (top, full width but the meters, 300 px): time across from the input (a line
  at the left) over 0.25…16 s (the smallest span that holds two echoes of the longest tap; held
  still while dragging), each tap's echoes as stems from the middle line, the left side up and
  the right side down, at their level (−48 dB at the middle, +6 at the edges). The selected tap
  lit (its first stem glowing, a dashed line over its echoes' tops), the others dim. While a tap
  is synced the beats are drawn (bars brighter), and its note sits over its grip.
- **Grips:** a tap's first echo (numbered): across its time (to notes while synced), up and down
  its level; its second echo (small, hollow): the feedback, so that echo is at the pointer. Wheel:
  pan (tap), feedback (second echo). Double-click: a new tap there; on a tap, it is removed.
  Right-click a tap: sync or free (nothing moves), ping-pong or straight, remove.
- **The loop EQ** (bottom left): 20 Hz…20 kHz, ±24 dB, over the input's spectrum (dim, filled) and
  the echoes' (a lit line); the bands' sum lit; the machine's own losses dashed. Bands as Verb's
  tone EQ.
- **Dials** (bottom right), grouped: MACHINE (three pills DIGITAL / TAPE / BBD, DRIVE), MOD
  (DEPTH, RATE), DUCK (DEPTH, THRESH, RELEASE), SPACE (DIFFUSE, WIDTH), and TAP n (the tap last
  grabbed: LEVEL, FEEDBACK, PAN dials; SYNC and PING-PONG pills, the sync pill showing its note
  or its ms).
- **Meters** IN and OUT. **Footer:** Bypass, latency, MIX, OUTPUT.

Measured: 120 fps in the preview with music running (scene 0.29 ms, render 0.62 ms median); a
drag event under 0.01 ms, a draw under 0.6 ms with four taps and four loop bands
(`examples/drag_bench`).

## 3. Parameters (frozen with the first release; `tests/golden/mite_delay.params.txt`)

`mode` (Digital, Tape, BBD), `drive`, `mod_depth`, `mod_rate`, `diffusion`, `width`, `duck`,
`duck_thresh`, `duck_release`, `mix`, `out_gain`, `bypass`; per tap `t1…t4_{on, sync, note, time,
level, feedback, pan, pingpong}` (t1 on by default, every tap synced at a quarter note); the loop
EQ `e1…e6_{on, type, freq, gain, q, slope}` (Mite EQ's types and slopes, none on). Bypass is
monitoring: never in presets or snapshots. State 1.

## 4. Tests (theory, not taste)

`crates/mite-dsp/tests/delay.rs`:
- Echoes land on time at their level: echo `k` at `k × time` (whole and fractional), level ×
  feedback^(k−1), nothing between them.
- The loop gain is what the editor draws: echo 1 against `loop_gain` within 0.5 dB, every machine,
  with a bell and a low cut in the loop.
- `echoes` follows the loop gain (each echo the feedback × the loop under the one before; pan).
- Ping-pong alternates sides (first echo left only, then right only…), and `echoes` says so.
- Nothing blows up: full feedback, +24 dB in the loop, full drive, diffusion and modulation,
  every machine, ping-pong or not.
- Ducking turns the echoes down by DEPTH while the input is over the threshold.
- Output identical for every block size (1, 7, 64, 333, 512).
- Every setting moved fast is click-free (time in Digital and in Tape, level, feedback, pan,
  ping-pong, a tap on and off, the machine, drive, diffusion, modulation, a loop band, width).
- Tape glides to a new time; Digital crossfades (the first head holds the old time mid-fade).

The plugin: mix 0 % and bypass null bit for bit, latency 0; the manifest and golden state v1;
no allocation in `process`; every factory preset names only real sound parameters; the editor
(below).

## 5. Slices

1. (done 2026-11-04, user: "let's hit delay") `mite_dsp::delay` with the tests above; the plugin
   with every parameter and the host's tempo; the editor (echo plot with tap and feedback grips,
   the loop EQ over the spectra, the dial groups with the machine and tap pills, meters, footer);
   seven presets (Init, Dotted Eighth, Tape Echo, Wide Ping-Pong, BBD Slapback, Ducked Vocal,
   Rhythm Taps); golden manifest and state v1. Editor tests: one tap by default, a synced tap
   snapping to notes and its level, a free tap in ms, the second echo setting the feedback,
   taps made (four at most) and removed, the tap menu (free and back without moving, ping-pong,
   remove), the pills, the wheel's pan, loop bands, the groups, selecting a tap. Not yet heard by
   the user.
2. (done 2026-11-05) The modulation matrix (two LFOs and an envelope follower; MIDI and XY sources
   later), shared with Sat and Mod: targets the machine's dials, each tap's time (a synced note's
   too), level, feedback and pan, and the loop bands' frequency, gain and Q. State 2.
3. The granular/pitch smear stage; per-tap EQ on the spectrum display.
