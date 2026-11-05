//! Mite Pitch's engine (docs/design/pitch-brief.md §2): a monophonic take analysed into notes, and
//! resynthesised with each note's pitch, drift, timing, length, formant and level as edited.
//! Offline, off the audio thread; allocates freely.
//!
//! - **Analysis:** the pitch every [`HOP_S`] (the McLeod method, `crate::pitch`), voicing, the
//!   level; notes cut where the (vibrato-smoothed) pitch steps or the level dips between
//!   syllables; each note's centre (the median of its steady middle) and drift (the pitch curve
//!   less the centre); pitch marks, one per period at the waveform's main peak.
//! - **Resynthesis (TD-PSOLA):** each output period takes a two-period Hann grain of the input
//!   around the mark nearest the matching input time and lays it down at the new period spacing,
//!   so the pitch moves and the formants stay; a grain is resampled first to move the formants.
//!   A time map from output to input moves and stretches notes. Unvoiced stretches are carried in
//!   short grains. Wherever nothing is edited the input passes exactly (crossfaded into what is).

use crate::pitch::PitchDetector;

/// Analysis hop, s.
pub const HOP_S: f64 = 0.005;
/// The pitch window for voice, s (60 Hz takes 2.4 periods).
pub const WINDOW_S: f64 = 0.04;
pub const MIN_HZ: f64 = 60.0;
pub const MAX_HZ: f64 = 1500.0;
/// Voicing: clarity at least this, level within [`VOICED_RANGE_DB`] of the take's loud level (its
/// frames' 95th percentile; a stem recorded quietly is read as one recorded hot), and above
/// [`FLOOR_DB`] at any rate.
pub const VOICED_CLARITY: f64 = 0.8;
pub const VOICED_RANGE_DB: f64 = 40.0;
pub const FLOOR_DB: f64 = -80.0;
/// A note is cut where the vibrato-smoothed pitch moves this much over [`STEP_SPAN_S`].
pub const STEP_CENTS: f64 = 80.0;
pub const STEP_SPAN_S: f64 = 0.06;
/// The pitch smoothing that hides vibrato from the cutting, s (about one vibrato cycle).
pub const SMOOTH_S: f64 = 0.15;
/// A level dip of this much between two louder points cuts a note (a new syllable), dB.
pub const DIP_DB: f64 = 6.0;
/// Notes shorter than this join a neighbour, s.
pub const MIN_NOTE_S: f64 = 0.05;
/// Where edited audio meets the input, a crossfade this long, s.
pub const FADE_S: f64 = 0.01;
/// Unvoiced grains: this long, laid every half of it, s.
pub const NOISE_GRAIN_S: f64 = 0.01;

/// MIDI note numbers × 100: cents above C−1.
pub fn hz_to_cents(hz: f64) -> f64 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

