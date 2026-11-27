//! Mite Cut's engine: drum hits found in whole tracks, offline, and the silences between them cut
//! (2026-11-27, user: "i dont want to manually edit toms ... it is detectable by software").
//! Off the audio thread; allocates freely, except [`Curve::fill`], which the player calls.
//!
//! - **Candidates:** a SuperFlux onset function (log magnitude, so a soft hit rises as much as
//!   a loud one; against the previous frame's neighbouring bins, so a falling pitch is no hit),
//!   its peaks over a moving mean. Each onset: the steepest energy rise followed back to where it
//!   began, then the first sample a linear predictor of what sounded before cannot explain (a
//!   ring is predicted almost exactly, a new hit is not, whatever its phase), snapped back to a
//!   zero crossing. A candidate must add something of its own, band by band, over the loudest of
//!   what sounded in the 45 ms before (two rings beating add nothing).
//! - **What a candidate is made of:** a tom mic hears the snare as loudly as a soft tom, so level
//!   cannot tell them apart. Every other cutting track the user named (kick, snare, toms, hats)
//!   is compared: its bleed into this mic per band is learned from its own hits (first where it
//!   stands furthest above this mic, whichever is the hotter recorded; then from what that
//!   reading keeps), and each candidate is fit, band by band, as this drum's own sound (learned
//!   the same way: the median spectrum of its own hits) plus each track's bleed, each free
//!   within ±6 dB of the learned. What is this drum's, against what the mic heard in its band,
//!   is the candidate's *dominance*: 0 dB for a hit of this drum, well under for bleed, a few dB
//!   under for this drum played with another.
//! - **Who heard it first:** a drum reaches its own close mic first; a candidate another track
//!   heard more than 0.2 ms earlier loses up to 12 dB of dominance, as far as that track's bleed
//!   accounts for it.
//! - **The boundary:** −7 dB of dominance, or a clear gap between −12 and −5 dB. With no tracks
//!   to compare, the level: only what is 30 dB or more under the track's loud hits is cut. A
//!   candidate inside a louder kept hit's ring is part of it.
//! - **Ends:** a hit lasts until its envelope comes down to the track's floor (a low percentile
//!   of the envelope around it: the bleed and room under everything) plus a margin, then holds.
//! - **Render:** each kept hit opens a pre-roll before its onset, holds and fades out; regions
//!   closer than the minimum gap join; outside them the gain is the floor (−∞: cut).

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};
use std::sync::Arc;

use crate::db_to_gain;

/// Onset function frame and hop, samples (at any rate: 10.7 ms and 2.7 ms at 48 kHz).
pub const FRAME: usize = 512;
pub const HOP: usize = 128;
/// The window a candidate's band powers are read over, samples; it starts [`SPEC_LEAD`] before
/// the onset, and what it holds is less what the window before held: what the hit adds.
pub const SPEC: usize = 2048;
const SPEC_LEAD: usize = 64;
/// Log-spaced bands, 30 Hz to 16 kHz.
pub const BANDS: usize = 32;
/// Two onsets closer than this are one hit, s.
const REFRACTORY_S: f32 = 0.015;
/// Onset refinement blocks, samples.
const BLOCK: usize = 16;
/// What was sounding before an onset: this long, s (longer than two toms' rings beat).
const BACKGROUND_S: f32 = 0.03;
/// The rise of an attack is followed back at most this far, s.
const RISE_BACK_S: f32 = 0.004;
/// Onset refinement: a predictor of this order, learned on this long, searching this far
/// before the energy rise for the first sample this many error spreads out.
const LPC_ORDER: usize = 16;
const LPC_TRAIN_S: f32 = 0.02;
const REFINE_BACK_S: f32 = 0.008;
const LPC_SPREADS: f64 = 4.0;
/// A hit adds at least this share of what sounds after its onset (band by band, over the
/// loudest of what sounded before: two rings beating add nothing).
const NOVELTY: f32 = 0.08;
/// Candidates more than this under the track's loud hits count dB for dB against their dominance.
const LEVEL_RANGE_DB: f32 = 30.0;
/// A hit's bleed may be this much (power ratio) louder or softer than the learned one: 6 dB.
const BLEED_SPAN: f32 = 4.0;
/// Arrival: onsets of other tracks within this are the same event, s; one this much earlier
/// starts to count, in full from the next; and then lowers the dominance this much, dB.
const ARRIVAL_WINDOW_S: f32 = 0.006;
const ARRIVAL_MIN_S: f32 = 0.0002;
const ARRIVAL_FULL_S: f32 = 0.0008;
const ARRIVAL_DB: f32 = 12.0;
/// Alone (no tracks to compare), nothing within this of the track's loud hits is cut: a soft
/// hit and bleed cannot be told apart by level, and a hit cut is worse than bleed kept, dB.
const ALONE_KEEP_DB: f32 = 30.0;
/// A kept hit this much softer than the hit whose region it starts in is part of that hit, dB.
const SHADOW_DB: f32 = 12.0;
/// Where the own/bleed boundary may lie (dominance), dB.
const DOMINANCE_RANGE_DB: (f32, f32) = (-12.0, -5.0);
/// The narrowest gap that tells two groups apart, dB.
const MIN_GAP_DB: f32 = 3.0;
/// A flux peak is a candidate over the moving mean times this, plus this.
const FLUX_OVER_MEAN: f32 = 0.8;
const FLUX_MARGIN: f32 = 0.03;
/// A frame quieter than this starts no hit, dBFS.
const SILENT_DB: f32 = -75.0;
/// The share of a track's candidates (its loudest) taken as its own sound, alone, and as the
/// track's loud hits.
const OWN_SHARE: f32 = 0.2;
/// The share of a track's candidates its own sound and bleed are learned from, with other
/// tracks to compare (a tom's mic hears far more bleed than tom hits).
const LEARN_SHARE: f32 = 0.1;
/// A boundary when there are not two groups: dominance, dB (with tracks to compare).
const DEFAULT_DOMINANCE_DB: f32 = -7.0;
/// How sharply confidence turns at the boundary, dB.
const CONFIDENCE_DB: f32 = 2.0;
/// Overrides find their hit within this, s.
const MATCH_S: f32 = 0.01;
/// Floor: blocks of this many hops, each the percentile of its neighbourhood.
const FLOOR_BLOCK: usize = 96;
const FLOOR_SPAN_S: f32 = 1.0;
const FLOOR_PERCENTILE: f32 = 0.2;

