//! Mite Pitch's take and everything done to it, off the audio thread (docs/design/pitch-brief.md
//! §1): the transfer's recording, the take's file, the analysis, the edits, and renders handed to
//! the audio thread. One worker thread per instance does the heavy work (writing and reading the
//! file, analysing, rendering), so the editor never waits and playback works without the editor
//! after a project reload.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use mite_core::Handover;
use crate::grid::Grid;
use mite_dsp::resample::SincTable;
use mite_dsp::melody::{affected, analyse, detect_key, render, render_range, Analysis, NoteEdit};

/// The waveform lane's resolution: one low and high per this many samples.
pub const PEAK_HOP: usize = 128;

/// The longest take a transfer records, s.
pub const MAX_TAKE_S: f64 = 600.0;

/// The transfer's recording: filled by the audio thread, read once it has stopped. Atomics only,
/// so neither side ever locks.
pub struct RecordBuffer {
    data: Box<[AtomicU32]>,
    len: AtomicUsize,
    /// The timeline sample of its first sample (set by the audio thread when it starts).
    start: AtomicI64,
    started: AtomicBool,
    pub sample_rate: f32,
}

impl RecordBuffer {
    /// Allocates (on the UI thread, when the transfer is armed).
    pub fn new(sample_rate: f32) -> Self {
        let cap = (MAX_TAKE_S * f64::from(sample_rate)) as usize;
        Self {
            data: (0..cap).map(|_| AtomicU32::new(0)).collect(),
            len: AtomicUsize::new(0),
            start: AtomicI64::new(0),
            started: AtomicBool::new(false),
            sample_rate,
        }
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn start(&self) -> i64 {
        self.start.load(Ordering::Acquire)
    }

    /// Audio thread: appends `samples` heard at timeline sample `pos`. Returns false when the
    /// recording cannot go on (the transport jumped, or it is full).
    pub fn push(&self, pos: i64, samples: impl Iterator<Item = f32>) -> bool {
        let len = self.len.load(Ordering::Relaxed);
        if !self.started.load(Ordering::Relaxed) {
            self.start.store(pos, Ordering::Release);
            self.started.store(true, Ordering::Release);
        } else if pos != self.start.load(Ordering::Relaxed) + len as i64 {
            return false;
        }
        let mut k = len;
        for v in samples {
            if k >= self.data.len() {
                self.len.store(k, Ordering::Release);
                return false;
            }
            self.data[k].store(v.to_bits(), Ordering::Relaxed);
            k += 1;
        }
        self.len.store(k, Ordering::Release);
        true
    }

    fn read(&self) -> Vec<f32> {
        self.data[..self.len()]
            .iter()
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .collect()
    }
}

/// The take: its file's id, where it sits on the timeline, its rate and length.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeInfo {
    pub id: String,
    pub start: i64,
    pub sample_rate: f64,
    pub len: usize,
}

/// The rendered take, handed to the audio thread.
pub struct Rendered {
    /// What plays where on the host's timeline.
    pub segments: Vec<Segment>,
    /// Bound to an ARA document: the regions replace the input (silence elsewhere).
    pub ara: bool,
    /// The take as rendered, for the plugin's own transport (transfer only).
    pub take: Option<Arc<Vec<f32>>>,
}

/// A stretch of rendered audio on the host's timeline: from timeline sample `start`, `len`
/// timeline samples, reading `audio` from its sample `offset`, `rate` of its samples per timeline
/// sample (a clip of another sample rate than the host's, ARA), through `sinc` when that is not 1.
pub struct Segment {
    pub start: i64,
    pub offset: usize,
    pub len: usize,
    pub audio: Arc<Vec<f32>>,
    pub rate: f64,
    pub sinc: Option<Arc<SincTable>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No take yet.
    Empty,
    /// Waiting for the transport to play.
    Armed,
    Recording,
    /// Analysing the take (or reading it back).
    Analysing,
    Ready,
    /// The take's file could not be read (or written).
    Problem(String),
}

/// Another clip on the editor's track (bound to ARA): drawn beside the current one, in its
/// place on the song; a click makes it the current one.
#[derive(Clone)]
pub struct OtherClip {
    /// Its playback region.
    pub region: u64,
    /// Where its take (its audio source) starts on the song, s.
    pub song_s: f64,
    /// The region's part of its take, samples.
    pub from: usize,
    pub to: usize,
    pub analysis: Arc<Analysis>,
    pub edits: Vec<NoteEdit>,
    /// Its rendered waveform (see [`State::peaks`]).
    pub peaks: Arc<Vec<(f32, f32)>>,
}

/// A waveform: the lowest and highest sample in each [`PEAK_HOP`].
pub type Peaks = Arc<Vec<(f32, f32)>>;

/// The waveform lane's peaks of `r`: its lowest and highest sample in each [`PEAK_HOP`].
pub fn peaks_of(r: &[f32]) -> Vec<(f32, f32)> {
    r.chunks(PEAK_HOP).map(|c| c.iter().fold((0.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)))).collect()
}

/// What a drag snaps pitch to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Snap {
    #[default]
    Semitone,
    Scale,
    Off,
}

/// The take's key: its tonic (0 = C), whether minor, and the snapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Scale {
    pub tonic: usize,
    pub minor: bool,
    pub snap: Snap,
}

const MAJOR_STEPS: [bool; 12] = [true, false, true, false, true, true, false, true, false, true, false, true];

impl Scale {
    /// Whether pitch class `pc` (0 = C) is in the key.
    pub fn contains(&self, pc: usize) -> bool {
        // A minor key is its relative major's notes.
        let major_tonic = if self.minor { (self.tonic + 3) % 12 } else { self.tonic };
        MAJOR_STEPS[(pc + 12 - major_tonic) % 12]
    }

    /// The pitch `cents` snaps to (with [`Snap::Off`], itself).
    pub fn snap(&self, cents: f64, snap: Snap) -> f64 {
        match snap {
            Snap::Off => cents,
            Snap::Semitone => (cents / 100.0).round() * 100.0,
            Snap::Scale => {
                let k = (cents / 100.0).round() as i64;
                // The nearest semitone in the key.
                (k - 6..=k + 6)
                    .filter(|&m| self.contains(m.rem_euclid(12) as usize))
                    .min_by(|a, b| ((*a as f64) * 100.0 - cents).abs().total_cmp(&((*b as f64) * 100.0 - cents).abs()))
                    .map_or(k as f64 * 100.0, |m| m as f64 * 100.0)
            }
        }
    }
}

