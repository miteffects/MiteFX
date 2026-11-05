# Mite Pitch: design and build spec (in-house, 2026-11-05)

**From:** MiteFX engineering. User, 2026-11-05: "let's implement our melodyne." Decisions:
monophonic first; per note: pitch, pitch drift and vibrato, timing and length, formant and level;
DAW integration like Melodyne's (ARA 2) wanted, built after a transfer workflow ("transfer first,
ARA after"), with everything but the audio's way in shared.

**Name:** Mite Pitch (not Melodyne, Celemony's name). Accent: a new token, chosen with the
contrast checks.

## 1. How audio gets in

- **Transfer (slices 2–4):** arm TRANSFER and play the section: the plugin records what comes in
  against the host's timeline (sample positions), analyses it into notes on a worker thread, and
  from then on plays the edited audio in that stretch in place of the input, in sync with the
  transport (outside it, the input passes). The recording lives in a file in the user's data
  folder, named by an id saved with the plugin's state (never in the state itself: it is large).
- **ARA 2 (slice 5):** the host hands the clip's audio to the plugin (no recording; edits
  follow the clip). Only the audio's way in changes: the analysis, the notes, the editor and the
  engine stay. Transfer stays for hosts without ARA.

### 1a. ARA plan (2026-11-05, user: "do it but with specific plan")