/// What a track is. The user names each track; only cutting roles are cut and compared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Role {
    #[default]
    Off,
    Kick,
    Snare,
    Tom,
    Hat,
    Cymbal,
    /// Overheads and room hear everything: never cut, never compared.
    Overhead,
    Room,
}

impl Role {
    pub const ALL: [Role; 8] = [
        Role::Off,
        Role::Kick,
        Role::Snare,
        Role::Tom,
        Role::Hat,
        Role::Cymbal,
        Role::Overhead,
        Role::Room,
    ];

    /// Cut, and compared with the other cutting tracks.
    pub fn cuts(self) -> bool {
        matches!(
            self,
            Role::Kick | Role::Snare | Role::Tom | Role::Hat | Role::Cymbal
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Off => "Off",
            Role::Kick => "Kick",
            Role::Snare => "Snare",
            Role::Tom => "Tom",
            Role::Hat => "Hat",
            Role::Cymbal => "Cymbal",
            Role::Overhead => "OH",
            Role::Room => "Room",
        }
    }
}

/// The tuning. Times in ms, levels in dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// 0…1: moves the own/bleed boundary, 12 dB either way of the found one at 0 and 1.
    pub sensitivity: f32,
    /// 0…1: how loud the other tracks' bleed is taken to be (0.5: as learned; each hit's may
    /// still be 6 dB louder or softer).
    pub bleed: f32,
    pub pre_roll_ms: f32,
    pub fade_in_ms: f32,
    pub hold_ms: f32,
    /// A hit ends this far above the floor.
    pub release_db: f32,
    pub fade_out_ms: f32,
    /// The gain outside the hits (at or under −100: cut).
    pub floor_db: f32,
    /// Regions closer than this join.
    pub min_gap_ms: f32,
    /// A region is at least this long (a short one is held longer, not dropped).
    pub min_length_ms: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            bleed: 0.5,
            pre_roll_ms: 2.0,
            fade_in_ms: 1.0,
            hold_ms: 10.0,
            release_db: 6.0,
            fade_out_ms: 40.0,
            floor_db: -100.0,
            min_gap_ms: 30.0,
            min_length_ms: 30.0,
        }
    }
}

impl Settings {
    fn floor_gain(&self) -> f32 {
        if self.floor_db <= -100.0 {
            0.0
        } else {
            db_to_gain(self.floor_db)
        }
    }
}

/// A possible hit.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The first sample of the hit (after a zero crossing), and its largest.
    pub onset: usize,
    pub peak: usize,
    /// Band power over [`SPEC`] from the onset, weighted by the track's own sound, dBFS.
    pub level_db: f32,
    power: [f32; BANDS],
    /// The compared tracks' band powers at the same moment, and all they sound there (a ring
    /// adds nothing, but is there).
    others: Vec<[f32; BANDS]>,
    others_after: Vec<f32>,
    /// How much earlier each compared track heard this (its nearest onset), s; 0 if it heard
    /// no onset here.
    earlier: Vec<f32>,
}

/// Steps that need no settings, per track: run once, re-read for every setting.
#[derive(Clone, Debug)]
pub struct TrackDetection {
    pub role: Role,
    pub len: usize,
    pub fs: f32,
    pub candidates: Vec<Candidate>,
    /// Which tracks (indices into what [`detect`] took) were compared, and their bleed into this
    /// one per band (power ratio).
    pub compared: Vec<usize>,
    gains: Vec<[f32; BANDS]>,
    template: [f32; BANDS],
    /// Envelope per [`HOP`], dBFS, and the floor per [`FLOOR_BLOCK`] hops.
    env_db: Vec<f32>,
    floor_db: Vec<f32>,
}

/// A hit as the settings read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub onset: usize,
    pub peak: usize,
    /// The region: opens at `start` (pre-roll), holds until `end` (then fades out).
    pub start: usize,
    pub end: usize,
    pub level_db: f32,
    /// What is left after the other tracks' bleed, against what the mic heard, dB (0: all its own).
    pub dominance_db: f32,
    /// 0…1: how surely it is this drum's own hit.
    pub confidence: f32,
    pub kept: bool,
    /// Kept or removed by the user, not the detection.
    pub forced: bool,
    /// Not kept as a hit of its own, but inside a louder kept hit's ring: it sounds.
    pub inside: bool,
}

/// The user's hand on one hit: found again by position after every re-detection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Override {
    pub at: usize,
    pub keep: Option<bool>,
    pub start: Option<usize>,
    pub end: Option<usize>,
}

