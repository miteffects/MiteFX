//! Mite Pitch's audio thread: while the transfer is armed and the transport plays, the input is
//! recorded against the timeline; otherwise, inside the take's stretch of the timeline the
//! rendered take plays in place of the input (crossfaded over [`EDGE_MS`] at its ends), and
//! everywhere else the input passes. Never allocates, never locks: the recording buffer and the
//! renders come and go through `Handover`s.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::session::{RecordBuffer, Rendered, Session};

/// The crossfade between the input and the take at the take's ends, ms.
pub const EDGE_MS: f32 = 5.0;

/// The transport as the plugin needs it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transport {
    pub playing: bool,
    /// The timeline sample at the block's start.
    pub pos: Option<i64>,
}

// The handover carries boxes; the buffer inside is shared with the UI (an `Arc`).
#[allow(clippy::redundant_allocation)]
pub struct PitchEngine {
    record: Option<Box<Arc<RecordBuffer>>>,
    recording: bool,
    rendered: Option<Box<Rendered>>,
    /// Retired values the handovers' slots were too full to take (given back next block).
    stale_render: Option<Box<Rendered>>,
    stale_record: Option<Box<Arc<RecordBuffer>>>,
        /// 0: the input, 1: the take.
    blend: f32,
    /// The plugin's own transport: playing, where (take samples), its fade (0–1).
    local: bool,
    local_pos: i64,
    local_gain: f32,
    blend_step: f32,
}