pub fn cents_to_hz(cents: f64) -> f64 {
    440.0 * 2f64.powf((cents - 6900.0) / 1200.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// The pitch, cents (MIDI × 100), when voiced.
    pub cents: Option<f64>,
    pub clarity: f64,
    pub db: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// First and last sample (exclusive).
    pub start: usize,
    pub end: usize,
    /// Its pitch centre, cents (MIDI × 100).
    pub cents: f64,
    /// The pitch less the centre, one per frame from `start` (unvoiced frames inside it
    /// filled in), cents.
    pub drift: Vec<f64>,
    /// The slow part of the drift (smoothed over about a vibrato cycle: slides, sags, the
    /// approach), one per frame; the rest (`drift − slow`) is the modulation (vibrato, wobble).
    pub slow: Vec<f64>,
    /// Its RMS, dBFS.
    pub db: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub fs: f64,
    pub hop: usize,
    pub frames: Vec<Frame>,
    pub notes: Vec<Note>,
    /// Pitch marks: one sample per period through every voiced stretch, sorted.
    pub marks: Vec<usize>,
}

impl Analysis {
    /// The frame holding sample `t`.
    pub fn frame_at(&self, t: f64) -> usize {
        ((t / self.hop as f64).max(0.0) as usize).min(self.frames.len().saturating_sub(1))
    }

    /// The pitch at sample `t`, Hz (interpolated between frames), if voiced there.
    pub fn hz_at(&self, t: f64) -> Option<f64> {
        let f = t / self.hop as f64;
        let i = (f.floor().max(0.0) as usize).min(self.frames.len().saturating_sub(1));
        let j = (i + 1).min(self.frames.len().saturating_sub(1));
        match (self.frames.get(i)?.cents, self.frames.get(j)?.cents) {
            (Some(a), Some(b)) => Some(cents_to_hz(a + (b - a) * (f - i as f64).clamp(0.0, 1.0))),
            (Some(a), None) | (None, Some(a)) => Some(cents_to_hz(a)),
            _ => None,
        }
    }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Analyses a take (mono) at `fs` into frames, notes and pitch marks.
pub fn analyse(x: &[f32], fs: f64) -> Analysis {
    let hop = ((HOP_S * fs).round() as usize).max(1);
    let win = ((WINDOW_S * fs) as usize).next_power_of_two();
    let mut det = PitchDetector::new(win);
    let n_frames = x.len() / hop + 1;
    let mut buf = vec![0.0f32; win];
    let fill = |buf: &mut [f32], k: usize| {
        let c = k * hop;
        for (i, b) in buf.iter_mut().enumerate() {
            let t = c as isize - (win / 2) as isize + i as isize;
            *b = if t >= 0 && (t as usize) < x.len() {
                x[t as usize]
            } else {
                0.0
            };
        }
    };
    // The levels first: the voicing gate is relative to the take's loud level.
    let levels: Vec<f64> = (0..n_frames)
        .map(|k| {
            fill(&mut buf, k);
            let rms = (buf.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / win as f64).sqrt();
            20.0 * rms.max(1e-12).log10()
        })
        .collect();
    let gate = {
        let mut sorted: Vec<f64> = levels.iter().copied().filter(|d| *d > FLOOR_DB).collect();
        sorted.sort_by(f64::total_cmp);
        let loud = sorted
            .get(sorted.len() * 95 / 100)
            .copied()
            .unwrap_or(FLOOR_DB);
        (loud - VOICED_RANGE_DB).max(FLOOR_DB)
    };
    let mut frames: Vec<Frame> = (0..n_frames)
        .map(|k| {
            fill(&mut buf, k);
            let db = levels[k];
            let p = if db > gate {
                det.detect(&buf, fs)
            } else {
                None
            };
            match p {
                Some(p) if p.clarity >= VOICED_CLARITY && (MIN_HZ..=MAX_HZ).contains(&p.hz) => {
                    Frame {
                        cents: Some(hz_to_cents(p.hz)),
                        clarity: p.clarity,
                        db,
                    }
                }
                Some(p) => Frame {
                    cents: None,
                    clarity: p.clarity,
                    db,
                },
                None => Frame {
                    cents: None,
                    clarity: 0.0,
                    db,
                },
            }
        })
        .collect();
    clean_pitch(&mut frames);
    let notes = find_notes(&frames, hop, x, fs);
    let marks = find_marks(x, fs, hop, &frames);
    Analysis {
        fs,
        hop,
        frames,
        notes,
        marks,
    }
}

/// Octave slips folded back (a frame an octave off both neighbours), lone voiced or unvoiced
/// frames dropped or filled.
fn clean_pitch(frames: &mut [Frame]) {
    let n = frames.len();
    for i in 1..n.saturating_sub(1) {
        if let (Some(a), Some(c), Some(b)) =
            (frames[i - 1].cents, frames[i].cents, frames[i + 1].cents)
        {
            let around = 0.5 * (a + b);
            for k in [-2400.0, -1200.0, 1200.0, 2400.0] {
                if (c + k - around).abs() < 150.0 && (c - around).abs() > 900.0 {
                    frames[i].cents = Some(c + k);
                }
            }
        }
    }
    for i in 1..n.saturating_sub(1) {
        let (prev, next) = (frames[i - 1].cents, frames[i + 1].cents);
        match (prev, frames[i].cents, next) {
            (None, Some(_), None) => frames[i].cents = None,
            (Some(a), None, Some(b)) if frames[i].db > FLOOR_DB && (a - b).abs() < 100.0 => {
                frames[i].cents = Some(0.5 * (a + b))
            }
            _ => {}
        }
    }
}

fn find_notes(frames: &[Frame], hop: usize, x: &[f32], fs: f64) -> Vec<Note> {
    let n = frames.len();
    let per_s = 1.0 / (hop as f64 / fs);
    let min_len = (MIN_NOTE_S * per_s) as usize;
    let smooth = ((SMOOTH_S * per_s) as usize).max(1);
    let span = ((STEP_SPAN_S * per_s) as usize).max(1);
    // Voiced runs.
    let mut runs = Vec::new();
    let mut k = 0;
    while k < n {
        if frames[k].cents.is_none() {
            k += 1;
            continue;
        }
        let s = k;
        while k < n && frames[k].cents.is_some() {
            k += 1;
        }
        if k - s >= min_len {
            runs.push((s, k));
        }
    }
    let mut cuts_all = Vec::new();
    for &(s, e) in &runs {
        let c: Vec<f64> = (s..e).map(|i| frames[i].cents.unwrap_or(0.0)).collect();
        // Vibrato-smoothed pitch: a moving average over about one cycle.
        let sm: Vec<f64> = (0..c.len())
            .map(|i| {
                let (a, b) = (
                    i.saturating_sub(smooth / 2),
                    (i + smooth / 2 + 1).min(c.len()),
                );
                c[a..b].iter().sum::<f64>() / (b - a) as f64
            })
            .collect();
        let mut cuts = vec![s];
        // Pitch steps: where the smoothed pitch moves most over a span, past the threshold.
        let mv: Vec<f64> = (0..c.len())
            .map(|i| {
                if i >= span / 2 && i + span / 2 < c.len() {
                    (sm[i + span / 2] - sm[i - span / 2]).abs()
                } else {
                    0.0
                }
            })
            .collect();
        for i in 1..c.len().saturating_sub(1) {
            if mv[i] > STEP_CENTS && mv[i] >= mv[i - 1] && mv[i] > mv[i + 1] {
                cuts.push(s + i);
            }
        }
        // Level dips between syllables.
        let lv: Vec<f64> = (s..e).map(|i| frames[i].db).collect();
        let reach = (0.08 * per_s) as usize;
        for i in reach..lv.len().saturating_sub(reach) {
            let left = lv[i - reach..i].iter().cloned().fold(f64::MIN, f64::max);
            let right = lv[i + 1..=i + reach]
                .iter()
                .cloned()
                .fold(f64::MIN, f64::max);
            let local_min = lv[i - 1] >= lv[i] && lv[i + 1] > lv[i];
            if local_min && left - lv[i] > DIP_DB && right - lv[i] > DIP_DB {
                cuts.push(s + i);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.push(e);
        // Too short: join the neighbour.
        let mut merged = vec![cuts[0]];
        for &b in &cuts[1..] {
            if b - *merged.last().unwrap_or(&s) < min_len && b != e {
                continue;
            }
            if b == e && merged.len() > 1 && b - *merged.last().unwrap_or(&s) < min_len {
                merged.pop();
            }
            merged.push(b);
        }
        for w in merged.windows(2) {
            cuts_all.push((w[0], w[1]));
        }
    }
    cuts_all.into_iter().map(|(a, b)| make_note(frames, hop, x, fs, a, b)).collect()
}

/// The note over frames `a..b`: its centre, drift (and its slow part), level.
fn make_note(frames: &[Frame], hop: usize, x: &[f32], fs: f64, a: usize, b: usize) -> Note {
    let per_s = fs / hop as f64;
    let smooth = ((SMOOTH_S * per_s) as usize).max(1);
    let len = b - a;
    // The centre: the vibrato-smoothed pitch (a moving average over about a cycle) averaged over
    // the note's middle; a plain median of a vibrato that does not span whole cycles leans to
    // one side.
    let c: Vec<Option<f64>> = (a..b).map(|i| frames[i].cents).collect();
    let sm: Vec<f64> = (len / 5..len - len / 5)
        .filter_map(|i| {
            let (lo, hi) = (i.saturating_sub(smooth / 2), (i + smooth / 2 + 1).min(len));
            let v: Vec<f64> = c[lo..hi].iter().flatten().copied().collect();
            (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
        })
        .collect();
    let centre = if sm.is_empty() {
        median(&mut c.iter().flatten().copied().collect::<Vec<_>>())
    } else {
        sm.iter().sum::<f64>() / sm.len() as f64
    };
    let drift: Vec<f64> = c.iter().map(|c| c.map_or(0.0, |c| c - centre)).collect();
    // The slow part: the drift through two moving averages [`SLOW_S`] long (voiced frames
    // only), which take vibrato (4.5–7 Hz) down by 25 dB or more.
    let w = ((SLOW_S * per_s) as usize).max(1);
    let average = |v: &[f64], voiced: &dyn Fn(usize) -> bool| -> Vec<f64> {
        (0..len)
            .map(|i| {
                let (lo, hi) = (i.saturating_sub(w / 2), (i + w / 2 + 1).min(len));
                let k: Vec<f64> = (lo..hi).filter(|&k| voiced(k)).map(|k| v[k]).collect();
                if k.is_empty() { v[i] } else { k.iter().sum::<f64>() / k.len() as f64 }
            })
            .collect()
    };
    let once = average(&drift, &|k| c[k].is_some());
    let slow = average(&once, &|_| true);
    let (s0, s1) = (a * hop, (b * hop).min(x.len()));
    let rms = (x[s0..s1].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (s1 - s0).max(1) as f64).sqrt();
    Note { start: s0, end: s1, cents: centre, drift, slow, db: 20.0 * rms.max(1e-12).log10() }
}

/// The slow drift: the drift through two moving averages this long, s.
pub const SLOW_S: f64 = 0.22;

/// The shortest part a split leaves, s.
pub const MIN_SPLIT_S: f64 = 0.03;

impl Analysis {
    /// The notes' bounds, samples (what a state keeps of splits and merges).
    pub fn bounds(&self) -> Vec<(usize, usize)> {
        self.notes.iter().map(|n| (n.start, n.end)).collect()
    }

    /// Notes rebuilt over `bounds` (samples, sorted, apart; as [`Self::bounds`] gave them) of the
    /// take `x` this analysed. Returns false (and changes nothing) for bounds that do not fit.
    pub fn set_bounds(&mut self, x: &[f32], bounds: &[(usize, usize)]) -> bool {
        let n = self.frames.len();
        let ok = bounds.iter().all(|&(s, e)| s < e && e / self.hop <= n && s % self.hop == 0) && bounds.windows(2).all(|w| w[0].1 <= w[1].0);
        if !ok {
            return false;
        }
        let fs = self.fs;
        self.notes = bounds.iter().map(|&(s, e)| make_note(&self.frames, self.hop, x, fs, s / self.hop, e.div_ceil(self.hop).min(n))).collect();
        true
    }

    /// Splits note `i` at sample `at` (to the nearest frame) into two. Returns false if either
    /// part would be shorter than [`MIN_SPLIT_S`].
    pub fn split(&mut self, x: &[f32], i: usize, at: usize) -> bool {
        let Some(n) = self.notes.get(i) else { return false };
        let k = ((at as f64 / self.hop as f64).round() as usize) * self.hop;
        let min = (MIN_SPLIT_S * self.fs) as usize;
        if k < n.start + min || k + min > n.end {
            return false;
        }
        let mut b = self.bounds();
        let end = b[i].1;
        b[i].1 = k;
        b.insert(i + 1, (k, end));
        self.set_bounds(x, &b)
    }

    /// Joins note `i` and the next into one (across any gap between them).
    pub fn merge(&mut self, x: &[f32], i: usize) -> bool {
        if i + 1 >= self.notes.len() {
            return false;
        }
        let mut b = self.bounds();
        b[i].1 = b[i + 1].1;
        b.remove(i + 1);
        self.set_bounds(x, &b)
    }
}

/// The key the notes suggest: its tonic (0 = C) and whether minor, by correlating the notes'
/// pitch classes (weighted by length) with the Krumhansl–Kessler profiles.
pub fn detect_key(notes: &[Note]) -> (usize, bool) {
    const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let mut h = [0.0f64; 12];
    for n in notes {
        h[((n.cents / 100.0).round() as i64).rem_euclid(12) as usize] += (n.end - n.start) as f64;
    }
    let corr = |p: &[f64; 12], t: usize| {
        let (mh, mp) = (h.iter().sum::<f64>() / 12.0, p.iter().sum::<f64>() / 12.0);
        let (mut num, mut dh, mut dp) = (0.0, 0.0, 0.0);
        for k in 0..12 {
            let (a, b) = (h[(k + t) % 12] - mh, p[k] - mp);
            num += a * b;
            dh += a * a;
            dp += b * b;
        }
        num / (dh * dp).sqrt().max(1e-12)
    };
    let mut best = (f64::MIN, 0, false);
    for t in 0..12 {
        for (minor, p) in [(false, &MAJOR), (true, &MINOR)] {
            let r = corr(p, t);
            if r > best.0 {
                best = (r, t, minor);
            }
        }
    }
    (best.1, best.2)
}

/// One mark per period through the voiced stretches: the first at the stretch's first period's
/// largest peak, each next one a period on where the waveform around it best matches the waveform
/// around the last (normalised cross-correlation), so every mark sits at the same point of its
/// cycle however the peaks change shape.
fn find_marks(x: &[f32], fs: f64, hop: usize, frames: &[Frame]) -> Vec<usize> {
    let hz_at = |t: usize| {
        let f = (t / hop).min(frames.len() - 1);
        frames[f].cents.map(cents_to_hz)
    };
    let at = |k: isize| if k >= 0 && (k as usize) < x.len() { f64::from(x[k as usize]) } else { 0.0 };
    let mut marks = Vec::new();
    let mut t = 0usize;
    while t < x.len() {
        let Some(hz) = hz_at(t) else {
            t += hop;
            continue;
        };
        let p = (fs / hz) as usize;
        let first = (t..(t + p).min(x.len())).max_by(|&a, &b| x[a].total_cmp(&x[b])).unwrap_or(t);
        marks.push(first);
        let mut m = first;
        while let Some(hz) = hz_at(m) {
            let p = fs / hz;
            let half = (0.5 * p).round() as isize;
            let (lo, hi) = ((0.8 * p) as isize, (1.25 * p).ceil() as isize);
            let mi = m as isize;
            if (m as f64 + 0.8 * p) as usize >= x.len() {
                break;
            }
            let e0: f64 = (-half..=half).map(|j| at(mi + j).powi(2)).sum();
            let mut best = (f64::MIN, lo);
            for lag in lo..=hi {
                let (mut xy, mut e1) = (0.0, 0.0);
                for j in -half..=half {
                    let v = at(mi + lag + j);
                    xy += at(mi + j) * v;
                    e1 += v * v;
                }
                let r = xy / (e0 * e1).sqrt().max(1e-20);
                if r > best.0 {
                    best = (r, lag);
                }
            }
            let next = (mi + best.1) as usize;
            if next >= x.len() || hz_at(next).is_none() {
                break;
            }
            marks.push(next);
            m = next;
        }
        t = m + hop;
    }
    marks
}

/// What is done to a note. Neutral: nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEdit {
    /// The centre moved by this, cents.
    pub shift_cents: f64,
    /// How much of the slow drift (slides, sags) stays: 1 as sung, 0 none.
    pub drift: f64,
    /// Moved in time by this, samples.
    pub offset: f64,
    /// Its length times this.
    pub stretch: f64,
    /// Formants moved by this, semitones.
    pub formant_semitones: f64,
    pub gain_db: f64,
    /// How much of the modulation (vibrato, wobble) stays: 1 as sung, 0 none.
    pub modulation: f64,
}

impl Default for NoteEdit {
    fn default() -> Self {
        Self {
            modulation: 1.0,
            shift_cents: 0.0,
            drift: 1.0,
            offset: 0.0,
            stretch: 1.0,
            formant_semitones: 0.0,
            gain_db: 0.0,
        }
    }
}

impl NoteEdit {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    fn moves_time(&self) -> bool {
        self.offset != 0.0 || self.stretch != 1.0
    }
}

/// The time map: output sample → input sample, piecewise linear through each note's ends (moved
/// and stretched), the stretches between notes following.
pub struct TimeMap {
    /// (output, input) anchors, increasing in both.
    anchors: Vec<(f64, f64)>,
}

impl TimeMap {
    pub fn new(notes: &[Note], edits: &[NoteEdit], len: usize) -> Self {
        let mut anchors = vec![(0.0, 0.0)];
        for (n, e) in notes.iter().zip(edits) {
            let (s, t) = (n.start as f64, n.end as f64);
            let out_s = s + e.offset;
            let out_e = out_s + (t - s) * e.stretch;
            let last = anchors.last().copied().unwrap_or((0.0, 0.0));
            // Kept increasing: a note cannot start before the last anchor.
            let out_s = out_s.max(last.0 + 1.0);
            let out_e = out_e.max(out_s + 1.0);
            if s > last.1 {
                anchors.push((out_s, s));
                anchors.push((out_e, t));
            }
        }
        let last = anchors.last().copied().unwrap_or((0.0, 0.0));
        let tail = len as f64 - last.1;
        anchors.push((last.0 + tail.max(0.0), len as f64));
        Self { anchors }
    }

    /// The output's length, samples.
    pub fn out_len(&self) -> usize {
        self.anchors.last().map_or(0, |a| a.0.ceil() as usize)
    }

    pub fn map(&self, out: f64) -> f64 {
        let i = self
            .anchors
            .partition_point(|a| a.0 <= out)
            .clamp(1, self.anchors.len() - 1);
        let (a, b) = (self.anchors[i - 1], self.anchors[i]);
        if b.0 <= a.0 {
            return a.1;
        }
        a.1 + (out - a.0) / (b.0 - a.0) * (b.1 - a.1)
    }

    /// Whether the map is the identity at `out` (and so it can pass the input).
    fn identity_at(&self, out: f64) -> bool {
        (self.map(out) - out).abs() < 0.5 && (self.map(out + 1.0) - out - 1.0).abs() < 0.5
    }
}

/// Renders the take with `edits` (one per note), the whole of it.
pub fn render(x: &[f32], a: &Analysis, edits: &[NoteEdit]) -> Vec<f32> {
    let out_len = TimeMap::new(&a.notes, edits, x.len()).out_len();
    render_range(x, a, edits, 0..out_len)
}

/// The output samples an edit of note `i` can change (from the note before it to the note after
/// it, with the grains and crossfades around them), as the take's samples.
pub fn affected(a: &Analysis, i: usize, len: usize) -> std::ops::Range<usize> {
    let margin = affect_margin(a.fs);
    let lo = i
        .checked_sub(1)
        .and_then(|j| a.notes.get(j))
        .map_or(0, |n| n.start)
        .saturating_sub(margin);
    let hi = a
        .notes
        .get(i + 1)
        .map_or(len, |n| n.end)
        .saturating_add(margin)
        .min(len);
    lo..hi
}

fn affect_margin(fs: f64) -> usize {
    (2.0 * fs / MIN_HZ) as usize + ((FADE_S * fs) as usize) * 2
}

/// Output times the grains are laid at whatever came before (a note's start, less
/// [`affect_margin`]), so a render begun at one of them lays the same grains as the whole render.
fn sync_points(a: &Analysis) -> Vec<f64> {
    let m = affect_margin(a.fs);
    let mut s: Vec<f64> = std::iter::once(0.0)
        .chain(a.notes.iter().map(|n| n.start.saturating_sub(m) as f64))
        .collect();
    s.sort_by(f64::total_cmp);
    s.dedup();
    s
}

/// Renders output samples `range` of the take with `edits` (the rest of the output is not
/// worked out): for re-rendering only what an edit changes.
pub fn render_range(
    x: &[f32],
    a: &Analysis,
    edits: &[NoteEdit],
    range: std::ops::Range<usize>,
) -> Vec<f32> {
    let fs = a.fs;
    let map = TimeMap::new(&a.notes, edits, x.len());
    // Grains reach this far (a two-period grain, the formant down an octave): work out a little
    // more on either side, from a sync point.
    let margin = (5.0 * fs / MIN_HZ) as usize + 8;
    let sync = sync_points(a);
    let lo = sync
        .iter()
        .rev()
        .find(|&&p| p <= range.start.saturating_sub(margin) as f64)
        .copied()
        .unwrap_or(0.0) as usize;
    let hi = range.end + margin;
    let span = hi - lo;
    let mut acc = vec![0.0f64; span + 1];
    let mut wsum = vec![0.0f64; span + 1];
    // Which note (if any) an input sample belongs to (the notes are sorted and apart).
    let note_of_in = |t: f64| {
        let i = a.notes.partition_point(|n| (n.start as f64) <= t);
        i.checked_sub(1).filter(|&j| t < a.notes[j].end as f64)
    };
    let noise_len = ((NOISE_GRAIN_S * fs) as usize).max(8);
    let lay = |acc: &mut [f64], wsum: &mut [f64], o: f64, w: f64, v: f64| {
        if o >= lo as f64 && (o as usize) < hi {
            let oi = o as usize - lo;
            acc[oi] += w * v;
            wsum[oi] += w;
        }
    };
    let mut t_out = lo as f64;
    let mut next_sync = sync.partition_point(|&p| p <= t_out);
    // Whether the next voiced grain must first be put on a mark (after unvoiced audio, a sync
    // point, the start): so wherever nothing moves the grains land where they were, and the
    // output is the input.
    let mut anchor = true;
    let max_p = 2.0 * fs / MIN_HZ;
    while (t_out as usize) < hi {
        let mut t_in = map.map(t_out);
        let voiced = a.hz_at(t_in).is_some() && !a.marks.is_empty();
        if voiced && anchor {
            // The first mark from here on: the output time that maps onto it.
            let mk = a.marks.partition_point(|&m| (m as f64) < t_in);
            if let Some(&m) = a.marks.get(mk) {
                if (m as f64 - t_in) < max_p {
                    let slope = (map.map(t_out + 1.0) - t_in).max(1e-6);
                    t_out += (m as f64 - t_in) / slope;
                    t_in = map.map(t_out);
                }
            }
            anchor = false;
            if t_out as usize >= hi {
                break;
            }
        }
        let note = note_of_in(t_in);
        let e = note.map_or(NoteEdit::default(), |i| edits[i]);
        let gain = 10f64.powf(e.gain_db / 20.0);
        let mk = a.marks.partition_point(|&m| (m as f64) < t_in);
        let nearest = [mk.checked_sub(1), (mk < a.marks.len()).then_some(mk)]
            .into_iter()
            .flatten()
            .min_by(|&i, &j| ((a.marks[i] as f64) - t_in).abs().total_cmp(&((a.marks[j] as f64) - t_in).abs()));
        if let (true, Some(mi)) = (voiced, nearest) {
            let m = a.marks[mi] as f64;
            let fallback = fs / a.hz_at(t_in).unwrap_or(200.0);
            // The periods either side of the mark (the grain's halves).
            let left = mi.checked_sub(1).map(|j| m - a.marks[j] as f64).filter(|&d| d < max_p);
            let right = a.marks.get(mi + 1).map(|&r| r as f64 - m).filter(|&d| d < max_p);
            let (hl, hr) = match (left, right) {
                (Some(l), Some(r)) => (l, r),
                (Some(l), None) => (l, l),
                (None, Some(r)) => (r, r),
                (None, None) => (fallback, fallback),
            };
            // The pitch moves by the note's shift and its drift scaled: a ratio on the period.
            let ratio = match note {
                Some(i) => {
                    let n = &a.notes[i];
                    let f = ((t_in - n.start as f64) / a.hop as f64).max(0.0) as usize;
                    let d = n.drift.get(f.min(n.drift.len().saturating_sub(1))).copied().unwrap_or(0.0);
                    let sl = n.slow.get(f.min(n.slow.len().saturating_sub(1))).copied().unwrap_or(0.0);
                    2f64.powf((e.shift_cents + (e.drift - 1.0) * sl + (e.modulation - 1.0) * (d - sl)) / 1200.0)
                }
                None => 1.0,
            };
            let fr = 2f64.powf(e.formant_semitones / 12.0);
            // The grain: the input from the mark before to the mark after (Hann halves),
            // resampled by the formant ratio, centred at t_out.
            let (ol, or) = (hl / fr, hr / fr);
            for k in (-ol.ceil() as isize)..=(or.ceil() as isize) {
                let (u, src) = if k < 0 { (k as f64 / ol, m + k as f64 * fr) } else { (k as f64 / or, m + k as f64 * fr) };
                if u.abs() >= 1.0 {
                    continue;
                }
                let w = 0.5 + 0.5 * (std::f64::consts::PI * u).cos();
                lay(&mut acc, &mut wsum, t_out + k as f64, w, sample(x, src) * gain);
            }
            // The next output period: the input's (towards the next mark) over the ratio.
            t_out += hr / ratio;
        } else {
            // Unvoiced: a short grain from the matching input time.
            let half = noise_len as f64 / 2.0;
            for k in -(half as isize)..=(half as isize) {
                let u = k as f64 / half;
                if u.abs() >= 1.0 {
                    continue;
                }
                let w = 0.5 + 0.5 * (std::f64::consts::PI * u).cos();
                lay(&mut acc, &mut wsum, t_out + k as f64, w, sample(x, t_in + k as f64) * gain);
            }
            t_out += half;
            anchor = true;
        }
        // The next grain starts at a sync point rather than step over it.
        if let Some(&p) = sync.get(next_sync) {
            if t_out >= p {
                t_out = p;
                next_sync += 1;
                anchor = true;
            }
        }
    }
    // Wherever nothing is edited, the input itself, crossfaded into what is (decided over the
    // whole span, so the fades are the same however the range is cut).
    let fade = ((FADE_S * fs) as usize).max(1);
    let time_moved: Vec<(f64, f64)> = edits
        .iter()
        .zip(&a.notes)
        .filter(|(e, _)| e.moves_time())
        .map(|(_, n)| (n.start as f64 - fade as f64, n.end as f64 + fade as f64))
        .collect();
    let neutral: Vec<bool> = (lo..hi)
        .map(|o| {
            let t = o as f64;
            if !map.identity_at(t) {
                return false;
            }
            match note_of_in(t) {
                Some(i) => edits[i].is_neutral(),
                None => !time_moved.iter().any(|&(s, e)| t >= s && t < e),
            }
        })
        .collect();
    let mut edited = vec![0.0f64; span];
    let mut last_edit: Option<usize> = None;
    for o in 0..span {
        if !neutral[o] {
            last_edit = Some(o);
        }
        edited[o] = last_edit.map_or(0.0, |l| (1.0 - (o - l) as f64 / fade as f64).max(0.0));
    }
    let mut next_edit: Option<usize> = None;
    for o in (0..span).rev() {
        if !neutral[o] {
            next_edit = Some(o);
        }
        let w = next_edit.map_or(0.0, |n| (1.0 - (n - o) as f64 / fade as f64).max(0.0));
        edited[o] = edited[o].max(w);
    }
    range
        .map(|o| {
            let k = o - lo;
            let w = edited[k];
            let dry = if o < x.len() { f64::from(x[o]) } else { 0.0 };
            let wet = if wsum[k] > 1e-3 {
                acc[k] / wsum[k].max(0.5)
            } else {
                0.0
            };
            (dry * (1.0 - w) + wet * w) as f32
        })
        .collect()
}

/// `x` at a fractional position (cubic Hermite), 0 outside.
fn sample(x: &[f32], t: f64) -> f64 {
    let i = t.floor() as isize;
    let f = t - i as f64;
    let at = |k: isize| {
        if k >= 0 && (k as usize) < x.len() {
            f64::from(x[k as usize])
        } else {
            0.0
        }
    };
    let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * f + c2) * f + c1) * f + y1
}

/// A synthetic sung line for tests and listening: glottal pulses through three formants (an
/// "ah"), each note with vibrato and a breathy consonant before it. Returns the take and each
/// note's (start, end, cents) as sung.
pub mod voice {
    use super::cents_to_hz;

    /// One sung note: its pitch (cents), length, vibrato depth (cents) and rate (Hz), level (dB).
    #[derive(Debug, Clone, Copy)]
    pub struct Sung {
        pub cents: f64,
        pub seconds: f64,
        pub vibrato_cents: f64,
        pub vibrato_hz: f64,
        pub db: f64,
    }

    /// Formants of an "ah": centre (Hz) and bandwidth.
    pub const FORMANTS: [(f64, f64); 3] = [(730.0, 90.0), (1090.0, 110.0), (2440.0, 170.0)];

    struct Reson {
        a1: f64,
        a2: f64,
        g: f64,
        y1: f64,
        y2: f64,
    }

    impl Reson {
        fn new(f: f64, bw: f64, fs: f64) -> Self {
            let r = (-std::f64::consts::PI * bw / fs).exp();
            let a1 = -2.0 * r * (std::f64::consts::TAU * f / fs).cos();
            let a2 = r * r;
            Self {
                a1,
                a2,
                g: 1.0 - r,
                y1: 0.0,
                y2: 0.0,
            }
        }

        fn run(&mut self, x: f64) -> f64 {
            let y = self.g * x - self.a1 * self.y1 - self.a2 * self.y2;
            self.y2 = self.y1;
            self.y1 = y;
            y
        }
    }

    pub fn sing(notes: &[Sung], gap_s: f64, fs: f64) -> (Vec<f32>, Vec<(usize, usize, f64)>) {
        let mut x: Vec<f32> = Vec::new();
        let mut truth = Vec::new();
        let mut seed = 0x1234_5678u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f64::from(seed) / f64::from(u32::MAX) * 2.0 - 1.0
        };
        let mut res: Vec<Reson> = FORMANTS
            .iter()
            .map(|&(f, b)| Reson::new(f, b, fs))
            .collect();
        let mut phase = 0.0f64;
        for n in notes {
            // A breathy consonant (filtered noise) in the gap before the note.
            let gap = (gap_s * fs) as usize;
            for i in 0..gap {
                let env = if i > gap / 2 {
                    (1.0 - (i - gap / 2) as f64 / (gap / 2) as f64) * 0.5 + 0.5
                } else {
                    i as f64 / (gap / 2) as f64
                };
                let v = noise() * 0.02 * env * if i > gap / 3 { 1.0 } else { 0.0 };
                x.push(v as f32);
            }
            let len = (n.seconds * fs) as usize;
            let start = x.len();
            let amp = 10f64.powf(n.db / 20.0);
            for i in 0..len {
                let t = i as f64 / fs;
                let c =
                    n.cents + n.vibrato_cents * (std::f64::consts::TAU * n.vibrato_hz * t).sin();
                phase += cents_to_hz(c) / fs;
                // A glottal pulse once a period: a smooth bump, then the formants ring.
                let p = phase.fract();
                let pulse = if p < 0.4 {
                    (std::f64::consts::PI * p / 0.4).sin().powi(2)
                } else {
                    0.0
                };
                let excite = pulse - 0.4 * 0.5;
                let mut y = 0.0;
                for r in &mut res {
                    y += r.run(excite);
                }
                let env =
                    (i as f64 / (0.02 * fs)).min(1.0) * ((len - i) as f64 / (0.03 * fs)).min(1.0);
                x.push((y * amp * env * 4.0 + 0.002 * noise()) as f32);
            }
            truth.push((start, start + len, n.cents));
        }
        x.extend(std::iter::repeat_n(0.0, (0.2 * fs) as usize));
        (x, truth)
    }
}

#[cfg(test)]
mod tests {
    use super::voice::{Sung, sing};
    use super::*;

    const FS: f64 = 48_000.0;

    fn line() -> (Vec<f32>, Vec<(usize, usize, f64)>) {
        let n = |cents: f64, seconds: f64| Sung {
            cents,
            seconds,
            vibrato_cents: 35.0,
            vibrato_hz: 5.5,
            db: -12.0,
        };
        sing(
            &[
                n(5700.0, 0.5),
                n(5940.0, 0.4),
                n(6200.0, 0.6),
                n(5900.0, 0.45),
            ],
            0.08,
            FS,
        )
    }

    /// The pitch spread (standard deviation, cents) over a note's steady middle in a take.
    fn spread(x: &[f32], s: usize, e: usize) -> (f64, f64) {
        let a = analyse(x, FS);
        let (f0, f1) = (
            s / a.hop + (e - s) / a.hop / 4,
            e / a.hop - (e - s) / a.hop / 4,
        );
        let c: Vec<f64> = (f0..f1)
            .filter_map(|f| a.frames.get(f).and_then(|fr| fr.cents))
            .collect();
        let m = c.iter().sum::<f64>() / c.len().max(1) as f64;
        (
            m,
            (c.iter().map(|v| (v - m).powi(2)).sum::<f64>() / c.len().max(1) as f64).sqrt(),
        )
    }

    #[test]
    fn analysis_finds_the_notes_of_a_sung_line() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        assert_eq!(
            a.notes.len(),
            truth.len(),
            "{:?}",
            a.notes
                .iter()
                .map(|n| (n.start, n.end, n.cents as i64))
                .collect::<Vec<_>>()
        );
        for (n, (s, e, c)) in a.notes.iter().zip(&truth) {
            assert!(
                (n.start as f64 - *s as f64).abs() < 0.015 * FS,
                "start {} vs {s}",
                n.start
            );
            assert!(
                (n.end as f64 - *e as f64).abs() < 0.03 * FS,
                "end {} vs {e}",
                n.end
            );
            assert!((n.cents - c).abs() < 5.0, "centre {:.1} vs {c}", n.cents);
        }
        // Marks a period apart through the first note.
        let p = FS / cents_to_hz(5700.0);
        let marks: Vec<usize> = a
            .marks
            .iter()
            .copied()
            .filter(|&m| m > truth[0].0 + 2000 && m < truth[0].1 - 2000)
            .collect();
        for w in marks.windows(2) {
            let d = (w[1] - w[0]) as f64;
            assert!(
                (d / p - 1.0).abs() < 0.08,
                "mark spacing {d} vs period {p:.1}"
            );
        }
    }

    #[test]
    fn nothing_edited_is_the_input_exactly() {
        let (x, _) = line();
        let a = analyse(&x, FS);
        let y = render(&x, &a, &vec![NoteEdit::default(); a.notes.len()]);
        assert_eq!(y, x);
    }

    #[test]
    fn a_pitch_edit_lands_and_keeps_the_formants() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[2].shift_cents = 300.0;
        let y = render(&x, &a, &edits);
        let (s, e, _) = truth[2];
        let ((was, _), (got, _)) = (spread(&x, s, e), spread(&y, s, e));
        assert!(
            (got - was - 300.0).abs() < 3.0,
            "moved {:.1} cents",
            got - was
        );
        // The formants stay: the spectral centroid moves under 6 % (a resampling shift of three
        // semitones moves it 19 %).
        let (was, now) = (
            centroid(&x[s + 4800..e - 4800]),
            centroid(&y[s + 4800..e - 4800]),
        );
        assert!(
            (now / was - 1.0).abs() < 0.06,
            "centroid {was:.0} → {now:.0} Hz"
        );
        // Other notes untouched.
        assert_eq!(&y[..truth[1].1], &x[..truth[1].1]);
    }

    #[test]
    fn modulation_flattens_or_doubles_the_vibrato_and_drift_leaves_it() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        let (s, e, _) = truth[0];
        let (_, sung) = spread(&x, s, e);
        for (amount, lo, hi) in [(0.0, 0.0, 6.0), (2.0, 1.7 * sung, 2.3 * sung)] {
            let mut edits = vec![NoteEdit::default(); a.notes.len()];
            edits[0].modulation = amount;
            let (_, sd) = spread(&render(&x, &a, &edits), s, e);
            assert!((lo..hi).contains(&sd), "modulation {amount}: spread {sd:.1} (sung {sung:.1})");
        }
        // The line sings no slides: without its slow drift the vibrato stays.
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[0].drift = 0.0;
        let (_, sd) = spread(&render(&x, &a, &edits), s, e);
        assert!((0.8 * sung..1.2 * sung).contains(&sd), "drift 0: spread {sd:.1} (sung {sung:.1})");
    }

    #[test]
    fn split_and_merge_rebuild_the_notes() {
        let (x, _) = line();
        let mut a = analyse(&x, FS);
        let before = a.notes.clone();
        let n = &before[2];
        let mid = (n.start + n.end) / 2;
        assert!(a.split(&x, 2, mid));
        assert_eq!(a.notes.len(), before.len() + 1);
        assert_eq!((a.notes[2].start, a.notes[3].end), (n.start, n.end));
        // Both halves sing the same note.
        assert!((a.notes[2].cents - n.cents).abs() < 15.0 && (a.notes[3].cents - n.cents).abs() < 15.0);
        // Too near an end: refused.
        assert!(!a.split(&x, 2, a.notes[2].start + 10));
        assert!(a.merge(&x, 2));
        assert_eq!(a.notes, before);
        // Bounds round-trip.
        let b = a.bounds();
        let mut c = analyse(&x, FS);
        assert!(c.set_bounds(&x, &b));
        assert_eq!(c.notes, a.notes);
        assert!(!c.set_bounds(&x, &[(100, 50)]));
    }

    #[test]
    fn the_key_is_found() {
        // A C major scale up and down, the tonic held longest.
        let n = |cents: f64, seconds: f64| Sung { cents, seconds, vibrato_cents: 10.0, vibrato_hz: 5.5, db: -12.0 };
        let steps = [0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1100.0, 1200.0, 700.0, 400.0, 0.0];
        let line: Vec<Sung> = steps.iter().map(|&c| n(6000.0 + c, if c == 0.0 || c == 1200.0 { 0.6 } else { 0.25 })).collect();
        let (x, _) = sing(&line, 0.06, FS);
        let a = analyse(&x, FS);
        assert_eq!(detect_key(&a.notes), (0, false));
        // A minor: the same notes, A held.
        let steps = [900.0, 1100.0, 1200.0, 1400.0, 1600.0, 1700.0, 1900.0, 2100.0, 1600.0, 900.0];
        let line: Vec<Sung> = steps.iter().map(|&c| n(5700.0 + c - 900.0, if c == 900.0 || c == 2100.0 { 0.6 } else { 0.25 })).collect();
        let (x, _) = sing(&line, 0.06, FS);
        assert_eq!(detect_key(&analyse(&x, FS).notes), (9, true));
    }

    #[test]
    fn timing_and_length_move_and_stretch_a_note() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[1].offset = 0.03 * FS;
        edits[1].stretch = 1.4;
        let y = render(&x, &a, &edits);
        let b = analyse(&y, FS);
        let n = b
            .notes
            .iter()
            .min_by(|p, q| {
                (p.cents - truth[1].2)
                    .abs()
                    .total_cmp(&(q.cents - truth[1].2).abs())
            })
            .expect("the note");
        let (s, e, c) = truth[1];
        assert!((n.cents - c).abs() < 5.0);
        assert!(
            (n.start as f64 - (s as f64 + 0.03 * FS)).abs() < 0.02 * FS,
            "start {} vs {}",
            n.start,
            s as f64 + 0.03 * FS
        );
        let want = (e - s) as f64 * 1.4;
        assert!(
            ((n.end - n.start) as f64 / want - 1.0).abs() < 0.12,
            "length {} vs {want}",
            n.end - n.start
        );
    }

    #[test]
    fn formant_and_level_edits() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        let (s, e, _) = truth[2];
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[2].formant_semitones = 3.0;
        let y = render(&x, &a, &edits);
        // The formants move up (the centroid with them, held back by the harmonics that stay).
        let (was, now) = (
            centroid(&x[s + 4800..e - 4800]),
            centroid(&y[s + 4800..e - 4800]),
        );
        assert!(now > was, "centroid {was:.0} → {now:.0} Hz");
        // In order from −6 to +6 semitones, ±6 by more than 10 % (the synthetic voice's
        // resonators against its harmonics make single steps uneven; a real vocal moves −15,
        // −10, +13, +31 %: examples/melody_quality).
        let at = |f: f64| {
            let mut ed = vec![NoteEdit::default(); a.notes.len()];
            ed[2].formant_semitones = f;
            centroid(&render(&x, &a, &ed)[s + 4800..e - 4800])
        };
        let c: Vec<f64> = [-6.0, -3.0, 0.0, 3.0, 6.0].into_iter().map(at).collect();
        assert!(c.windows(2).all(|w| w[1] > w[0]), "{c:?}");
        assert!(c[0] / was < 0.9 && c[4] / was > 1.1, "{c:?}");
        let ((was, _), (got, _)) = (spread(&x, s, e), spread(&y, s, e));
        assert!(
            (got - was).abs() < 3.0,
            "the pitch stays: {got:.1} vs {was:.1}"
        );
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[2].gain_db = -6.0;
        let y = render(&x, &a, &edits);
        let rms = |v: &[f32]| {
            (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let d = 20.0 * (rms(&y[s + 4800..e - 4800]) / rms(&x[s + 4800..e - 4800])).log10();
        assert!((d + 6.0).abs() < 0.3, "{d:.2} dB");
    }

    #[test]
    fn a_range_renders_what_the_whole_does_there() {
        let (x, _) = line();
        let a = analyse(&x, FS);
        for i in 1..a.notes.len() {
            let mut edits = vec![NoteEdit::default(); a.notes.len()];
            edits[i].shift_cents = if i % 2 == 0 { -150.0 } else { 150.0 };
            edits[i].drift = 0.5;
            edits[i].formant_semitones = -3.0;
            let whole = render(&x, &a, &edits);
            let r = affected(&a, i, x.len());
            let part = render_range(&x, &a, &edits, r.clone());
            // From a sync point the same grains: the same, sample for sample.
            let worst = part
                .iter()
                .zip(&whole[r.clone()])
                .map(|(p, w)| (p - w).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-4, "{worst}");
            // Outside the affected range nothing changed.
            assert_eq!(&whole[..r.start], &x[..r.start]);
            assert_eq!(&whole[r.end..x.len()], &x[r.end..]);
        }
    }

    #[test]
    fn edited_notes_meet_the_input_without_clicks() {
        let (x, _) = line();
        let a = analyse(&x, FS);
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[1].shift_cents = -200.0;
        edits[3].gain_db = 4.0;
        let y = render(&x, &a, &edits);
        let d2 = |v: &[f32]| {
            v.windows(3)
                .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        assert!(d2(&y) < 2.5 * d2(&x), "{} vs {}", d2(&y), d2(&x));
    }

    /// The spectral centroid between 200 Hz and 4 kHz, Hz: where the formants put the energy.
    fn centroid(v: &[f32]) -> f64 {
        let n = 16_384.min(v.len().next_power_of_two() / 2);
        let mut an = crate::analyzer::Analyzer::new(n);
        let mut p = vec![0.0f32; an.bins()];
        an.analyze_power(&v[..n], &mut p);
        let bin = FS / n as f64;
        let (mut num, mut den) = (0.0, 0.0);
        for (k, v) in p.iter().enumerate() {
            let f = k as f64 * bin;
            if (200.0..4000.0).contains(&f) {
                num += f * f64::from(*v);
                den += f64::from(*v);
            }
        }
        num / den
    }
}