/// Every track's candidates and bleed model. `tracks`: each mono, with its role; the same rate
/// and timeline (sample 0 is the same moment in each).
pub fn detect(tracks: &[(&[f32], Role)], fs: f32) -> Vec<TrackDetection> {
    let mut bands = Bands::new(fs);
    let mut dets: Vec<TrackDetection> = tracks
        .iter()
        .map(|&(x, role)| {
            let env_db = envelope(x);
            let floor_db = floor(&env_db, fs);
            let candidates = if role == Role::Off {
                Vec::new()
            } else {
                candidates(x, fs, &mut bands)
            };
            TrackDetection {
                role,
                len: x.len(),
                fs,
                candidates,
                compared: Vec::new(),
                gains: Vec::new(),
                template: [1.0 / BANDS as f32; BANDS],
                env_db,
                floor_db,
            }
        })
        .collect();
    compare(&mut dets, tracks, fs, &mut bands);
    learn_gains_seed(&mut dets);
    let mut own = seed_own(&dets);
    learn_templates(&mut dets, &own);
    // Then twice from what that reading keeps (the louder half): many more hits, and none
    // picked for standing out (those are the ones whose bleed happened to be softest).
    for _ in 0..2 {
        let next: Vec<Vec<usize>> = dets
            .iter()
            .map(|d| {
                let kept: Vec<usize> = d
                    .hits(&Settings::default(), &[])
                    .iter()
                    .zip(0..)
                    .filter(|(h, _)| h.kept)
                    .map(|(_, i)| i)
                    .collect();
                let mut v = kept;
                v.sort_by(|&a, &b| {
                    d.candidates[b]
                        .level_db
                        .total_cmp(&d.candidates[a].level_db)
                });
                v.truncate(v.len().div_ceil(2));
                v
            })
            .collect();
        // (A track that keeps too few keeps what it was learned from.)
        own = next
            .into_iter()
            .zip(own)
            .map(|(n, o)| if n.len() < 3 { o } else { n })
            .collect();
        for t in 0..dets.len() {
            let gains: Vec<[f32; BANDS]> = (0..dets[t].compared.len())
                .map(|k| {
                    let o = dets[t].compared[k];
                    let Some(kk) = dets[o].compared.iter().position(|&x| x == t) else {
                        return dets[t].gains[k];
                    };
                    if own[o].len() < 3 {
                        return dets[t].gains[k];
                    }
                    std::array::from_fn(|b| {
                        let mut v: Vec<f32> = own[o]
                            .iter()
                            .map(|&i| {
                                dets[o].candidates[i].others[kk][b]
                                    / dets[o].candidates[i].power[b].max(1e-30)
                            })
                            .collect();
                        v.sort_by(f32::total_cmp);
                        v[v.len() / 2]
                    })
                })
                .collect();
            dets[t].gains = gains;
        }
        learn_templates(&mut dets, &own);
    }
    dets
}

impl TrackDetection {
    /// The candidates as `s` reads them, with the user's overrides; kept or not, in time order.
    pub fn hits(&self, s: &Settings, overrides: &[Override]) -> Vec<Hit> {
        if !self.role.cuts() {
            return Vec::new();
        }
        let beta = 2.0 * s.bleed.clamp(0.0, 1.0);
        let compare = !self.compared.is_empty();
        // With tracks to compare, the dominance, less how far a candidate is under the track's
        // loud hits beyond [`LEVEL_RANGE_DB`] (a tiny increment's ratio means nothing); alone,
        // the level.
        let mut levels: Vec<f32> = self.candidates.iter().map(|c| c.level_db).collect();
        levels.sort_by(|a, b| b.total_cmp(a));
        let loud = levels
            .get(
                ((levels.len() as f32 * OWN_SHARE) as usize / 2)
                    .min(levels.len().saturating_sub(1)),
            )
            .copied()
            .unwrap_or(0.0);
        let feature: Vec<f32> = self
            .candidates
            .iter()
            .map(|c| {
                if compare {
                    self.dominance(c, beta) + (c.level_db - (loud - LEVEL_RANGE_DB)).min(0.0)
                } else {
                    c.level_db
                }
            })
            .collect();
        let range = if compare {
            (DOMINANCE_RANGE_DB.0, DOMINANCE_RANGE_DB.1)
        } else {
            (loud - LEVEL_RANGE_DB - 15.0, loud - ALONE_KEEP_DB)
        };
        let (boundary, width) = boundary(&feature, range).unwrap_or(if compare {
            (DEFAULT_DOMINANCE_DB, 2.0 * CONFIDENCE_DB)
        } else {
            (f32::NEG_INFINITY, 0.0)
        });
        let scale = (width / 4.0).max(0.5 * CONFIDENCE_DB);
        let shift = (s.sensitivity.clamp(0.0, 1.0) - 0.5) * 24.0;
        let mut hits: Vec<Hit> = self
            .candidates
            .iter()
            .zip(&feature)
            .map(|(c, &f)| {
                let confidence = if boundary.is_finite() {
                    1.0 / (1.0 + (-(f + shift - boundary) / scale).exp())
                } else {
                    1.0
                };
                Hit {
                    onset: c.onset,
                    peak: c.peak,
                    start: usize::MAX,
                    end: usize::MAX,
                    level_db: c.level_db,
                    dominance_db: if compare { f } else { 0.0 },
                    confidence,
                    kept: confidence > 0.5,
                    forced: false,
                    inside: false,
                }
            })
            .collect();
        let near = (MATCH_S * self.fs) as usize;
        for o in overrides {
            let i = hits
                .iter()
                .enumerate()
                .filter(|(_, h)| h.onset.abs_diff(o.at) <= near)
                .min_by_key(|(_, h)| h.onset.abs_diff(o.at))
                .map(|(i, _)| i);
            let i = match i {
                Some(i) => i,
                None if o.keep == Some(true) => {
                    let k = (o.at / HOP).min(self.env_db.len().saturating_sub(1));
                    hits.push(Hit {
                        onset: o.at,
                        peak: o.at,
                        start: usize::MAX,
                        end: usize::MAX,
                        level_db: self.env_db.get(k).copied().unwrap_or(-150.0),
                        dominance_db: 0.0,
                        confidence: 1.0,
                        kept: true,
                        forced: true,
                        inside: false,
                    });
                    hits.len() - 1
                }
                None => continue,
            };
            if let Some(k) = o.keep {
                hits[i].kept = k;
                hits[i].forced = true;
            }
            if let Some(a) = o.start {
                hits[i].start = a;
            }
            if let Some(e) = o.end {
                hits[i].end = e;
            }
        }
        hits.sort_by_key(|h| h.onset);
        // Ends: down to the floor plus the margin, never past the next kept hit.
        let pre = (s.pre_roll_ms * 1e-3 * self.fs) as usize;
        let hold = (s.hold_ms * 1e-3 * self.fs) as usize;
        let min_len = (s.min_length_ms * 1e-3 * self.fs) as usize;
        let end_of = |h: &Hit, next: usize| {
            let mut k = h.peak / HOP;
            let top = self.env_db.get(k).copied().unwrap_or(-150.0);
            while k + 1 < self.env_db.len() && (k + 1) * HOP < next {
                let fl = self.floor_db[(k / FLOOR_BLOCK).min(self.floor_db.len() - 1)];
                let e = self.env_db[k + 1];
                if e < fl + s.release_db || e < top - 60.0 {
                    break;
                }
                k += 1;
            }
            ((k + 1) * HOP + hold).max(h.onset + min_len).min(self.len)
        };
        // A kept hit inside a louder one's ring (by [`SHADOW_DB`]) is part of it: a swell of
        // the ring or a soft hit into it, sounding either way, no hit of its own.
        let mut owner: Option<(usize, f32)> = None;
        for h in hits.iter_mut().filter(|h| h.kept) {
            if let Some((end, level)) = owner {
                if !h.forced && h.onset < end && h.level_db < level - SHADOW_DB {
                    h.kept = false;
                    h.inside = true;
                    continue;
                }
            }
            let end = if h.end == usize::MAX { end_of(h, self.len) } else { h.end };
            if owner.is_none_or(|(e, _)| end > e || h.onset >= e) {
                owner = Some((end, h.level_db));
            }
        }
        let kept_onsets: Vec<usize> = hits.iter().filter(|h| h.kept).map(|h| h.onset).collect();
        for h in &mut hits {
            let next = kept_onsets.iter().copied().find(|&o| o > h.onset).unwrap_or(self.len);
            if h.start == usize::MAX {
                h.start = h.onset.saturating_sub(pre);
            }
            if h.end == usize::MAX {
                h.end = end_of(h, next);
            }
        }
        hits
    }