impl PitchEngine {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            record: None,
            recording: false,
            rendered: None,
            stale_render: None,
            stale_record: None,
                        blend: 0.0,
            blend_step: 1.0,
            local: false,
            local_pos: 0,
            local_gain: 0.0,
        };
        e.prepare(sample_rate);
        e
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        self.blend_step = 1.0 / (EDGE_MS * 0.001 * sample_rate).max(1.0);
    }

    pub fn reset(&mut self) {
        self.blend = 0.0;
    }

    fn retire_render(&mut self, s: &Session, r: Box<Rendered>) {
        if let Err(back) = s.renders.retire(r) {
            self.stale_render = Some(back);
        }
    }

    /// `channels`: one or two, processed in place. `bypass`: the input as it is (the take still
    /// records).
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        t: Transport,
        bypass: bool,
        s: &Session,
    ) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels
            .iter()
            .take(n_ch)
            .map(|c| c.len())
            .min()
            .unwrap_or(0);
        // Hand back what could not be retired before.
        if let Some(r) = self.stale_render.take() {
            self.retire_render(s, r);
        }
        if let Some(r) = self.stale_record.take() {
            if let Err(back) = s.records.retire(r) {
                self.stale_record = Some(back);
            }
        }
        if let Some(r) = s.renders.take() {
            if let Some(old) = self.rendered.replace(r) {
                self.retire_render(s, old);
            }
        }
        if let Some(r) = s.records.take() {
            if let Some(old) = self.record.replace(r) {
                if let Err(back) = s.records.retire(old) {
                    self.stale_record = Some(back);
                }
            }
        }
        s.playing.store(t.playing, Ordering::Relaxed);
        if let Some(p) = t.pos {
            s.playhead.store(p, Ordering::Relaxed);
        }
        // The transfer: record while armed and playing.
        let armed = s.armed.load(Ordering::Acquire);
        if armed && t.playing {
            if let (Some(buf), Some(pos)) = (&self.record, t.pos) {
                let mono = (0..len).map(|i| {
                    if n_ch == 2 {
                        0.5 * (channels[0][i] + channels[1][i])
                    } else {
                        channels[0][i]
                    }
                });
                if buf.push(pos, mono) {
                    self.recording = true;
                } else {
                    self.end_recording(s);
                }
            }
            // The input is heard while recording.
            return;
        }
                if self.recording {
            self.end_recording(s);
        }
        // The host is stopped: the plugin's own transport.
        if !t.playing {
            self.blend = 0.0;
            self.play_local(channels, n_ch, len, s);
            return;
        }
        if self.local {
            // The host plays: it wins.
            self.local = false;
            self.local_gain = 0.0;
            s.local_active.store(false, Ordering::Release);
        }
        s.local_play.store(false, Ordering::Release);
        // Playback: the rendered stretches where they sit on the timeline (crossfaded with the
        // input at their ends; bound to ARA, with silence: the clips are all that plays).
        let (Some(r), Some(pos), true) = (&self.rendered, t.pos, !bypass) else {
            self.blend = 0.0;
            return;
        };
        let ara = r.ara;
        let end = pos + len as i64;
        let near = r.segments.iter().any(|g| g.start < end && g.start + g.len as i64 > pos);
        if !near && self.blend == 0.0 {
            if ara {
                for ch in channels.iter_mut().take(n_ch) {
                    ch[..len].fill(0.0);
                }
            }
            return;
        }
        for i in 0..len {
            let at = pos + i as i64;
            let seg = r.segments.iter().find(|g| at >= g.start && at < g.start + g.len as i64);
            self.blend = if seg.is_some() { (self.blend + self.blend_step).min(1.0) } else { (self.blend - self.blend_step).max(0.0) };
            let v = seg.map_or(0.0, |g| g.audio.get(g.offset + (at - g.start) as usize).copied().unwrap_or(0.0));
            for ch in channels.iter_mut().take(n_ch) {
                let dry = if ara { 0.0 } else { ch[i] };
                ch[i] = dry * (1.0 - self.blend) + v * self.blend;
            }
        }
    }

    /// The take (as rendered) from the cue, looping if asked, faded in and out over
    /// [`EDGE_MS`]; the input under it as the fade lets it through.
    fn play_local(&mut self, channels: &mut [&mut [f32]], n_ch: usize, len: usize, s: &Session) {
        let want = s.local_play.load(Ordering::Acquire);
                let Some(take) = self.rendered.as_ref().and_then(|r| r.take.as_ref()) else {
            s.local_play.store(false, Ordering::Release);
            return;
        };
        let n = take.len() as i64;
        if want && !self.local {
            self.local = true;
            self.local_gain = 0.0;
            self.local_pos = s.cue.load(Ordering::Acquire).clamp(0, n);
            s.seek.store(false, Ordering::Release);
        }
        if !self.local {
            return;
        }
        if s.seek.swap(false, Ordering::AcqRel) {
            self.local_pos = s.cue.load(Ordering::Acquire).clamp(0, n);
        }
        let looping = s.loop_on.load(Ordering::Acquire);
        let (ls, le) = (s.loop_start.load(Ordering::Acquire).clamp(0, n), s.loop_end.load(Ordering::Acquire).clamp(0, n));
        let mut want = want;
        for i in 0..len {
            if looping && le > ls && self.local_pos >= le {
                self.local_pos = ls;
            }
            if self.local_pos >= n {
                // The take's end: stop (fading out over silence).
                want = false;
                s.local_play.store(false, Ordering::Release);
            }
            self.local_gain = if want { (self.local_gain + self.blend_step).min(1.0) } else { (self.local_gain - self.blend_step).max(0.0) };
            let v = if self.local_pos >= 0 && self.local_pos < n { take[self.local_pos as usize] } else { 0.0 };
            for ch in channels.iter_mut().take(n_ch) {
                ch[i] = ch[i] * (1.0 - self.local_gain) + v * self.local_gain;
            }
            self.local_pos += 1;
        }
        s.local_pos.store(self.local_pos, Ordering::Release);
        if !want && self.local_gain == 0.0 {
            self.local = false;
        }
        s.local_active.store(self.local, Ordering::Release);
    }

    fn end_recording(&mut self, s: &Session) {
        self.recording = false;
        if let Some(buf) = self.record.take() {
            if let Err(back) = s.records.retire(buf) {
                self.stale_record = Some(back);
            }
        }
        s.finished.store(true, Ordering::Release);
    }
}