/// What undo keeps: the notes' bounds and the edits.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub bounds: Vec<(usize, usize)>,
    pub edits: Vec<NoteEdit>,
}

pub struct State {
    pub scale: Scale,
    pub take: Option<TakeInfo>,
    pub audio: Option<Arc<Vec<f32>>>,
        pub analysis: Option<Arc<Analysis>>,
    /// The rendered take's waveform: its lowest and highest sample in each [`PEAK_HOP`].
    pub peaks: Arc<Vec<(f32, f32)>>,
    pub edits: Vec<NoteEdit>,
    pub status: Status,
    /// Bound to ARA: the current clip's part of the take (samples; `None`: all of it), the other
    /// clips on the editor's track, and the clip picked in the editor (a playback region; until the
    /// host's selection changes).
    pub clip: Option<(usize, usize)>,
    pub others: Arc<Vec<OtherClip>>,
    pub pick: Option<u64>,
    /// The song's bars and beats from the host's ARA musical context.
    pub ara_grid: Option<Arc<Grid>>,
}

enum Job {
    /// A recording finished: keep it (file), analyse it, render it.
    Recorded(Vec<f32>, i64, f64),
    /// Read a saved take back, analyse it, render it with these edits.
        Load(TakeInfo, Vec<NoteEdit>, Option<Vec<(usize, usize)>>),
    /// Re-render the samples of note `i`'s edit (or everything).
        Render(Option<usize>),
    /// Look at the ARA document now.
    Sync,
}

pub struct Session {
    pub state: Mutex<State>,
    /// Renders for the audio thread.
    pub renders: Handover<Rendered>,
    /// A fresh recording buffer for the audio thread, when the transfer is armed.
    pub records: Handover<Arc<RecordBuffer>>,
    pub armed: AtomicBool,
    /// Set by the audio thread when a recording has ended (the transport stopped or jumped).
    pub finished: AtomicBool,
    /// The transport, as the audio thread last saw it.
    pub playhead: AtomicI64,
        pub playing: AtomicBool,
    /// The plugin's own transport (while the host is stopped): play wanted (the UI sets it, the
    /// audio thread clears it at the take's end), the cue (take samples: where play starts),
    /// a seek to the cue wanted while playing, the position playing (published by the audio
    /// thread), the loop range (take samples) and whether it loops.
    pub local_play: AtomicBool,
    pub cue: AtomicI64,
    pub seek: AtomicBool,
    pub local_pos: AtomicI64,
    pub local_active: AtomicBool,
    pub loop_on: AtomicBool,
    pub loop_start: AtomicI64,
    pub loop_end: AtomicI64,
    /// The recording in progress (the UI's hold on it, so the audio thread never frees it).
    recording: Mutex<Option<Arc<RecordBuffer>>>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    /// Jobs waiting or running (for tests and the status).
    busy: AtomicUsize,
    /// Bumped whenever the notes or edits change (the editor redraws).
    pub version: AtomicU64,
    /// The song's time as the host's transport last gave it (the audio thread writes it): the
    /// tempo (bpm; f64 bits, 0: none), the signature (numerator << 16 | denominator), the quarter
    /// at timeline sample 0 and where in a bar quarter 0 falls (f64 bits; both constant while a
    /// tempo holds, so a read between two writes is off by nothing).
    song_bpm: AtomicU64,
    song_sig: AtomicU32,
    song_q0: AtomicU64,
    song_bar_q: AtomicU64,
    /// Diagnostics (MITE_PITCH_DIAG set): the loudest input and output sample since the worker
    /// last logged them (f32 bits), and the blocks seen.
    pub diag_in: AtomicU32,
    pub diag_out: AtomicU32,
    pub diag_blocks: AtomicU64,
    /// Renders this instance's worker has made, whole or in part (tests: a player instance takes
    /// the editor's render and makes none).
    pub renders_made: AtomicUsize,
    sample_rate: Mutex<f32>,
        /// The ARA plug-in instance, when the host bound this plugin to its document.
    ara: Mutex<Option<Arc<mite_ara::Instance>>>,
    /// Where this session keeps takes (`data_dir()` unless set: tests).
    dir: Mutex<std::path::PathBuf>,
    /// The editor's undo and redo per take, kept while the editor is closed.
    pub history: Mutex<History>,
}

/// Undo and redo steps (oldest first) per take id.
pub type History = std::collections::HashMap<String, (Vec<Snapshot>, Vec<Snapshot>)>;