    /// `c`'s band powers as this drum's own sound (the template, scaled by `a ≥ 0`) plus each
    /// compared track's predicted bleed, scaled within [`BLEED_SPAN`] of `beta` (a hit's bleed
    /// is a dB or two louder or softer than the median): least squares on the bands' error (weighted by this drum's band, and less steeply than relative: a band the hit adds
    /// nothing to must not decide it). Returns `a` and the bleed scales.
    fn fit(&self, c: &Candidate, beta: f32) -> (f32, Vec<f32>) {
        let n = self.compared.len();
        let bleed: Vec<[f32; BANDS]> = (0..n)
            .map(|k| std::array::from_fn(|b| self.gains[k][b] * c.others[k][b]))
            .collect();
        let top = c.power.iter().copied().fold(0.0f32, f32::max);
        let w: [f32; BANDS] =
            std::array::from_fn(|b| self.template[b] / (c.power[b] + 0.03 * top + 1e-30));
        let (lo, hi) = (beta / BLEED_SPAN, beta * BLEED_SPAN);
        let mut x: Vec<f32> = std::iter::once(0.0)
            .chain(std::iter::repeat_n(beta, n))
            .collect();
        let basis = |j: usize, b: usize| {
            if j == 0 {
                self.template[b]
            } else {
                bleed[j - 1][b]
            }
        };
        for _ in 0..30 {
            for j in 0..=n {
                let (mut num, mut den) = (0.0f32, 0.0f32);
                for (b, w) in w.iter().enumerate() {
                    let rest: f32 = c.power[b]
                        - (0..=n)
                            .filter(|&k| k != j)
                            .map(|k| x[k] * basis(k, b))
                            .sum::<f32>();
                    let v = basis(j, b);
                    num += w * rest * v;
                    den += w * v * v;
                }
                let v = if den > 0.0 { num / den } else { x[j] };
                x[j] = if j == 0 { v.max(0.0) } else { v.clamp(lo, hi) };
            }
        }
        (x[0], x[1..].to_vec())
    }

    /// What of `c` is this drum's own, against what this mic heard (in its band), dB (≤ 0);
    /// less, for each track that heard it earlier, [`ARRIVAL_DB`] in proportion to how much
    /// earlier (in full from [`ARRIVAL_FULL_S`]) and to the root of the share of what starts
    /// here that track's bleed accounts for (a kick's sweep into a tom's band is only part of
    /// it). A drum reaches its own close mic first.
    fn dominance(&self, c: &Candidate, beta: f32) -> f32 {
        let heard: f32 = (0..BANDS)
            .map(|b| self.template[b] * c.power[b])
            .sum::<f32>()
            .max(1e-30);
        let (a, scale) = self.fit(c, beta);
        let own: f32 = (0..BANDS)
            .map(|b| self.template[b] * (a * self.template[b]).min(c.power[b]))
            .sum();
        // (The share over all bands: a hat's bleed is nothing in a tom's band, but all that
        // starts there.)
        let all: f32 = c.power.iter().sum::<f32>().max(1e-30);
        let late: f32 = (0..self.compared.len())
            .map(|k| {
                let share = ((0..BANDS)
                    .map(|b| (scale[k] * self.gains[k][b] * c.others[k][b]).min(c.power[b]))
                    .sum::<f32>()
                    / all)
                    .min(1.0);
                ((c.earlier[k] - ARRIVAL_MIN_S) / (ARRIVAL_FULL_S - ARRIVAL_MIN_S)).clamp(0.0, 1.0)
                    * share.sqrt()
            })
            .fold(0.0, f32::max);
        10.0 * (own.max(heard * 1e-6) / heard).log10() - ARRIVAL_DB * late
    }

