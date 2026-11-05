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
            *b = if t >= 0 && (t as usize) < x.len() { x[t as usize] } else { 0.0 };
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
        let loud = sorted.get(sorted.len() * 95 / 100).copied().unwrap_or(FLOOR_DB);
        (loud - VOICED_RANGE_DB).max(FLOOR_DB)
    };
    let mut frames: Vec<Frame> = (0..n_frames)
        .map(|k| {
            fill(&mut buf, k);
            let db = levels[k];
            let p = if db > gate { det.detect(&buf, fs) } else { None };
            match p {
                Some(p) if p.clarity >= VOICED_CLARITY && (MIN_HZ..=MAX_HZ).contains(&p.hz) => Frame { cents: Some(hz_to_cents(p.hz)), clarity: p.clarity, db },
                Some(p) => Frame { cents: None, clarity: p.clarity, db },
                None => Frame { cents: None, clarity: 0.0, db },
            }
        })
        .collect();
    clean_pitch(&mut frames);
    let notes = find_notes(&frames, hop, x, fs);
    let marks = find_marks(x, fs, hop, &frames);
    Analysis { fs, hop, frames, notes, marks }
}

/// Octave slips folded back (a frame an octave off both neighbours), lone voiced or unvoiced
/// frames dropped or filled.
fn clean_pitch(frames: &mut [Frame]) {
    let n = frames.len();
    for i in 1..n.saturating_sub(1) {
        if let (Some(a), Some(c), Some(b)) = (frames[i - 1].cents, frames[i].cents, frames[i + 1].cents) {
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
            (Some(a), None, Some(b)) if frames[i].db > FLOOR_DB && (a - b).abs() < 100.0 => frames[i].cents = Some(0.5 * (a + b)),
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
                let (a, b) = (i.saturating_sub(smooth / 2), (i + smooth / 2 + 1).min(c.len()));
                c[a..b].iter().sum::<f64>() / (b - a) as f64
            })
            .collect();
        let mut cuts = vec![s];
        // Pitch steps: where the smoothed pitch moves most over a span, past the threshold.
        let mv: Vec<f64> = (0..c.len()).map(|i| if i >= span / 2 && i + span / 2 < c.len() { (sm[i + span / 2] - sm[i - span / 2]).abs() } else { 0.0 }).collect();
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
            let right = lv[i + 1..=i + reach].iter().cloned().fold(f64::MIN, f64::max);
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
    cuts_all
        .into_iter()
        .map(|(a, b)| {
            let len = b - a;
            // The centre: the vibrato-smoothed pitch (a moving average over about a cycle)
            // averaged over the note's middle; a plain median of a vibrato that does not span
            // whole cycles leans to one side.
            let c: Vec<Option<f64>> = (a..b).map(|i| frames[i].cents).collect();
            let sm: Vec<f64> = (len / 5..len - len / 5)
                .filter_map(|i| {
                    let (lo, hi) = (i.saturating_sub(smooth / 2), (i + smooth / 2 + 1).min(len));
                    let v: Vec<f64> = c[lo..hi].iter().flatten().copied().collect();
                    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
                })
                .collect();
            let centre = if sm.is_empty() { median(&mut c.iter().flatten().copied().collect::<Vec<_>>()) } else { sm.iter().sum::<f64>() / sm.len() as f64 };
            let drift: Vec<f64> = (a..b).map(|i| frames[i].cents.map_or(0.0, |c| c - centre)).collect();
            let (s0, s1) = (a * hop, (b * hop).min(x.len()));
            let rms = (x[s0..s1].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (s1 - s0).max(1) as f64).sqrt();
            Note { start: s0, end: s1, cents: centre, drift, db: 20.0 * rms.max(1e-12).log10() }
        })
        .collect()
}