impl Session {
    pub fn new() -> Arc<Self> {
        let s = Arc::new(Self {
            state: Mutex::new(State { scale: Scale::default(), take: None,
                audio: None,
                                analysis: None,
                peaks: Arc::new(Vec::new()),
                edits: Vec::new(),
                status: Status::Empty,
                clip: None,
                others: Arc::new(Vec::new()),
                pick: None,
                ara_grid: None,
            }),
            renders: Handover::new(),
            records: Handover::new(),
            armed: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            playhead: AtomicI64::new(0),
                        playing: AtomicBool::new(false),
            local_play: AtomicBool::new(false),
            cue: AtomicI64::new(0),
            seek: AtomicBool::new(false),
            local_pos: AtomicI64::new(0),
            local_active: AtomicBool::new(false),
            loop_on: AtomicBool::new(false),
            loop_start: AtomicI64::new(0),
            loop_end: AtomicI64::new(0),
            recording: Mutex::new(None),
            jobs: Mutex::new(None),
            busy: AtomicUsize::new(0),
            version: AtomicU64::new(0),
            renders_made: AtomicUsize::new(0),
            history: Mutex::new(History::new()),
            diag_in: AtomicU32::new(0),
            diag_out: AtomicU32::new(0),
            diag_blocks: AtomicU64::new(0),
            song_bpm: AtomicU64::new(0),
            song_sig: AtomicU32::new(0),
            song_q0: AtomicU64::new(0),
            song_bar_q: AtomicU64::new(0),
            sample_rate: Mutex::new(48_000.0),
                        dir: Mutex::new(data_dir()),
            ara: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        *s.jobs.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        let weak = Arc::downgrade(&s);
        let _ = std::thread::Builder::new()
            .name("mite-pitch-worker".into())
            .spawn(move || worker(weak, rx));
        s
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &*self.jobs.lock().unwrap_or_else(|e| e.into_inner()) {
            self.busy.fetch_add(1, Ordering::AcqRel);
            if tx.send(job).is_err() {
                self.busy.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

        /// Binds to an ARA document (the host's call, through the CLAP plugin extension): from now on
    /// the take is the clip the host gives, not a transfer.
    pub fn bind_ara(&self, inst: Arc<mite_ara::Instance>) {
        *self.ara.lock().unwrap_or_else(|e| e.into_inner()) = Some(inst);
        self.armed.store(false, Ordering::Release);
        self.lock().status = Status::Analysing;
        self.send(Job::Sync);
    }

    pub fn ara(&self) -> Option<Arc<mite_ara::Instance>> {
        self.ara.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The host's transport, each block (audio thread: atomics only): tempo `bpm`, signature
    /// `sig`, and at timeline sample `pos` (rate `fs`) quarter `beats`, the bar having started at
    /// quarter `bar_start`.
    pub fn set_song_time(&self, bpm: f64, sig: (i32, i32), fs: f64, pos: i64, beats: f64, bar_start: Option<f64>) {
        if !(bpm > 1.0 && bpm.is_finite() && fs > 0.0 && beats.is_finite()) {
            return;
        }
        let q0 = beats - pos as f64 / fs * bpm / 60.0;
        let (num, den) = (sig.0.clamp(1, 64), sig.1.clamp(1, 64));
        let bar_len = f64::from(num) * 4.0 / f64::from(den);
        let bar_q = bar_start.filter(|b| b.is_finite()).map_or(0.0, |b| b.rem_euclid(bar_len));
        self.song_bpm.store(bpm.to_bits(), Ordering::Relaxed);
        self.song_sig.store(((num as u32) << 16) | den as u32, Ordering::Relaxed);
        self.song_q0.store(q0.to_bits(), Ordering::Relaxed);
        self.song_bar_q.store(bar_q.to_bits(), Ordering::Relaxed);
    }

    /// The song's bars and beats: the ARA musical context's, else the host transport's (one
    /// tempo and signature), else none.
    pub fn grid(&self) -> Option<Arc<Grid>> {
        if let Some(g) = self.lock().ara_grid.clone() {
            return Some(g);
        }
        let bpm = f64::from_bits(self.song_bpm.load(Ordering::Relaxed));
        let sig = self.song_sig.load(Ordering::Relaxed);
        let (num, den) = ((sig >> 16) as i32, (sig & 0xffff) as i32);
        let q0 = f64::from_bits(self.song_q0.load(Ordering::Relaxed));
        let bar_q = f64::from_bits(self.song_bar_q.load(Ordering::Relaxed));
        Grid::constant(bpm, (num.max(1), den.max(1)), 0.0, q0, bar_q).map(Arc::new)
    }

    /// The other clips on the editor's track (ARA).
    pub fn others(&self) -> Arc<Vec<OtherClip>> {
        self.lock().others.clone()
    }

    /// The current clip's part of the take, samples (ARA; `None`: all of it).
    pub fn clip(&self) -> Option<(usize, usize)> {
        self.lock().clip
    }

    /// Makes another clip on the track (playback region `region`) the one being edited.
    pub fn pick_clip(&self, region: u64) {
        self.lock().pick = Some(region);
        self.send(Job::Sync);
    }

    /// Looks at the ARA document now (tests; the worker also looks every 40 ms).
    pub fn sync_ara(&self) {
        self.send(Job::Sync);
    }

    /// Keeps takes in `dir` instead of the data folder (tests).
    pub fn set_dir(&self, dir: std::path::PathBuf) {
        *self.dir.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }

    pub fn dir(&self) -> std::path::PathBuf {
        self.dir.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_sample_rate(&self, fs: f32) {
        let old = std::mem::replace(&mut *self.sample_rate.lock().unwrap_or_else(|e| e.into_inner()), fs);
        // Bound to ARA, the clips are placed in the host's samples: place them again.
        if old != fs && self.ara().is_some() {
            self.send(Job::Sync);
        }
    }

    /// The host's sample rate (the timeline's samples).
    pub fn host_rate(&self) -> f64 {
        f64::from(*self.sample_rate.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn status(&self) -> Status {
        self.lock().status.clone()
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire) > 0
    }

    /// Waits until the worker has nothing left to do (tests), at most `timeout`.
    pub fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let t0 = std::time::Instant::now();
        while self.busy() {
            if t0.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        true
    }

    /// Arms the transfer: a fresh buffer goes to the audio thread; the next stretch played is
    /// recorded (replacing the take). Or disarms it. UI thread (allocates).
    pub fn set_armed(&self, on: bool) {
        if on {
            let fs = *self.sample_rate.lock().unwrap_or_else(|e| e.into_inner());
            let buf = Arc::new(RecordBuffer::new(fs));
            *self.recording.lock().unwrap_or_else(|e| e.into_inner()) = Some(buf.clone());
            self.records.send(Box::new(buf));
            self.finished.store(false, Ordering::Release);
            self.armed.store(true, Ordering::Release);
            self.lock().status = Status::Armed;
        } else {
            self.armed.store(false, Ordering::Release);
            let mut st = self.lock();
            if matches!(st.status, Status::Armed) {
                st.status = if st.take.is_some() {
                    Status::Ready
                } else {
                    Status::Empty
                };
            }
        }
    }

    /// The UI's poll: a recording that has ended goes to the worker. Returns whether anything
    /// changed.
    pub fn poll(&self) -> bool {
        self.records.collect();
        let recording = self
            .recording
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(buf) = &recording {
            if !buf.is_empty() && matches!(self.lock().status, Status::Armed) {
                self.lock().status = Status::Recording;
            }
        }
        if self.finished.swap(false, Ordering::AcqRel) {
            self.armed.store(false, Ordering::Release);
            if let Some(buf) = self
                .recording
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                if !buf.is_empty() {
                    self.lock().status = Status::Analysing;
                    self.send(Job::Recorded(
                        buf.read(),
                        buf.start(),
                        f64::from(buf.sample_rate),
                    ));
                    return true;
                }
            }
            let mut st = self.lock();
            st.status = if st.take.is_some() {
                Status::Ready
            } else {
                Status::Empty
            };
            return true;
        }
        false
    }

    /// Note `i`'s edit becomes `edit`; only what it touches is rendered again.
    pub fn set_edit(&self, i: usize, edit: NoteEdit) {
        {
            let mut st = self.lock();
            if i >= st.edits.len() || st.edits[i] == edit {
                return;
            }
            st.edits[i] = edit;
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        self.send(Job::Render(Some(i)));
    }

    /// Every edit at once (undo, redo), rendered again throughout.
    pub fn set_edits(&self, edits: Vec<NoteEdit>) {
        {
            let mut st = self.lock();
            if st.edits.len() != edits.len() || st.edits == edits {
                return;
            }
            st.edits = edits;
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        self.send(Job::Render(None));
    }

        /// Several notes' edits at once (one drag over a selection).
    pub fn set_many(&self, changes: &[(usize, NoteEdit)]) {
        let changed: Vec<usize> = {
            let mut st = self.lock();
            changes
                .iter()
                .filter_map(|&(i, e)| {
                    let slot = st.edits.get_mut(i)?;
                    (*slot != e).then(|| {
                        *slot = e;
                        i
                    })
                })
                .collect()
        };
        if changed.is_empty() {
            return;
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        if changed.len() > 6 {
            self.send(Job::Render(None));
        } else {
            for i in changed {
                self.send(Job::Render(Some(i)));
            }
        }
    }

    /// The notes and edits now (for undo).
    pub fn snapshot(&self) -> Option<Snapshot> {
        let st = self.lock();
        Some(Snapshot { bounds: st.analysis.as_ref()?.bounds(), edits: st.edits.clone() })
    }

    /// Back to a snapshot: the notes rebuilt if they were split or merged since, the edits.
    pub fn restore(&self, snap: &Snapshot) {
        {
            let mut st = self.lock();
            let (Some(a), Some(x)) = (st.analysis.clone(), st.audio.clone()) else { return };
            if a.bounds() != snap.bounds {
                let mut b = (*a).clone();
                if !b.set_bounds(&x, &snap.bounds) {
                    return;
                }
                st.analysis = Some(Arc::new(b));
            }
            if st.analysis.as_ref().map_or(0, |a| a.notes.len()) != snap.edits.len() {
                return;
            }
            st.edits = snap.edits.clone();
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        self.send(Job::Render(None));
    }

    /// Splits note `i` at take sample `at`; both halves keep its edit.
    pub fn split(&self, i: usize, at: usize) -> bool {
        {
            let mut st = self.lock();
            let (Some(a), Some(x)) = (st.analysis.clone(), st.audio.clone()) else { return false };
            let mut b = (*a).clone();
            if !b.split(&x, i, at) {
                return false;
            }
            let e = st.edits[i];
            st.edits.insert(i + 1, e);
            st.analysis = Some(Arc::new(b));
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        self.send(Job::Render(None));
        true
    }

    /// Joins note `i` and the next; the joined note keeps note `i`'s edit.
    pub fn merge(&self, i: usize) -> bool {
        {
            let mut st = self.lock();
            let (Some(a), Some(x)) = (st.analysis.clone(), st.audio.clone()) else { return false };
            let mut b = (*a).clone();
            if !b.merge(&x, i) {
                return false;
            }
            st.edits.remove(i + 1);
            st.analysis = Some(Arc::new(b));
        }
        self.version.fetch_add(1, Ordering::AcqRel);
        self.send(Job::Render(None));
        true
    }

    pub fn scale(&self) -> Scale {
        self.lock().scale
    }

    pub fn set_scale(&self, scale: Scale) {
        self.lock().scale = scale;
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn edits(&self) -> Vec<NoteEdit> {
        self.lock().edits.clone()
    }

    pub fn analysis(&self) -> Option<Arc<Analysis>> {
        self.lock().analysis.clone()
    }

        /// The rendered take's waveform (see [`State::peaks`]).
    pub fn peaks(&self) -> Arc<Vec<(f32, f32)>> {
        self.lock().peaks.clone()
    }

    pub fn take(&self) -> Option<TakeInfo> {
        self.lock().take.clone()
    }

    /// What the plugin's state keeps: the take's id, place, rate, length and the edits (JSON).
        pub fn to_json(&self) -> Option<String> {
        if self.ara().is_some() {
            // The host's ARA archive keeps the edits.
            return None;
        }
        let st = self.lock();
        let t = st.take.as_ref()?;
        let edits: Vec<serde_json::Value> = st
            .edits
            .iter()
            .map(|e| {
                serde_json::json!([e.shift_cents, e.drift, e.offset, e.stretch, e.formant_semitones, e.gain_db, e.modulation, e.transition_ms])
            })
            .collect();
        let bounds: Vec<[usize; 2]> = st.analysis.as_ref().map(|a| a.bounds().into_iter().map(|(s, e)| [s, e]).collect()).unwrap_or_default();
        let sc = st.scale;
        let snap = match sc.snap {
            Snap::Semitone => 0,
            Snap::Scale => 1,
            Snap::Off => 2,
        };
        Some(serde_json::json!({"id": t.id, "start": t.start, "fs": t.sample_rate, "len": t.len, "edits": edits, "bounds": bounds, "key": sc.tonic, "minor": sc.minor, "snap": snap}).to_string())
    }

    /// Restores what [`Self::to_json`] kept: the take is read back and analysed on the worker.
    pub fn load_json(&self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        let (Some(id), Some(start), Some(fs), Some(len)) = (
            v["id"].as_str(),
            v["start"].as_i64(),
            v["fs"].as_f64(),
            v["len"].as_u64(),
        ) else {
            return;
        };
        let edits: Vec<NoteEdit> = v["edits"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|e| {
                        let g = |k: usize, d: f64| {
                            e.get(k).and_then(serde_json::Value::as_f64).unwrap_or(d)
                        };
                        NoteEdit {
                            shift_cents: g(0, 0.0),
                            drift: g(1, 1.0),
                            offset: g(2, 0.0),
                            stretch: g(3, 1.0),
                            formant_semitones: g(4, 0.0),
                            gain_db: g(5, 0.0), modulation: g(6, 1.0), transition_ms: g(7, 0.0) }
                    })
                    .collect()
            })
            .unwrap_or_default();
                let bounds: Option<Vec<(usize, usize)>> = v["bounds"].as_array().map(|a| a.iter().filter_map(|p| Some((p.get(0)?.as_u64()? as usize, p.get(1)?.as_u64()? as usize))).collect());
        let snap = match v["snap"].as_u64() {
            Some(1) => Snap::Scale,
            Some(2) => Snap::Off,
            _ => Snap::Semitone,
        };
        let scale = Scale { tonic: v["key"].as_u64().unwrap_or(0) as usize % 12, minor: v["minor"].as_bool().unwrap_or(false), snap };
        self.lock().scale = scale;
        let info = TakeInfo { id: id.to_owned(), start, sample_rate: fs, len: len as usize };
        if self.lock().take.as_ref() == Some(&info) {
            match bounds {
                Some(b) => self.restore(&Snapshot { bounds: b, edits }),
                None => self.set_edits(edits),
            }
            return;
        }
        self.lock().status = Status::Analysing;
        self.send(Job::Load(info, edits, bounds));
    }
}

/// Where takes live: the OS's data folder, `MiteFX/Pitch`.
pub fn data_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("MiteFX")
            .join("Pitch")
    } else if cfg!(target_os = "macos") {
        home.unwrap_or_else(std::env::temp_dir)
            .join("Library/Application Support/MiteFX/Pitch")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.map(|h| h.join(".local/share")))
            .unwrap_or_else(std::env::temp_dir)
            .join("MiteFX")
            .join("Pitch")
    }
}

fn take_path(dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    dir.join(format!("{id}.f32"))
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    format!("take-{:016x}", h.finish())
}

/// An audio modification's edits as JSON (what the ARA archive keeps for it).
pub fn edits_json(edits: &[NoteEdit], a: &Analysis, scale: Scale) -> String {
    let e: Vec<serde_json::Value> = edits.iter().map(|e| serde_json::json!([e.shift_cents, e.drift, e.offset, e.stretch, e.formant_semitones, e.gain_db, e.modulation, e.transition_ms])).collect();
    let bounds: Vec<[usize; 2]> = a.bounds().into_iter().map(|(s, e)| [s, e]).collect();
    let snap = match scale.snap {
        Snap::Semitone => 0,
        Snap::Scale => 1,
        Snap::Off => 2,
    };
    serde_json::json!({"edits": e, "bounds": bounds, "key": scale.tonic, "minor": scale.minor, "snap": snap}).to_string()
}

/// [`edits_json`] read back onto analysis `a` (its notes rebuilt over the kept bounds): the
/// edits (one per note, neutral when they do not fit) and the scale (`None`: not kept).
pub fn parse_edits_json(json: &str, x: &[f32], a: &mut Analysis) -> (Vec<NoteEdit>, Option<Scale>) {
    let neutral = |a: &Analysis| vec![NoteEdit::default(); a.notes.len()];
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return (neutral(a), None) };
    if let Some(b) = v["bounds"].as_array() {
        let b: Vec<(usize, usize)> = b.iter().filter_map(|p| Some((p.get(0)?.as_u64()? as usize, p.get(1)?.as_u64()? as usize))).collect();
        a.set_bounds(x, &b);
    }
    let edits: Vec<NoteEdit> = v["edits"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|e| {
                    let g = |k: usize, d: f64| e.get(k).and_then(serde_json::Value::as_f64).unwrap_or(d);
                    NoteEdit { shift_cents: g(0, 0.0), drift: g(1, 1.0), offset: g(2, 0.0), stretch: g(3, 1.0), formant_semitones: g(4, 0.0), gain_db: g(5, 0.0), modulation: g(6, 1.0), transition_ms: g(7, 0.0) }
                })
                .collect()
        })
        .unwrap_or_default();
    let snap = match v["snap"].as_u64() {
        Some(1) => Snap::Scale,
        Some(2) => Snap::Off,
        _ => Snap::Semitone,
    };
    let scale = v["key"].as_u64().map(|k| Scale { tonic: k as usize % 12, minor: v["minor"].as_bool().unwrap_or(false), snap });
    (if edits.len() == a.notes.len() { edits } else { neutral(a) }, scale)
}

/// One audio modification, worked out: its source's audio and analysis (notes rebuilt as its
/// edits keep them), its edits, its render, and the state string they came from. The editor
/// instance shares it through the document with each edit, so the player instance plays the edit
/// at once instead of rendering it again.
#[derive(Clone)]
struct ModCache {
    audio: Arc<Vec<f32>>,
    analysis: Arc<Analysis>,
    edits: Vec<NoteEdit>,
    scale: Scale,
    render: Arc<Vec<f32>>,
    state: String,
}

/// The worker's view of the ARA document.
#[derive(Default)]
struct AraState {
    seen: u64,
    /// Analyses by audio source, with the audio they came from.
    sources: std::collections::HashMap<u64, (Arc<Vec<f32>>, Arc<Analysis>)>,
    mods: std::collections::HashMap<u64, ModCache>,
    /// The region the editor shows, its modification, and the timeline sample its source starts at.
    current: Option<(u64, u64, i64)>,
    /// The host's selection as last seen (a change drops the editor's pick).
    selection_seen: Vec<u64>,
    /// The other clips as last published: (region, modification state, from, to, song start bits).
    others_key: Vec<(u64, String, usize, usize, u64)>,
    /// The waveform of each modification's render (by the render it came from).
    peaks: std::collections::HashMap<u64, (Arc<Vec<f32>>, Peaks)>,
    /// The regions this instance plays: (modification, timeline start and length in the host's
    /// samples, offset in the clip's, the clip's samples per host sample).
    regions: Vec<(u64, i64, usize, usize, f64)>,
    /// Sinc tables by (clip rate, host rate).
    sincs: Vec<((u64, u64), Arc<SincTable>)>,
    /// The host's rate the regions were placed at.
    host: f64,
}

impl AraState {
    /// Brings the caches up to date with the document. Returns whether anything that plays or
    /// shows changed, and the current take's render when the editor's take was replaced.
    fn sync(&mut self, s: &Session, force: bool) -> (bool, Option<Vec<f32>>) {
        let Some(inst) = s.ara() else { return (false, None) };
        let v = inst.document.version.load(Ordering::Acquire).wrapping_add(inst.version.load(Ordering::Acquire));
        if v == self.seen && !force {
            return (false, None);
        }
        self.seen = v;
                let model = inst.document.snapshot();
                                let mut st = inst.state();
        // Only a playback renderer plays its regions.
        if !Self::renders(&inst) {
            st.playback.clear();
        }
                // The regions this instance's editor shows: those given to it, and those on the tracks
        // (region sequences) given to it (REAPER gives the editor its track, not the clips).
        let mut shown: Vec<u64> = st.editor.iter().copied().chain(model.regions.values().filter(|r| st.sequences.contains(&r.sequence) || st.selected_sequences.contains(&r.sequence)).map(|r| r.id)).collect();
        {
            let mut seen = std::collections::HashSet::new();
            shown.retain(|id| seen.insert(*id));
        }
        // The host's selection moved: it wins over the clip picked in the editor.
        if st.selection != self.selection_seen {
            self.selection_seen = st.selection.clone();
            s.lock().pick = None;
        }
        let pick = s.lock().pick.filter(|p| shown.contains(p));
        let current_region = pick.iter().chain(st.selection.iter().filter(|id| shown.contains(id) || st.playback.contains(id))).chain(shown.iter()).chain(st.playback.iter()).find_map(|id| model.regions.get(id)).cloned();
        let mut wanted: Vec<u64> = st.playback.iter().filter_map(|id| model.regions.get(id).map(|r| r.modification)).collect();
        if let Some(r) = &current_region {
            wanted.push(r.modification);
        }
        // Every clip on the track is drawn.
        wanted.extend(shown.iter().filter_map(|id| model.regions.get(id).map(|r| r.modification)));
        wanted.sort_unstable();
        wanted.dedup();
        let mut changed = false;
        let mut pending = false;
        for &m in &wanted {
            let (Some(md), true) = (model.modifications.get(&m), true) else { continue };
            let Some(src) = model.sources.get(&md.source) else { continue };
            let Some(audio) = src.audio.clone() else {
                pending = true;
                continue;
            };
            let fresh_source = !self.sources.get(&src.id).is_some_and(|(a, _)| Arc::ptr_eq(a, &audio));
            if fresh_source {
                let a = analyse(&audio, src.sample_rate);
                self.sources.insert(src.id, (audio.clone(), Arc::new(a)));
            }
            let stale = fresh_source || !self.mods.get(&m).is_some_and(|c| c.state == md.state && Arc::ptr_eq(&c.audio, &audio));
            if stale {
                // Another instance (the editor) already rendered this state: take its render.
                let shared = inst.document.shared(m, &md.state).and_then(|v| v.downcast::<ModCache>().ok()).filter(|c| Arc::ptr_eq(&c.audio, &audio));
                let entry = match shared {
                    Some(c) => (*c).clone(),
                    None => {
                        let mut a = (*self.sources[&src.id].1).clone();
                        let (edits, scale) = parse_edits_json(&md.state, &audio, &mut a);
                        let scale = scale.unwrap_or_else(|| {
                            let (tonic, minor) = detect_key(&a.notes);
                            Scale { tonic, minor, snap: Snap::Semitone }
                        });
                        // Unedited, a render is its audio exactly: none needed (a track of many
                        // clips comes up at once).
                        let r = if edits.iter().all(NoteEdit::is_neutral) {
                            audio.clone()
                        } else {
                            let old = self.mods.get(&m).filter(|c| !fresh_source && Arc::ptr_eq(&c.audio, &audio));
                            s.renders_made.fetch_add(1, Ordering::AcqRel);
                            Arc::new(rerender(&audio, &a, &edits, old))
                        };
                        ModCache { audio: audio.clone(), analysis: Arc::new(a), edits, scale, render: r, state: md.state.clone() }
                    }
                };
                self.mods.insert(m, entry);
                changed = true;
            }
        }
        self.mods.retain(|m, _| model.modifications.contains_key(m));
        self.sources.retain(|id, _| model.sources.contains_key(id));
        // Where each played region sits.
        // The timeline counts the host's samples; the clip, its own (they differ when the clip's
        // file has another rate than the project).
        let host = s.host_rate();
        self.host = host;
        let regions: Vec<(u64, i64, usize, usize, f64)> = st
            .playback
            .iter()
            .filter_map(|id| {
                let r = model.regions.get(id)?;
                let fs = model.modifications.get(&r.modification).and_then(|m| model.sources.get(&m.source)).map(|s| s.sample_rate)?;
                Some((r.modification, (r.start_in_playback * host).round() as i64, (r.start_in_modification * fs).round().max(0.0) as usize, (r.duration_in_playback * host).round().max(0.0) as usize, fs / host))
            })
            .collect();
        for &(_, _, _, _, rate) in &regions {
            let key = ((rate * host).to_bits(), host.to_bits());
            if (rate - 1.0).abs() > 1e-9 && !self.sincs.iter().any(|(k, _)| *k == key) {
                self.sincs.push((key, Arc::new(SincTable::for_rates(rate * host, host))));
            }
        }
        if regions != self.regions {
            self.regions = regions;
            changed = true;
        }
        // The editor's take: the current region's modification.
        let current = current_region.as_ref().and_then(|r| {
            let fs = model.modifications.get(&r.modification).and_then(|m| model.sources.get(&m.source)).map(|s| s.sample_rate)?;
            Some((r.id, r.modification, ((r.start_in_playback - r.start_in_modification) * fs).round() as i64))
        });
        // The song's bars and beats, from the current clip's musical context (or any shown).
        let grid = current_region.iter().chain(shown.iter().filter_map(|id| model.regions.get(id))).find_map(|r| {
            let c = model.context_of(r)?;
            Grid::new(c.tempo.clone(), c.bars.clone())
        });
        {
            let mut stl = s.lock();
            if stl.ara_grid.as_deref() != grid.as_ref() {
                stl.ara_grid = grid.map(Arc::new);
                changed = true;
            }
        }
        // The other clips on the track, where they sit.
        let fs_of = |r: &mite_ara::PlaybackRegion| model.modifications.get(&r.modification).and_then(|m| model.sources.get(&m.source)).map(|s| s.sample_rate);
        let others: Vec<(&mite_ara::PlaybackRegion, f64)> = shown.iter().filter(|id| current.is_none_or(|c| c.0 != **id)).filter_map(|id| model.regions.get(id)).filter_map(|r| Some((r, fs_of(r)?))).filter(|(r, _)| self.mods.contains_key(&r.modification)).collect();
        let span = |r: &mite_ara::PlaybackRegion, fs: f64| ((r.start_in_modification * fs).round().max(0.0) as usize, ((r.start_in_modification + r.duration_in_modification) * fs).round().max(0.0) as usize);
        let key: Vec<(u64, String, usize, usize, u64)> = others.iter().map(|(r, fs)| {
            let (from, to) = span(r, *fs);
            (r.id, self.mods[&r.modification].state.clone(), from, to, (r.start_in_playback - r.start_in_modification).to_bits())
        }).collect();
        if key != self.others_key {
            let list: Vec<OtherClip> = others.iter().map(|(r, fs)| {
                let c = &self.mods[&r.modification];
                let peaks = match self.peaks.get(&r.modification) {
                    Some((src, p)) if Arc::ptr_eq(src, &c.render) => p.clone(),
                    _ => {
                        let p = Arc::new(peaks_of(&c.render));
                        self.peaks.insert(r.modification, (c.render.clone(), p.clone()));
                        p
                    }
                };
                let (from, to) = span(r, *fs);
                OtherClip { region: r.id, song_s: r.start_in_playback - r.start_in_modification, from, to, analysis: c.analysis.clone(), edits: c.edits.clone(), peaks }
            }).collect();
            self.peaks.retain(|m, _| model.modifications.contains_key(m));
            self.others_key = key;
            s.lock().others = Arc::new(list);
            changed = true;
        }
        let mut take = None;
        if let Some((rid, m, start)) = current {
            if let Some(c) = self.mods.get(&m) {
                if self.current != current || changed {
                    let fs = c.analysis.fs;
                    let clip = current_region.as_ref().map(|r| span(r, fs));
                    let mut stl = s.lock();
                    // The cursor and the loop keep their place on the song.
                    if let (Some(old), Some((orid, _, ostart))) = (&stl.take, self.current) {
                        let delta = ostart - start;
                        if orid != rid && delta != 0 && (old.sample_rate - fs).abs() < 1e-6 {
                            for a in [&s.cue, &s.loop_start, &s.loop_end] {
                                a.fetch_add(delta, Ordering::AcqRel);
                            }
                        }
                    }
                    stl.take = Some(TakeInfo { id: format!("ara:{m}"), start, sample_rate: fs, len: c.audio.len() });
                    stl.clip = clip;
                    stl.audio = Some(c.audio.clone());
                    stl.analysis = Some(c.analysis.clone());
                    stl.edits = c.edits.clone();
                    stl.scale = c.scale;
                    stl.status = Status::Ready;
                    drop(stl);
                    self.current = current;
                    take = Some((*c.render).clone());
                    changed = true;
                }
            }        } else if self.current.is_some() || pending || matches!(s.status(), Status::Analysing) {
            self.current = None;
            let mut stl = s.lock();
            stl.take = None;
            stl.clip = None;
            stl.analysis = None;
            stl.audio = None;
            stl.status = if pending { Status::Analysing } else { Status::Empty };
            changed = true;
        }
        if changed {
            s.version.fetch_add(1, Ordering::AcqRel);
        }
        (changed, take)
    }

    /// The editor changed the current modification's edits: keep them and tell the document.
    fn edited(&mut self, s: &Session, inst: &mite_ara::Instance, render: Arc<Vec<f32>>) {
        let Some((_, m, _)) = self.current else { return };
        let (analysis, edits, scale) = {
            let st = s.lock();
            (st.analysis.clone(), st.edits.clone(), st.scale)
        };
        let (Some(a), Some(c)) = (analysis, self.mods.get_mut(&m)) else { return };
        let state = edits_json(&edits, &a, scale);
        c.analysis = a;
        c.edits = edits.clone();
        c.scale = scale;
        c.render = render;
        c.state = state.clone();
        inst.document.share(m, &state, Arc::new(c.clone()));
        inst.document.set_state(m, state, edits.iter().all(NoteEdit::is_neutral));
        self.seen = inst.document.version.load(Ordering::Acquire).wrapping_add(inst.version.load(Ordering::Acquire));
    }

            /// Whether the host made this instance a playback renderer: then its regions are all that
    /// plays (silence elsewhere). An editor-only instance passes its input: REAPER (VST3) binds
    /// the visible FX with the editor roles and a hidden second instance as the renderer; edits
    /// reach it through the document.
    fn renders(inst: &mite_ara::Instance) -> bool {
        inst.roles & mite_ara::sys::kARAPlaybackRendererRole as i32 != 0
    }

    fn segments(&self) -> Vec<Segment> {
        self.regions
            .iter()
            .filter_map(|&(m, start, offset, len, rate)| {
                let c = self.mods.get(&m)?;
                let len = len.min((c.render.len().saturating_sub(offset) as f64 / rate) as usize);
                let same = (rate - 1.0).abs() <= 1e-9;
                let sinc = if same { None } else { self.sincs.iter().find(|(k, _)| *k == ((rate * self.host).to_bits(), self.host.to_bits())).map(|(_, t)| t.clone()) };
                if !same && sinc.is_none() {
                    return None;
                }
                Some(Segment { start, offset, len, audio: c.render.clone(), rate: if same { 1.0 } else { rate }, sinc })
            })
            .collect()
    }
}

/// The render of `x` with `edits` on analysis `a`: from `old` (the same audio's render with other
/// edits on the same notes) only the notes whose edits changed are rendered again; otherwise
/// all of it.
fn rerender(x: &[f32], a: &Analysis, edits: &[NoteEdit], old: Option<&ModCache>) -> Vec<f32> {
    let len = x.len();
    if let Some(o) = old.filter(|o| o.render.len() == len && o.edits.len() == edits.len() && o.analysis.bounds() == a.bounds()) {
        let changed: Vec<usize> = (0..edits.len()).filter(|&i| o.edits[i] != edits[i]).collect();
        if changed.len() <= 6 {
            let mut r = (*o.render).clone();
            for i in changed {
                let range = affected(a, i, len);
                let part = render_range(x, a, edits, range.clone());
                r[range].copy_from_slice(&part);
            }
            return r;
        }
    }
    let mut r = render(x, a, edits);
    r.resize(len, 0.0);
    r
}

fn worker(session: Weak<Session>, rx: mpsc::Receiver<Job>) {
    // The current take's render, kept here so an edit re-renders only its stretch.
    let mut current: Option<(i64, Vec<f32>)> = None;
    let mut ara = AraState::default();
    let diag = std::env::var_os("MITE_PITCH_DIAG").is_some();
    let mut diag_at = std::time::Instant::now();
    loop {
        let first = match rx.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(j) => Some(j),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Some(s) = session.upgrade() else { return };
        if diag && diag_at.elapsed() > std::time::Duration::from_millis(500) {
            diag_at = std::time::Instant::now();
            if let Some(inst) = s.ara() {
                let db = |a: &AtomicU32| 20.0 * f32::from_bits(a.swap(0, Ordering::AcqRel)).max(1e-9).log10();
                let segs: Vec<(i64, usize)> = ara.segments().iter().map(|g| (g.start, g.len)).collect();
                crate::ara::log(&format!("audio {:p} doc {:p}: roles {}, playing {}, pos {}, in {:.1} dB, out {:.1} dB, blocks {}, segments {segs:?}", Arc::as_ptr(&s), Arc::as_ptr(&inst.document), inst.roles, s.playing.load(Ordering::Relaxed), s.playhead.load(Ordering::Relaxed), db(&s.diag_in), db(&s.diag_out), s.diag_blocks.swap(0, Ordering::AcqRel)));
            }
        }
        let mut jobs: Vec<Job> = first.into_iter().collect();
        while let Ok(more) = rx.try_recv() {
            jobs.push(more);
        }
        let n = jobs.len();
        let force = jobs.iter().any(|j| matches!(j, Job::Sync));
        let (ara_changed, ara_take) = ara.sync(&s, force);
        if let Some(r) = ara_take {
            let start = s.take().map_or(0, |t| t.start);
            current = Some((start, r));
        }
        let mut render_all = false;
        let mut notes: Vec<usize> = Vec::new();
        for job in jobs {
            match job {
                Job::Recorded(audio, start, fs) => {
                    let id = new_id();
                    let dir = s.dir();
                    let problem = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(take_path(&dir, &id), audio.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())).err();
                    let a = analyse(&audio, fs);
                    let (tonic, minor) = detect_key(&a.notes);
                    let mut st = s.lock();
                    st.scale = Scale { tonic, minor, snap: st.scale.snap };
                    st.edits = vec![NoteEdit::default(); a.notes.len()];
                    st.take = Some(TakeInfo { id, start, sample_rate: fs, len: audio.len() });
                    st.audio = Some(Arc::new(audio));
                    st.analysis = Some(Arc::new(a));
                    st.status = match problem {
                        Some(e) => Status::Problem(format!("the take could not be saved ({e}); it plays until the project is closed")),
                        None => Status::Ready,
                    };
                    drop(st);
                    s.version.fetch_add(1, Ordering::AcqRel);
                    render_all = true;
                }
                Job::Load(info, edits, bounds) => match std::fs::read(take_path(&s.dir(), &info.id)) {
                    Ok(bytes) => {
                        let audio: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                        let mut a = analyse(&audio, info.sample_rate);
                        if let Some(b) = bounds {
                            a.set_bounds(&audio, &b);
                        }
                        let mut st = s.lock();
                        st.edits = if edits.len() == a.notes.len() { edits } else { vec![NoteEdit::default(); a.notes.len()] };
                        st.take = Some(info);
                        st.audio = Some(Arc::new(audio));
                        st.analysis = Some(Arc::new(a));
                        st.status = Status::Ready;
                        drop(st);
                        s.version.fetch_add(1, Ordering::AcqRel);
                        render_all = true;
                    }
                    Err(e) => s.lock().status = Status::Problem(format!("the take's file is missing ({e}): transfer it again")),
                },
                Job::Render(None) => render_all = true,
                Job::Render(Some(i)) => notes.push(i),
                Job::Sync => {}
            }
        }
        let rendering = render_all || !notes.is_empty();
        let (audio, analysis, edits, take) = {
            let st = s.lock();
            (st.audio.clone(), st.analysis.clone(), st.edits.clone(), st.take.clone())
        };
        if let (true, Some(x), Some(a), Some(t)) = (rendering, audio, analysis, take.clone()) {
            let len = x.len();
            s.renders_made.fetch_add(1, Ordering::AcqRel);
            let fresh = match current.take() {
                Some((start, mut r)) if !render_all && start == t.start && r.len() == len => {
                    for i in notes {
                        let range = affected(&a, i, len);
                        let part = render_range(&x, &a, &edits, range.clone());
                        r[range].copy_from_slice(&part);
                    }
                    r
                }
                _ => {
                    let mut r = render(&x, &a, &edits);
                    r.resize(len, 0.0);
                    r
                }
            };
            current = Some((t.start, fresh));
        }
        if rendering || ara_changed {
            if let Some((start, r)) = &current {
                let shared = Arc::new(r.clone());
                let rendered = match s.ara() {
                    Some(inst) => {
                        if rendering {
                            ara.edited(&s, &inst, shared.clone());
                        }
                        Rendered { segments: ara.segments(), ara: AraState::renders(&inst), take: None }
                    }
                    None => Rendered { segments: vec![Segment { start: *start, offset: 0, len: r.len(), audio: shared.clone(), rate: 1.0, sinc: None }], ara: false, take: Some(shared) },
                };
                s.renders.send(Box::new(rendered));
                let peaks = peaks_of(r);
                s.lock().peaks = Arc::new(peaks);
                s.version.fetch_add(1, Ordering::AcqRel);
            } else if let Some(inst) = s.ara() {
                s.renders.send(Box::new(Rendered { segments: ara.segments(), ara: AraState::renders(&inst), take: None }));
            }
        }
        s.busy.fetch_sub(n, Ordering::AcqRel);
    }
}