    /// This drum's own sound as learned: the share of its power in each band.
    pub fn sound(&self) -> &[f32; BANDS] {
        &self.template
    }

    /// A candidate's band powers (what it adds).
    pub fn power(&self, i: usize) -> Option<&[f32; BANDS]> {
        self.candidates.get(i).map(|c| &c.power)
    }

    /// Why candidate `i` reads as it does: the bleed each compared track accounts for there
    /// (track index, dB against what this mic heard), loudest first.
    pub fn blame(&self, i: usize, s: &Settings) -> Vec<(usize, f32)> {
        let Some(c) = self.candidates.get(i) else {
            return Vec::new();
        };
        let heard: f32 = (0..BANDS)
            .map(|b| self.template[b] * c.power[b])
            .sum::<f32>()
            .max(1e-30);
        let (_, scale) = self.fit(c, 2.0 * s.bleed);
        let mut v: Vec<(usize, f32)> = (0..self.compared.len())
            .map(|k| {
                (
                    self.compared[k],
                    10.0 * ((0..BANDS)
                        .map(|b| self.template[b] * scale[k] * self.gains[k][b] * c.others[k][b])
                        .sum::<f32>()
                        .max(1e-30)
                        / heard)
                        .log10(),
                )
            })
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }

    /// The gain curve the kept hits make.
    pub fn curve(&self, hits: &[Hit], s: &Settings) -> Curve {
        Curve::new(hits, s, self.fs)
    }
}

/// Kept regions as gain, read by the player block by block without allocating.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    /// (open from, full at, full until, closed at), samples, in order, not overlapping.
    spans: Vec<[usize; 4]>,
    floor: f32,
}

impl Curve {
    pub fn new(hits: &[Hit], s: &Settings, fs: f32) -> Self {
        let fade_in = ((s.fade_in_ms * 1e-3 * fs) as usize).max(1);
        let fade_out = ((s.fade_out_ms * 1e-3 * fs) as usize).max(1);
        let gap = (s.min_gap_ms * 1e-3 * fs) as usize;
        let mut kept: Vec<(usize, usize)> = hits
            .iter()
            .filter(|h| h.kept)
            .map(|h| (h.start, h.end.max(h.start + 1)))
            .collect();
        kept.sort();
        let mut spans: Vec<[usize; 4]> = Vec::new();
        for (a, e) in kept {
            if let Some(last) = spans.last_mut() {
                if a < last[3] + gap {
                    last[2] = last[2].max(e);
                    last[3] = last[2] + fade_out;
                    continue;
                }
            }
            // (A region from the clip's first sample opens there at once: no fade over the hit.)
            let full = if a == 0 { 0 } else { a + fade_in };
            spans.push([a, full, e.max(full), e.max(full) + fade_out]);
        }
        Self {
            spans,
            floor: s.floor_gain(),
        }
    }

    /// The kept regions: (open from, closed at), samples.
    pub fn regions(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.spans.iter().map(|s| (s[0], s[3]))
    }

    /// The gain at sample `n`.
    pub fn at(&self, n: usize) -> f32 {
        let i = self.spans.partition_point(|s| s[3] <= n);
        let open = self.spans.get(i).map_or(0.0, |s| shape(s, n));
        self.floor + (1.0 - self.floor) * open
    }

    /// Gains for samples `from..from + out.len()`. Allocation-free.
    pub fn fill(&self, from: usize, out: &mut [f32]) {
        let mut i = self.spans.partition_point(|s| s[3] <= from);
        for (k, g) in out.iter_mut().enumerate() {
            let n = from + k;
            while i < self.spans.len() && self.spans[i][3] <= n {
                i += 1;
            }
            let open = self.spans.get(i).map_or(0.0, |s| shape(s, n));
            *g = self.floor + (1.0 - self.floor) * open;
        }
    }
}

fn shape(s: &[usize; 4], n: usize) -> f32 {
    let ease = |t: f32| (t * std::f32::consts::FRAC_PI_2).sin().powi(2);
    if n < s[0] || n >= s[3] {
        0.0
    } else if n < s[1] {
        ease((n - s[0]) as f32 / (s[1] - s[0]) as f32)
    } else if n < s[2] {
        1.0
    } else {
        ease(1.0 - (n - s[2]) as f32 / (s[3] - s[2]) as f32)
    }
}

/// The boundary between the high group (own hits) and the rest: the middle of the highest gap
/// between neighbouring values whose middle lies in `range`, at least [`MIN_GAP_DB`] wide with
/// two values either side (lower down, gaps part one kind of bleed from another); or none (one
/// group: nothing to tell apart).
fn boundary(v: &[f32], range: (f32, f32)) -> Option<(f32, f32)> {
    let mut sorted = v.to_vec();
    sorted.sort_by(f32::total_cmp);
    let mut best: Option<(f32, f32)> = None;
    for i in 1..sorted.len().saturating_sub(2) {
        if i < 2 {
            continue;
        }
        let (a, b) = (sorted[i - 1], sorted[i]);
        let mid = 0.5 * (a + b);
        if mid < range.0 || mid > range.1 || b - a < MIN_GAP_DB {
            continue;
        }
        best = Some((mid, b - a)); // the highest: under the own hits, before kinds of bleed
    }
    best
}