What the host offers (checked in the user's REAPER binary, Linux): ARA is on, and over CLAP it
asks for `com.celemony.ara.plugin_extension.draft/1` (a pre-release name: the SDK, Celemony
ARA_API 2.3, Apache-2.0, names it `org.ara-audio.ara.pluginextension/2`, with the same two
calls `get_factory` and `bind_to_document_controller`). We offer all three names. REAPER has no
CLAP transport-control: the plugin moves REAPER's transport only through ARA's playback
controller.

| Step | What | Done when |
|---|---|---|
| A1 | `crates/mite-ara`: the SDK headers vendored (with their licence), Rust bindings generated once by bindgen and committed (no libclang needed to build), constants and the CLAP structs by hand | the bindings compile; struct sizes match the C headers (a test against bindgen's layout tests) |
| A2 | The document model, plain Rust (no nih-plug): `ARAFactory`, the document controller (edit cycles, musical context, region sequences, audio sources, audio modifications, playback regions; create, update, destroy), audio read through the host's audio reader on a worker, analysis per audio source, notes and edits per audio modification, archive store/restore (our JSON) | a test host in Rust (host interfaces implemented in the test) drives a whole document: add a source, a modification, a region; edits; store; restore into a fresh controller; same notes and edits |
| A3 | Playback renderer: the regions bound to an instance, rendered from their modification (`render_range`), placed by the region's song time and start in the source; editor renderer and view: selection notifications | the test host renders a region at a song offset and gets the edited audio sample-exact; unedited it nulls with the source |
| A4 | Our nih-plug fork: a `ClapPlugin` hook for the ARA factory and the plugin extension under the three names, the `ara:supported` feature, `org.ara-audio.ara.factory/2` on the entry | clap-validator still passes; the CLAP test host finds the extension and binds |
| A5 | Mite Pitch over ARA: when bound, the session's take comes from the selected region's audio modification; the editor shows that clip at song time; renders go to the playback renderer; transport buttons ask the host (start, stop, position, cycle) | in REAPER: Mite Pitch on a clip as ARA, notes show without transfer, edits play, the project reopens with them |
| A6 | Only if REAPER does not bind over CLAP: ARA over VST3 (`IPlugInEntryPoint`, `IMainFactory` in the fork's VST3 wrapper) | as A5, as VST3 |

A5 needs the user in REAPER (no headless REAPER here); A1–A4 are proved by tests.

Status 2026-11-05: A1–A5 built. `crates/mite-ara` (bindings with compile-time layout checks,
document controller, instances, `testing::TestHost`); the fork's CLAP hooks (`clap_factory`,
`clap_extension`, `clap_extension_context`, nih-plug 190d4dd); Mite Pitch offers the factory and
the extension, binds, analyses each clip's source, renders per modification, plays regions at
their song time (silence elsewhere), writes edits to the document (archived by the host), and
drives the host's transport. Tests: a Rust host plays REAPER's part (clip in, null, edit, archive,
reopen, region moved; no allocation on the audio thread). Not yet seen in REAPER. clap-validator:
32 passed, 2 failed (param-set-*: its fixed-seed random value for the only parameter, Bypass,
rounds to the default; Tuner passes all 34).

### 1b. Time in the plugin (user: "timeframe control so I don't have to control time in
REAPER but in plugin too")

- Click the ruler: the play cursor goes there. Space or the footer's play button: play from the
  cursor, stop. Drag along the ruler: a loop range; L: loop on/off. Double-click a note: play
  from it is not taken (double-click is "onto the snap").
- Without ARA the plugin plays the take itself while the host is stopped (REAPER runs FX when
  stopped), the edited render from the cursor, looping if asked; when the host plays, the host
  wins. With ARA (A5) the same controls drive REAPER's transport.

## 2. The engine (`mite_dsp::melody`), offline, off the audio thread

- **Analysis:** the pitch every 5 ms (the McLeod method of `mite_dsp::pitch`, a 40 ms window for
  voice: 60…1500 Hz), voicing (clarity and level), the level envelope. **Notes:** a voiced stretch
  is cut where the pitch steps (a held change of more than about 70 cents) or the level dips and
  rises again (a new syllable); each note keeps its start and end, its pitch centre (the median of
  its steady middle, cents), its drift (the pitch curve minus the centre: the slides, wobbles and
  vibrato) and its level. Unvoiced sounds (consonants, breaths) sit between notes and belong to
  the note after them for timing.
- **Pitch marks:** in voiced stretches, one mark per period at the waveform's main peak (searched
  around where the pitch says the next period starts), the grid the resynthesis cuts grains on.
- **Resynthesis (TD-PSOLA):** each output period takes a two-period grain (Hann-windowed) from
  the input around the mark nearest the matching input time and overlap-adds it at the new period
  spacing: the pitch moves while the formants (the vocal tract) stay. **Timing and length:** a
  time map from output to input time (a note moved, or stretched, repeats or skips periods; the
  consonants before it are moved, not stretched). **Formant:** each grain resampled before it is
  laid down (shorter grain: higher formants). **Drift and vibrato:** the output pitch is the
  note's new centre plus its drift times an amount (0: flat, 100 %: as sung, 200 %: twice).
  **Level:** a gain per note, crossfaded at its edges. Unvoiced stretches are copied (moved by the
  time map, in short crossfaded grains when stretched).
- Each edit re-renders only the notes it touches (and their neighbours' crossfades), on a worker
  thread; playback reads the rendered audio.

## 3. Main view (slice 3)

The notes as blobs on a pitch grid (time across, pitch up; a blob's thickness its level), the pitch
curve drawn through each (its drift), the scale's notes shaded. Tools: pitch (drag up and down:
snaps to semitones, Alt free; double-click: centre on the nearest note), timing (drag across;
edges: length), drift (drag: less or more of the wobble), formant, level; scale and key with
"snap all to scale"; selection by click, drag box and Shift; undo. A waveform lane under it.

## 4. Slices

1. The engine on its own: analysis, notes, pitch marks, resynthesis with every edit; tests on a
   synthetic sung line (glottal pulses through formants, vibrato, consonants), and renders to
   listen to (`examples/melody_render`).
2. The plugin: transfer (record against the timeline, the file, the state's id), analysis on a
   worker thread, playback of the rendered audio in sync.
3. The editor: the note view, the tools, scale, undo.
4. Polish: quality on real recordings (the user's), CPU and memory, long takes. Done so far:
   the resynthesis fixed (marks by correlation, grains anchored on marks), slow drift and
   vibrato apart, split and join, key, Correct macro, transitions, waveform lane.
5. ARA 2 (see §1).

## 5. Tests

Analysis finds the notes of a sung line (count, starts within 10 ms, centres within 5 cents); a
pitch edit lands within 3 cents and keeps the formants (the spectral envelope within 1 dB);
timing and length edits move and stretch notes to the sample; drift 0 flattens vibrato (pitch
spread under 3 cents), 200 % doubles it; formant shift moves the envelope by its ratio; level
edits within 0.1 dB; an unedited render nulls against the input (within −60 dB); no clicks at
note edges.

### 1c. REAPER and ARA over CLAP (traced 2026-11-05)

REAPER 7 (Linux) binds over CLAP (`com.celemony.ara.plugin_extension.draft/1`) and then crashes
in its own code, about two seconds in, both ways:

- **Track FX:** REAPER makes the instance in context 1 and assigns roles 6 (no playback
  renderer). It links the instance to the track's record, but the clip's record never gets
  linked, and the unchecked add-region path (0xa68db2 → 0xa6102f) dereferences the missing
  renderer.
- **Take FX:** context 2, roles 5; a record in REAPER's list at document +0x178 has no link, and
  0xa6784c dereferences it.

REAPER picks the roles from a context global (0x12d51c0: bit 0 adds the editor renderer, bit 1
the playback renderer, bit 2 removes the view). The fixes on our side (every role's interface
filled, readers made and destroyed on the model thread, notifications only from
notifyModelUpdates) were needed, but they don't stop it. How to trace: a private copy of the
REAPER config folder, `-cfgfile`, a `__startup.lua` that adds the FX, gdb with breakpoints at
the addresses above.

ARA over CLAP is now off unless `MITE_PITCH_CLAP_ARA` is set. Next: A6, ARA over VST3 (REAPER's
mature ARA path), and report the CLAP crash to Cockos.
