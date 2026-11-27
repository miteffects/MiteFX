//! Mite Cut's audio thread: bound to ARA as a playback renderer, each of its regions plays its
//! clip through the clip's cut ([`mite_dsp::hits::Curve`]) where the region sits on the song,
//! silence elsewhere; an editor-only instance passes its input. A new cut (other settings, an
//! edit) crossfades in over [`XFADE_MS`]; Bypass and Listen Removed glide over the same.
//! Never allocates, never locks: renders come and go through a `Handover`.

use std::sync::Arc;

use mite_dsp::hits::Curve;
use mite_dsp::resample::SincTable;

use crate::session::Session;

/// A new render, Bypass and Listen Removed glide in over this, ms.
pub const XFADE_MS: f32 = 10.0;

const STALE: usize = 4;

/// The transport as the plugin needs it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transport {
    pub playing: bool,
    /// The timeline sample at the block's start.
    pub pos: Option<i64>,
}

/// What the audio thread plays.
#[derive(Default)]
pub struct Rendered {
    pub segments: Vec<Segment>,
    /// A playback renderer: the regions replace the input (silence elsewhere).
    pub ara: bool,
}

/// A region on the host's timeline: from timeline sample `start`, `len` timeline samples,
/// reading the clip `audio` from its sample `offset`, `rate` of its samples per timeline sample
/// (through `sinc` when that is not 1), at the gain `curve` gives each clip sample.
pub struct Segment {
    pub start: i64,
    pub offset: usize,
    pub len: usize,
    pub audio: Arc<Vec<f32>>,
    pub rate: f64,
    pub sinc: Option<Arc<SincTable>>,
    pub curve: Arc<Curve>,
}

impl Rendered {
    /// The clip sample at timeline sample `at` and the cut's gain there; (0, 0) outside every
    /// region.
    fn at(&self, at: i64) -> (f32, f32) {
        let Some(g) = self.segments.iter().find(|g| at >= g.start && at < g.start + g.len as i64) else { return (0.0, 0.0) };
        let pos = g.offset as f64 + (at - g.start) as f64 * g.rate;
        let v = match &g.sinc {
            Some(t) => t.read(&g.audio, pos),
            None => g.audio.get(pos as usize).copied().unwrap_or(0.0),
        };
        (v, g.curve.at(pos as usize))
    }

    fn near(&self, from: i64, to: i64) -> bool {
        self.segments.iter().any(|g| g.start < to && g.start + g.len as i64 > from)
    }
}

pub struct CutEngine {
    current: Option<Box<Rendered>>,
    /// The render fading out, and how far the new one has come in (0–1).
    old: Option<Box<Rendered>>,
    xfade: f32,
    /// Retired renders the handover was too full to take (given back next block).
    stale: [Option<Box<Rendered>>; STALE],
    /// Bypass and Listen Removed as they glide (0–1).
    bypass: f32,
    removed: f32,
    step: f32,
}

impl CutEngine {
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self { current: None, old: None, xfade: 1.0, stale: Default::default(), bypass: 0.0, removed: 0.0, step: 1.0 };
        e.prepare(sample_rate);
        e
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        self.step = 1.0 / (XFADE_MS * 0.001 * sample_rate).max(1.0);
    }

    pub fn reset(&mut self) {
        self.xfade = 1.0;
        if let Some(o) = self.old.take() {
            self.keep(o);
        }
    }

    /// Holds a retired render until the handover takes it (never freed here).
    fn keep(&mut self, r: Box<Rendered>) {
        match self.stale.iter_mut().find(|s| s.is_none()) {
            Some(slot) => *slot = Some(r),
            // (A worker sends a render per change, not per block: four waiting does not happen.
            // Were it to, leaking one beats freeing on the audio thread.)
            None => std::mem::forget(r),
        }
    }

    fn retire(&mut self, s: &Session, r: Box<Rendered>) {
        if let Err(back) = s.renders.retire(r) {
            self.keep(back);
        }
    }

    /// `channels`: one or two, processed in place.
    pub fn process(&mut self, channels: &mut [&mut [f32]], t: Transport, bypass: bool, removed: bool, s: &Session) {
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return;
        }
        let len = channels.iter().take(n_ch).map(|c| c.len()).min().unwrap_or(0);
        for i in 0..STALE {
            if let Some(r) = self.stale[i].take() {
                self.retire(s, r);
            }
        }
        if let Some(r) = s.renders.take() {
            // The one fading out is replaced at once (a render faster than the fade).
            if let Some(o) = self.old.take() {
                self.retire(s, o);
            }
            self.old = self.current.replace(r);
            self.xfade = if self.old.is_some() { 0.0 } else { 1.0 };
        }
        let Some(cur) = &self.current else { return };
        if !cur.ara {
            return; // an editor-only instance: the input passes
        }
        let Some(pos) = t.pos.filter(|_| t.playing) else {
            for ch in channels.iter_mut().take(n_ch) {
                ch[..len].fill(0.0);
            }
            return;
        };
        let end = pos + len as i64;
        let near = cur.near(pos, end) || self.old.as_ref().is_some_and(|o| o.near(pos, end));
        let (want_b, want_r) = (if bypass { 1.0 } else { 0.0 }, if removed { 1.0 } else { 0.0 });
        if !near {
            for ch in channels.iter_mut().take(n_ch) {
                ch[..len].fill(0.0);
            }
            self.bypass = want_b;
            self.removed = want_r;
            self.xfade = 1.0;
            return;
        }
        for i in 0..len {
            let at = pos + i as i64;
            self.xfade = (self.xfade + self.step).min(1.0);
            self.bypass += (want_b - self.bypass).clamp(-self.step, self.step);
            self.removed += (want_r - self.removed).clamp(-self.step, self.step);
            let (v, mut g) = cur.at(at);
            if self.xfade < 1.0 {
                if let Some(o) = &self.old {
                    let (_, go) = o.at(at);
                    g = go + (g - go) * self.xfade;
                }
            }
            // Listen Removed: what the cut takes away; Bypass: all of it.
            let g = g + (1.0 - 2.0 * g) * self.removed;
            let g = g + (1.0 - g) * self.bypass;
            for ch in channels.iter_mut().take(n_ch) {
                ch[i] = v * g;
            }
        }
        if self.xfade >= 1.0 {
            if let Some(o) = self.old.take() {
                self.retire(s, o);
            }
        }
    }
}