/// Bleed of o into t, per band: at o's candidates where o stands furthest above t (o's own
/// hits, whichever mic is the louder or the hotter recorded: a hat mic's loudest are the
/// snare's bleed, but there the snare mic stands far above it).
fn learn_gains_seed(dets: &mut [TrackDetection]) {
    for t in 0..dets.len() {
        let gains: Vec<[f32; BANDS]> = dets[t]
            .compared
            .iter()
            .map(|&o| {
                let mut g = [0.0f32; BANDS];
                let Some(k) = dets[o].compared.iter().position(|&x| x == t) else {
                    return g;
                };
                let cs = &dets[o].candidates;
                // (Against all t sounds there: where t still rings, its increment is nothing.)
                let lead =
                    |c: &Candidate| c.power.iter().sum::<f32>() / (c.others_after[k] + 1e-30);
                let src = top(cs, LEARN_SHARE, |c| lead(c));
                if src.len() < 3 {
                    return g;
                }
                for (b, g) in g.iter_mut().enumerate() {
                    let mut v: Vec<f32> = src
                        .iter()
                        .map(|&i| cs[i].others[k][b] / cs[i].power[b].max(1e-30))
                        .collect();
                    v.sort_by(f32::total_cmp);
                    *g = v[v.len() / 2];
                }
                g
            })
            .collect();
        dets[t].gains = gains;
    }
}

/// Each track's own sound to learn from: its candidates standing furthest above every bleed
/// predicted there (alone, its loudest).
fn seed_own(dets: &[TrackDetection]) -> Vec<Vec<usize>> {
    dets.iter()
        .map(|d| {
            if d.compared.is_empty() {
                return top(&d.candidates, OWN_SHARE, |c| c.power.iter().sum());
            }
            top(&d.candidates, LEARN_SHARE, |c| {
                let p: f32 = c.power.iter().sum();
                d.gains
                    .iter()
                    .zip(&c.others)
                    .map(|(g, o)| p / ((0..BANDS).map(|b| g[b] * o[b]).sum::<f32>() + 1e-30))
                    .fold(f32::INFINITY, f32::min)
            })
        })
        .collect()
}

/// Every cutting track's candidates: what every other cutting track adds there, all it sounds
/// there, and how much earlier it heard an onset.
fn compare(dets: &mut [TrackDetection], tracks: &[(&[f32], Role)], fs: f32, bands: &mut Bands) {
    for t in 0..tracks.len() {
        if !tracks[t].1.cuts() {
            continue;
        }
        let compared: Vec<usize> = (0..tracks.len())
            .filter(|&o| o != t && tracks[o].1.cuts())
            .collect();
        let others: Vec<Vec<([f32; BANDS], f32)>> = dets[t]
            .candidates
            .iter()
            .map(|c| {
                compared
                    .iter()
                    .map(|&o| bands.novelty(tracks[o].0, c.onset))
                    .collect()
            })
            .collect();
        for (c, o) in dets[t].candidates.iter_mut().zip(others) {
            c.others_after = o.iter().map(|p| p.1).collect();
            c.others = o.into_iter().map(|p| p.0).collect();
        }
        dets[t].compared = compared;
    }
    let onsets: Vec<Vec<usize>> = dets
        .iter()
        .map(|d| d.candidates.iter().map(|c| c.onset).collect())
        .collect();
    let near = (ARRIVAL_WINDOW_S * fs) as usize;
    for d in dets.iter_mut() {
        let compared = d.compared.clone();
        for c in &mut d.candidates {
            c.earlier = compared
                .iter()
                .map(|&o| {
                    let v = &onsets[o];
                    let i = v.partition_point(|&x| x < c.onset.saturating_sub(near));
                    v[i..]
                        .iter()
                        .take_while(|&&x| x <= c.onset + near)
                        .min_by_key(|&&x| x.abs_diff(c.onset))
                        .map_or(0.0, |&x| (c.onset as f32 - x as f32) / fs)
                })
                .collect();
        }
    }
}

