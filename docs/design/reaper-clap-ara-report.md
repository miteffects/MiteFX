# Bug report for Cockos: ARA over CLAP crashes REAPER (Linux)

Ready to post in the REAPER Bug Reports forum (forum.cockos.com). Traced 2026-11-05 (see
pitch-brief.md §1c); written up 2026-11-09.

---

**Title:** [7.82 Linux x64] ARA 2 plug-in over CLAP: REAPER crashes ~2 s after binding (track FX
and take FX); the same plug-in over VST3 works

**Version:** REAPER v7.82 (October 4 2026), linux-x86_64, from the official tarball. Linux
7.3.1 (CachyOS), x86_64.

**Plug-in:** Mite Pitch, an open-source ARA 2 note editor (GPL, https://github.com/ansabic/MiteFX,
`plugins/mite-pitch`). It is one binary built as CLAP and VST3. Over CLAP it offers the ARA
factory (`org.ara-audio.ara.factory/2`) and the plug-in extension under
`com.celemony.ara.plugin_extension.draft/1` (the name REAPER asks for) and
`org.ara-audio.ara.pluginextension/2`. Over VST3 it offers `ARA::IMainFactory` and
`ARA::IPlugInEntryPoint2`.

**What happens:** REAPER binds the CLAP instance to its document controller
(`bind_to_document_controller`), and about two seconds later REAPER crashes in its own code
(SIGSEGV). Nothing of ours is on the stack. Both placements crash:

1. **Track FX.** REAPER makes the instance in context 1 and assigns roles 6 (editor renderer and
   editor view, no playback renderer). The instance is linked to the track's record, but the
   clip's record is never linked to it. The add-region path (0xa68db2 → 0xa6102f in the 7.82
   binary) then dereferences the missing renderer without a check.
2. **Take FX.** Context 2, roles 5. A record in the list at document +0x178 has no link, and
   0xa6784c dereferences it.

The roles appear to come from a global (0x12d51c0: bit 0 adds the editor renderer, bit 1 the
playback renderer, bit 2 removes the view).

**What we ruled out on the plug-in side:**
- We fill all three role interfaces (playback renderer, editor renderer, editor view) whatever
  roles are assigned. An earlier crash at 0xa6102f came from a null interface for an unassigned
  role, and that one is gone.
- Audio readers are created and destroyed only on the model thread.
- `notifyDocumentDataChanged` is called only from inside `notifyModelUpdates`.

The crash stays after all three.

**Expected:** as over VST3. With the same binary as VST3, REAPER binds a visible editor instance
(roles 6) plus a hidden playback instance (roles 1) and links both records. The VST3 version
plays, edits, saves the ARA archive and reopens without trouble.

**To reproduce:**
1. Build: `cargo xtask bundle mite_pitch --release` in the repository. This gives
   `target/bundled/Mite Pitch.clap`. Copy it to `~/.clap`.
2. Start REAPER with `MITE_PITCH_CLAP_ARA=1` in the environment. ARA over CLAP is off by default
   because of this crash.
3. Put an audio item on a track. Add "Mite Pitch (MiteFX)" from the **CLAP** list as a track FX,
   or as a take FX on the item.
4. REAPER crashes within a couple of seconds.

To trace it we used a private config folder (`-cfgfile`), a `__startup.lua` that adds the FX,
and gdb with breakpoints at the addresses above. We can share core dumps or a ready-built
`.clap` on request.
