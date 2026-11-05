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
        // Playback of the take in its stretch of the timeline.
        let (Some(r), Some(pos), true) = (&self.rendered, t.pos, t.playing && !bypass) else {
            self.blend = 0.0;
            return;
        };
        let end = r.start + r.audio.len() as i64;
        if self.blend == 0.0 && (pos + (len as i64) < r.start || pos > end) {
            return;
        }
        for i in 0..len {
            let at = pos + i as i64;
            let inside = at >= r.start && at < end;
            self.blend = if inside {
                (self.blend + self.blend_step).min(1.0)
            } else {
                (self.blend - self.blend_step).max(0.0)
            };
            if self.blend == 0.0 {
                continue;
            }
            let v = if inside {
                r.audio[(at - r.start) as usize]
            } else {
                0.0
            };
            for ch in channels.iter_mut().take(n_ch) {
                ch[i] = ch[i] * (1.0 - self.blend) + v * self.blend;
            }
        }
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