/// Each track's own sound from its `own` candidates, and every candidate's level in it.
fn learn_templates(dets: &mut [TrackDetection], own: &[Vec<usize>]) {
    for (d, own) in dets.iter_mut().zip(own) {
        // The median share per band: a few of them with another drum in them do not count.
        let shares: Vec<[f32; BANDS]> = own
            .iter()
            .map(|&i| {
                let p = &d.candidates[i].power;
                let total: f32 = p.iter().sum::<f32>().max(1e-30);
                p.map(|v| v / total)
            })
            .collect();
        let template: [f32; BANDS] = std::array::from_fn(|b| {
            let mut v: Vec<f32> = shares.iter().map(|s| s[b]).collect();
            v.sort_by(f32::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        });
        let sum: f32 = template.iter().sum();
        d.template = if sum > 0.0 {
            template.map(|t| t / sum)
        } else {
            [1.0 / BANDS as f32; BANDS]
        };
        let template = d.template;
        for c in &mut d.candidates {
            c.level_db = weighted_db(&c.power, &template);
        }
    }
}

/// The candidates with the largest `key`: `share` of them, at least 3 where there are.
fn top(c: &[Candidate], share: f32, key: impl Fn(&Candidate) -> f32) -> Vec<usize> {
    let mut v: Vec<usize> = (0..c.len()).collect();
    v.sort_by(|&a, &b| key(&c[b]).total_cmp(&key(&c[a])));
    let n = ((v.len() as f32 * share).ceil() as usize)
        .max(3)
        .min(v.len());
    v.truncate(n);
    v
}

fn weighted_db(p: &[f32; BANDS], w: &[f32; BANDS]) -> f32 {
    10.0 * p
        .iter()
        .zip(w)
        .map(|(p, w)| p * w)
        .sum::<f32>()
        .max(1e-30)
        .log10()
}

/// RMS over two hops around each hop, dBFS.
fn envelope(x: &[f32]) -> Vec<f32> {
    (0..x.len().div_ceil(HOP))
        .map(|k| {
            let a = (k * HOP).saturating_sub(HOP / 2);
            let b = (k * HOP + HOP + HOP / 2).min(x.len());
            let e = x[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a).max(1) as f32;
            10.0 * e.max(1e-15).log10()
        })
        .collect()
}

/// The track's floor per [`FLOOR_BLOCK`] hops: a low percentile of the envelope around it.
fn floor(env: &[f32], fs: f32) -> Vec<f32> {
    let span = ((FLOOR_SPAN_S * fs) as usize / HOP).max(FLOOR_BLOCK);
    (0..env.len().div_ceil(FLOOR_BLOCK).max(1))
        .map(|b| {
            let c = b * FLOOR_BLOCK + FLOOR_BLOCK / 2;
            let a = c.saturating_sub(span).min(env.len());
            let e = (c + span).min(env.len());
            let mut w: Vec<f32> = env[a..e].to_vec();
            if w.is_empty() {
                return -150.0;
            }
            w.sort_by(f32::total_cmp);
            w[((w.len() - 1) as f32 * FLOOR_PERCENTILE) as usize]
        })
        .collect()
}

/// Band powers over [`SPEC`] samples.
struct Bands {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    edges: [usize; BANDS + 1],
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
}

impl Bands {
    fn new(fs: f32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SPEC);
        let window = hann(SPEC);
        let bin = fs / SPEC as f32;
        let mut edges = [0usize; BANDS + 1];
        let mut prev = 0;
        for (b, e) in edges.iter_mut().enumerate() {
            let f = 30.0 * (16_000.0f32 / 30.0).powf(b as f32 / BANDS as f32);
            *e = ((f / bin).round() as usize).max(prev + 1).min(SPEC / 2);
            prev = *e;
        }
        let input = fft.make_input_vec();
        let spectrum = fft.make_output_vec();
        Self {
            fft,
            window,
            edges,
            input,
            spectrum,
        }
    }

    /// What a hit at `onset` adds: the band powers over [`SPEC`] from [`SPEC_LEAD`] before it,
    /// less those over the [`SPEC`] before that (a ring or tail already sounding cancels).
    /// [`Self::at`], and the total power after the onset.
    fn novelty(&mut self, x: &[f32], onset: usize) -> ([f32; BANDS], f32) {
        let a = onset as isize - SPEC_LEAD as isize;
        // Before: the loudest of three windows, so rings beating against each other add nothing.
        let post = self.raw(x, a);
        let pre = [1.0, 1.5, 2.0].map(|k| self.raw(x, a - (k * SPEC as f32) as isize));
        (
            std::array::from_fn(|b| (post[b] - pre[0][b].max(pre[1][b]).max(pre[2][b])).max(0.0)),
            post.iter().sum(),
        )
    }

    fn raw(&mut self, x: &[f32], a: isize) -> [f32; BANDS] {
        for (i, v) in self.input.iter_mut().enumerate() {
            let n = a + i as isize;
            *v = if n >= 0 && (n as usize) < x.len() {
                x[n as usize] * self.window[i]
            } else {
                0.0
            };
        }
        let _ = self.fft.process(&mut self.input, &mut self.spectrum);
        let norm = 4.0 / (SPEC as f32 * SPEC as f32);
        let p: [f32; BANDS] = std::array::from_fn(|b| {
            self.spectrum[self.edges[b]..self.edges[b + 1]]
                .iter()
                .map(|c| c.norm_sqr())
                .sum::<f32>()
                * norm
        });
        p
    }
}

/// The first sample at or before `near` (up to [`REFINE_BACK_S`] before it) where `x` stops
/// being what came before it predicts: a linear predictor (order [`LPC_ORDER`]) learned on the
/// [`LPC_TRAIN_S`] before the search, its error against the error's own spread there. A ring is
/// a sum of decaying sinusoids, predicted almost exactly; a new hit is not, however soft and
/// whatever its phase. `near` itself if nothing stands out.
fn refine(x: &[f32], near: usize, fs: f32) -> usize {
    let back = (REFINE_BACK_S * fs) as usize;
    let train = (LPC_TRAIN_S * fs) as usize;
    let from = near.saturating_sub(back);
    if from < train + LPC_ORDER {
        return near;
    }
    let seg = &x[from - train..from];
    // Autocorrelation (Hann-windowed), Levinson-Durbin.
    let w: Vec<f64> = seg
        .iter()
        .enumerate()
        .map(|(i, v)| {
            f64::from(*v) * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / train as f64).cos())
        })
        .collect();
    let mut r = [0.0f64; LPC_ORDER + 1];
    for (lag, r) in r.iter_mut().enumerate() {
        *r = (lag..w.len()).map(|i| w[i] * w[i - lag]).sum();
    }
    if r[0] <= 1e-18 {
        return near; // silence before: the energy rise is exact enough
    }
    r[0] *= 1.0 + 1e-9;
    let mut a = [0.0f64; LPC_ORDER + 1];
    a[0] = 1.0;
    let mut err = r[0];
    for i in 1..=LPC_ORDER {
        let k = -(0..i).map(|j| a[j] * r[i - j]).sum::<f64>() / err;
        let prev = a;
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        if err <= 0.0 {
            return near;
        }
    }
    let e = |n: usize| -> f64 { (0..=LPC_ORDER).map(|j| a[j] * f64::from(x[n - j])).sum() };
    let spread = ((from - train / 2..from).map(|n| e(n).powi(2)).sum::<f64>() / (train / 2) as f64)
        .sqrt()
        .max(1e-9);
    let hold = (0.0005 * fs) as usize;
    let end = (near + (0.001 * fs) as usize).min(x.len().saturating_sub(hold));
    // The first sample well out of the spread, and staying out on average over the next half
    // millisecond (a click of noise does not).
    (from..end)
        .find(|&n| {
            e(n).abs() > LPC_SPREADS * spread
                && (n..n + hold).map(|m| e(m).powi(2)).sum::<f64>() / hold as f64
                    > (LPC_SPREADS * spread).powi(2) * 0.25
        })
        .unwrap_or(near)
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect()
}