/// One mark per period through the voiced stretches, at the waveform's largest positive peak
/// near where the pitch says the period starts.
fn find_marks(x: &[f32], fs: f64, hop: usize, frames: &[Frame]) -> Vec<usize> {
    let hz_at = |t: usize| {
        let f = (t / hop).min(frames.len() - 1);
        frames[f].cents.map(cents_to_hz)
    };
    let mut marks = Vec::new();
    let mut t = 0usize;
    while t < x.len() {
        let Some(hz) = hz_at(t) else {
            t += hop;
            continue;
        };
        // A voiced stretch begins: the first mark at the peak of its first period.
        let p = (fs / hz) as usize;
        let first = (t..(t + p).min(x.len())).max_by(|&a, &b| x[a].total_cmp(&x[b])).unwrap_or(t);
        marks.push(first);
        let mut m = first;
        while let Some(hz) = hz_at(m) {
            let p = fs / hz;
            let guess = m as f64 + p;
            let (lo, hi) = ((guess - 0.3 * p) as usize, ((guess + 0.3 * p) as usize).min(x.len().saturating_sub(1)));
            if lo >= x.len() || lo > hi || hz_at(lo).is_none() {
                break;
            }
            let next = (lo..=hi).max_by(|&a, &b| x[a].total_cmp(&x[b])).unwrap_or(guess as usize);
            if next <= m {
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
    /// How much of the drift (vibrato, slides) stays: 1 as sung, 0 flat.
    pub drift: f64,
    /// Moved in time by this, samples.
    pub offset: f64,
    /// Its length times this.
    pub stretch: f64,
    /// Formants moved by this, semitones.
    pub formant_semitones: f64,
    pub gain_db: f64,
}

impl Default for NoteEdit {
    fn default() -> Self {
        Self { shift_cents: 0.0, drift: 1.0, offset: 0.0, stretch: 1.0, formant_semitones: 0.0, gain_db: 0.0 }
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
        let i = self.anchors.partition_point(|a| a.0 <= out).clamp(1, self.anchors.len() - 1);
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

/// Renders the take with `edits` (one per note).
pub fn render(x: &[f32], a: &Analysis, edits: &[NoteEdit]) -> Vec<f32> {
    let fs = a.fs;
    let map = TimeMap::new(&a.notes, edits, x.len());
    let out_len = map.out_len();
    let mut acc = vec![0.0f64; out_len + 1];
    let mut wsum = vec![0.0f64; out_len + 1];
    // Which note (if any) an output sample belongs to, by the input time it maps to.
    let note_of_in = |t: f64| a.notes.iter().position(|n| t >= n.start as f64 && t < n.end as f64);
    let noise_len = ((NOISE_GRAIN_S * fs) as usize).max(8);
    let mut t_out = 0.0f64;
    while (t_out as usize) < out_len {
        let t_in = map.map(t_out);
        let note = note_of_in(t_in);
        let e = note.map_or(NoteEdit::default(), |i| edits[i]);
        let gain = 10f64.powf(e.gain_db / 20.0);
        // The input period here and the mark to cut at, when voiced.
        let voiced = a.hz_at(t_in).is_some();
        let mk = a.marks.partition_point(|&m| (m as f64) < t_in);
        let nearest = [mk.checked_sub(1), (mk < a.marks.len()).then_some(mk)].into_iter().flatten().min_by(|&i, &j| ((a.marks[i] as f64) - t_in).abs().total_cmp(&((a.marks[j] as f64) - t_in).abs()));
        if let (true, Some(mi)) = (voiced, nearest) {
            let m = a.marks[mi];
            let p_in = match (mi.checked_sub(1).map(|j| a.marks[j]), a.marks.get(mi + 1).copied()) {
                (Some(l), Some(r)) if r - l < 4 * (fs / MIN_HZ) as usize => 0.5 * (r - l) as f64,
                (Some(l), _) if m - l < 2 * (fs / MIN_HZ) as usize => (m - l) as f64,
                (_, Some(r)) if r - m < 2 * (fs / MIN_HZ) as usize => (r - m) as f64,
                _ => fs / a.hz_at(t_in).unwrap_or(200.0),
            };
            // The new pitch: the note's new centre plus its drift (scaled), else as it was.
            let hz = match note {
                Some(i) => {
                    let n = &a.notes[i];
                    let f = ((t_in - n.start as f64) / a.hop as f64).max(0.0) as usize;
                    let d = n.drift.get(f.min(n.drift.len().saturating_sub(1))).copied().unwrap_or(0.0);
                    cents_to_hz(n.cents + e.shift_cents + e.drift * d)
                }
                None => a.hz_at(t_in).unwrap_or(200.0),
            };
            let p_out = fs / hz;
            let ratio = 2f64.powf(e.formant_semitones / 12.0);
            // A two-period grain around the mark, resampled by the formant ratio, laid at t_out.
            let half_in = p_in;
            let half_out = half_in / ratio;
            let n = (2.0 * half_out).ceil() as isize;
            for k in -n / 2..=n / 2 {
                let o = t_out + k as f64;
                if o < 0.0 || o as usize >= out_len {
                    continue;
                }
                let u = k as f64 / half_out; // −1…1
                if u.abs() >= 1.0 {
                    continue;
                }
                let w = 0.5 + 0.5 * (std::f64::consts::PI * u).cos();
                let src = m as f64 + u * half_in;
                let v = sample(x, src);
                let oi = o as usize;
                acc[oi] += w * v * gain;
                wsum[oi] += w;
            }
            t_out += p_out;
        } else {
            // Unvoiced: a short grain from the matching input time.
            let half = noise_len as f64 / 2.0;
            for k in -(half as isize)..=(half as isize) {
                let o = t_out + k as f64;
                if o < 0.0 || o as usize >= out_len {
                    continue;
                }
                let u = k as f64 / half;
                if u.abs() >= 1.0 {
                    continue;
                }
                let w = 0.5 + 0.5 * (std::f64::consts::PI * u).cos();
                let v = sample(x, t_in + k as f64);
                let oi = o as usize;
                acc[oi] += w * v * gain;
                wsum[oi] += w;
            }
            t_out += half;
        }
    }
    let rendered: Vec<f64> = acc.iter().zip(&wsum).take(out_len).map(|(a, w)| if *w > 1e-3 { a / w.max(0.5) } else { 0.0 }).collect();
    // Wherever nothing is edited, the input itself, crossfaded into what is.
    let fade = ((FADE_S * fs) as usize).max(1);
    let neutral: Vec<bool> = (0..out_len)
        .map(|o| {
            if !map.identity_at(o as f64) {
                return false;
            }
            match note_of_in(o as f64) {
                Some(i) => edits[i].is_neutral(),
                None => !edits.iter().zip(&a.notes).any(|(e, n)| e.moves_time() && (o as f64) >= n.start as f64 - fade as f64 && (o as f64) < n.end as f64 + fade as f64),
            }
        })
        .collect();
    // The crossfade weight: 1 where edited, ramping over `fade` samples either side.
    let mut edited = vec![0.0f64; out_len];
    let mut last_edit: Option<usize> = None;
    for o in 0..out_len {
        if !neutral[o] {
            last_edit = Some(o);
        }
        edited[o] = last_edit.map_or(0.0, |l| (1.0 - (o - l) as f64 / fade as f64).max(0.0));
    }
    let mut next_edit: Option<usize> = None;
    for o in (0..out_len).rev() {
        if !neutral[o] {
            next_edit = Some(o);
        }
        let w = next_edit.map_or(0.0, |n| (1.0 - (n - o) as f64 / fade as f64).max(0.0));
        edited[o] = edited[o].max(w);
    }
    (0..out_len)
        .map(|o| {
            let w = edited[o];
            let dry = if o < x.len() { f64::from(x[o]) } else { 0.0 };
            (dry * (1.0 - w) + rendered[o] * w) as f32
        })
        .collect()
}

/// `x` at a fractional position (cubic Hermite), 0 outside.
fn sample(x: &[f32], t: f64) -> f64 {
    let i = t.floor() as isize;
    let f = t - i as f64;
    let at = |k: isize| if k >= 0 && (k as usize) < x.len() { f64::from(x[k as usize]) } else { 0.0 };
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
            Self { a1, a2, g: 1.0 - r, y1: 0.0, y2: 0.0 }
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
        let mut res: Vec<Reson> = FORMANTS.iter().map(|&(f, b)| Reson::new(f, b, fs)).collect();
        let mut phase = 0.0f64;
        for n in notes {
            // A breathy consonant (filtered noise) in the gap before the note.
            let gap = (gap_s * fs) as usize;
            for i in 0..gap {
                let env = if i > gap / 2 { (1.0 - (i - gap / 2) as f64 / (gap / 2) as f64) * 0.5 + 0.5 } else { i as f64 / (gap / 2) as f64 };
                let v = noise() * 0.02 * env * if i > gap / 3 { 1.0 } else { 0.0 };
                x.push(v as f32);
            }
            let len = (n.seconds * fs) as usize;
            let start = x.len();
            let amp = 10f64.powf(n.db / 20.0);
            for i in 0..len {
                let t = i as f64 / fs;
                let c = n.cents + n.vibrato_cents * (std::f64::consts::TAU * n.vibrato_hz * t).sin();
                phase += cents_to_hz(c) / fs;
                // A glottal pulse once a period: a smooth bump, then the formants ring.
                let p = phase.fract();
                let pulse = if p < 0.4 { (std::f64::consts::PI * p / 0.4).sin().powi(2) } else { 0.0 };
                let excite = pulse - 0.4 * 0.5;
                let mut y = 0.0;
                for r in &mut res {
                    y += r.run(excite);
                }
                let env = (i as f64 / (0.02 * fs)).min(1.0) * ((len - i) as f64 / (0.03 * fs)).min(1.0);
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
    use super::voice::{sing, Sung};
    use super::*;

    const FS: f64 = 48_000.0;

    fn line() -> (Vec<f32>, Vec<(usize, usize, f64)>) {
        let n = |cents: f64, seconds: f64| Sung { cents, seconds, vibrato_cents: 35.0, vibrato_hz: 5.5, db: -12.0 };
        sing(&[n(5700.0, 0.5), n(5940.0, 0.4), n(6200.0, 0.6), n(5900.0, 0.45)], 0.08, FS)
    }

    /// The pitch spread (standard deviation, cents) over a note's steady middle in a take.
    fn spread(x: &[f32], s: usize, e: usize) -> (f64, f64) {
        let a = analyse(x, FS);
        let (f0, f1) = (s / a.hop + (e - s) / a.hop / 4, e / a.hop - (e - s) / a.hop / 4);
        let c: Vec<f64> = (f0..f1).filter_map(|f| a.frames.get(f).and_then(|fr| fr.cents)).collect();
        let m = c.iter().sum::<f64>() / c.len().max(1) as f64;
        (m, (c.iter().map(|v| (v - m).powi(2)).sum::<f64>() / c.len().max(1) as f64).sqrt())
    }

    #[test]
    fn analysis_finds_the_notes_of_a_sung_line() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        assert_eq!(a.notes.len(), truth.len(), "{:?}", a.notes.iter().map(|n| (n.start, n.end, n.cents as i64)).collect::<Vec<_>>());
        for (n, (s, e, c)) in a.notes.iter().zip(&truth) {
            assert!((n.start as f64 - *s as f64).abs() < 0.015 * FS, "start {} vs {s}", n.start);
            assert!((n.end as f64 - *e as f64).abs() < 0.03 * FS, "end {} vs {e}", n.end);
            assert!((n.cents - c).abs() < 5.0, "centre {:.1} vs {c}", n.cents);
        }
        // Marks a period apart through the first note.
        let p = FS / cents_to_hz(5700.0);
        let marks: Vec<usize> = a.marks.iter().copied().filter(|&m| m > truth[0].0 + 2000 && m < truth[0].1 - 2000).collect();
        for w in marks.windows(2) {
            let d = (w[1] - w[0]) as f64;
            assert!((d / p - 1.0).abs() < 0.08, "mark spacing {d} vs period {p:.1}");
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
        assert!((got - was - 300.0).abs() < 3.0, "moved {:.1} cents", got - was);
        // The formants stay: the spectral centroid moves under 6 % (a resampling shift of three
        // semitones moves it 19 %).
        let (was, now) = (centroid(&x[s + 4800..e - 4800]), centroid(&y[s + 4800..e - 4800]));
        assert!((now / was - 1.0).abs() < 0.06, "centroid {was:.0} → {now:.0} Hz");
        // Other notes untouched.
        assert_eq!(&y[..truth[1].1], &x[..truth[1].1]);
    }

    #[test]
    fn drift_flattens_or_doubles_the_vibrato() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        let (s, e, _) = truth[0];
        let (_, sung) = spread(&x, s, e);
        for (amount, lo, hi) in [(0.0, 0.0, 4.0), (2.0, 1.7 * sung, 2.3 * sung)] {
            let mut edits = vec![NoteEdit::default(); a.notes.len()];
            edits[0].drift = amount;
            let (_, sd) = spread(&render(&x, &a, &edits), s, e);
            assert!((lo..hi).contains(&sd), "drift {amount}: spread {sd:.1} (sung {sung:.1})");
        }
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
        let n = b.notes.iter().min_by(|p, q| (p.cents - truth[1].2).abs().total_cmp(&(q.cents - truth[1].2).abs())).expect("the note");
        let (s, e, c) = truth[1];
        assert!((n.cents - c).abs() < 5.0);
        assert!((n.start as f64 - (s as f64 + 0.03 * FS)).abs() < 0.02 * FS, "start {} vs {}", n.start, s as f64 + 0.03 * FS);
        let want = (e - s) as f64 * 1.4;
        assert!(((n.end - n.start) as f64 / want - 1.0).abs() < 0.12, "length {} vs {want}", n.end - n.start);
    }

    #[test]
    fn formant_and_level_edits() {
        let (x, truth) = line();
        let a = analyse(&x, FS);
        let (s, e, c) = truth[2];
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[2].formant_semitones = 3.0;
        let y = render(&x, &a, &edits);
        // The formants move up (the centroid with them, held back by the harmonics that stay).
        let (was, now) = (centroid(&x[s + 4800..e - 4800]), centroid(&y[s + 4800..e - 4800]));
        assert!(now / was > 1.1, "centroid {was:.0} → {now:.0} Hz");
        let mut down = vec![NoteEdit::default(); a.notes.len()];
        down[2].formant_semitones = -3.0;
        let z = render(&x, &a, &down);
        assert!(centroid(&z[s + 4800..e - 4800]) / was < 0.9);
        let ((was, _), (got, _)) = (spread(&x, s, e), spread(&y, s, e));
        assert!((got - was).abs() < 3.0, "the pitch stays: {got:.1} vs {was:.1}");
        let _ = c;
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[2].gain_db = -6.0;
        let y = render(&x, &a, &edits);
        let rms = |v: &[f32]| (v.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        let d = 20.0 * (rms(&y[s + 4800..e - 4800]) / rms(&x[s + 4800..e - 4800])).log10();
        assert!((d + 6.0).abs() < 0.3, "{d:.2} dB");
    }

    #[test]
    fn edited_notes_meet_the_input_without_clicks() {
        let (x, _) = line();
        let a = analyse(&x, FS);
        let mut edits = vec![NoteEdit::default(); a.notes.len()];
        edits[1].shift_cents = -200.0;
        edits[3].gain_db = 4.0;
        let y = render(&x, &a, &edits);
        let d2 = |v: &[f32]| v.windows(3).map(|w| (w[2] - 2.0 * w[1] + w[0]).abs()).fold(0.0f32, f32::max);
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