/// Onsets: spectral flux peaks, each refined to its sample; with their band powers.
fn candidates(x: &[f32], fs: f32, bands: &mut Bands) -> Vec<Candidate> {
    if x.len() < FRAME * 2 {
        return Vec::new();
    }
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
    let window = hann(FRAME);
    let mut input = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let top = ((16_000.0 / fs * FRAME as f32) as usize).min(FRAME / 2);
    let frames = (x.len() - FRAME) / HOP + 1;
    let mut prev = vec![0.0f32; top + 3];
    let mut cur = vec![0.0f32; top + 3];
    let mut flux = vec![0.0f32; frames];
    let mut loud = vec![false; frames];
    let norm = 4.0 / FRAME as f32;
    let silent = db_to_gain(SILENT_DB);
    for k in 0..frames {
        let a = k * HOP;
        let mut e = 0.0f32;
        for (i, v) in input.iter_mut().enumerate() {
            *v = x[a + i] * window[i];
            e += x[a + i] * x[a + i];
        }
        loud[k] = (e / FRAME as f32).sqrt() > silent;
        let _ = fft.process(&mut input, &mut spec);
        // SuperFlux (Böck & Widmer 2013): against the previous frame's maximum over the
        // neighbouring bins, so a tom's falling pitch is no new hit.
        let mut f = 0.0;
        for i in 1..=top {
            cur[i] = (1.0 + 1e4 * spec[i].norm() * norm).ln();
            let m = prev[i - 1]
                .max(prev[i])
                .max(prev[i + 1])
                .max(prev[(i + 2).min(top)])
                .max(prev[i.saturating_sub(2)]);
            f += (cur[i] - m).max(0.0);
        }
        std::mem::swap(&mut prev, &mut cur);
        flux[k] = f / top as f32;
    }
    // Peaks over a moving mean (a quarter second either side) plus a margin: low, as what is
    // no hit is told apart later (novelty, dominance), and a fill raises the mean.
    let w = ((0.25 * fs) as usize / HOP).max(4);
    let mut sum = vec![0.0f64; frames + 1];
    for k in 0..frames {
        sum[k + 1] = sum[k] + f64::from(flux[k]);
    }
    let local = 4;
    let refractory = (REFRACTORY_S * fs) as usize;
    let mut out: Vec<Candidate> = Vec::new();
    // (From frame 0: a clip cut right on a hit starts with it.)
    for k in 0..frames {
        let (a, b) = (k.saturating_sub(w), (k + w + 1).min(frames));
        let mean = ((sum[b] - sum[a]) / (b - a) as f64) as f32;
        let (la, lb) = (k.saturating_sub(local), (k + local + 1).min(frames));
        if !loud[k] && !loud[(k + 1).min(frames - 1)]
            || flux[k] < mean * FLUX_OVER_MEAN + FLUX_MARGIN
            || flux[la..lb].iter().any(|&f| f > flux[k])
            || flux[la..k].contains(&flux[k])
        {
            continue;
        }
        // The onset: in short blocks around the frame, the steepest rise of the energy against
        // what sounded before, followed back to where the rise began; then to the sample (see
        // [`refine`]), snapped back to a zero crossing within half a millisecond.
        let blk = BLOCK;
        let lo = (k * HOP).saturating_sub(FRAME) / blk;
        let hi = ((k * HOP + FRAME + HOP).min(x.len()) / blk).max(lo + 1);
        let energy = |j: usize| {
            x[j * blk..((j + 1) * blk).min(x.len())]
                .iter()
                .map(|v| v * v)
                .sum::<f32>()
                / blk as f32
        };
        // Against the loudest block of the [`BACKGROUND_S`] before (longer than a 50 Hz period,
        // so a low drum's own cycles are no rise).
        let back_blocks = (BACKGROUND_S * fs) as usize / blk + 1;
        let ratio = |j: usize| {
            let a = j.saturating_sub(back_blocks + 1);
            let b = j.saturating_sub(1).max(a + 1);
            let bg = (a..b).map(energy).fold(0.0f32, f32::max);
            energy(j) / (bg + 1e-12)
        };
        let ratios: Vec<f32> = (lo..hi).map(ratio).collect();
        let Some(steepest) = ratios
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(j, _)| j)
        else {
            continue;
        };
        // Back from the steepest rise while each block is still louder than all before it (an
        // attack rises within a few milliseconds; a ring swelling as it beats, for longer).
        let mut j = steepest;
        let limit = steepest.saturating_sub((RISE_BACK_S * fs) as usize / blk);
        while j > limit && ratios[j - 1] > 1.0 {
            j -= 1;
        }
        let start = (j + lo) * blk;
        let end = (start + FRAME).min(x.len());
        let peak = start
            + x[start..end]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .map_or(0, |(i, _)| i);
        let mut onset = refine(x, start, fs);
        let back = (0.0005 * fs) as usize;
        if let Some(z) = (onset.saturating_sub(back).max(1)..=onset)
            .rev()
            .find(|&n| x[n - 1] * x[n] <= 0.0)
        {
            onset = z - 1;
        }
        if let Some(last) = out.last() {
            if onset < last.onset + refractory {
                continue;
            }
        }
        let (power, after) = bands.novelty(x, onset);
        if power.iter().sum::<f32>() < NOVELTY * after {
            continue; // a ring or a tail moving, nothing new
        }
        out.push(Candidate {
            onset,
            peak,
            level_db: 0.0,
            power,
            others: Vec::new(),
            others_after: Vec::new(),
            earlier: Vec::new(),
        });
    }
    out
}
