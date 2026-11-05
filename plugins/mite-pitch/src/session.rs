//! Mite Pitch's take and everything done to it, off the audio thread (docs/design/pitch-brief.md
//! §1): the transfer's recording, the take's file, the analysis, the edits, and renders handed to
//! the audio thread. One worker thread per instance does the heavy work (writing and reading the
//! file, analysing, rendering), so the editor never waits and playback works without the editor
//! after a project reload.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use mite_core::Handover;
use mite_dsp::melody::{Analysis, NoteEdit, affected, analyse, render, render_range};

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
    pub start: i64,
    pub audio: Vec<f32>,
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

pub struct State {
    pub take: Option<TakeInfo>,
    pub audio: Option<Arc<Vec<f32>>>,
    pub analysis: Option<Arc<Analysis>>,
    pub edits: Vec<NoteEdit>,
    pub status: Status,
}

enum Job {
    /// A recording finished: keep it (file), analyse it, render it.
    Recorded(Vec<f32>, i64, f64),
    /// Read a saved take back, analyse it, render it with these edits.
    Load(TakeInfo, Vec<NoteEdit>),
    /// Re-render the samples of note `i`'s edit (or everything).
    Render(Option<usize>),
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
    /// The recording in progress (the UI's hold on it, so the audio thread never frees it).
    recording: Mutex<Option<Arc<RecordBuffer>>>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    /// Jobs waiting or running (for tests and the status).
    busy: AtomicUsize,
    /// Bumped whenever the notes or edits change (the editor redraws).
    pub version: AtomicU64,
    sample_rate: Mutex<f32>,
    /// Where this session keeps takes (`data_dir()` unless set: tests).
    dir: Mutex<std::path::PathBuf>,
}

impl Session {
    pub fn new() -> Arc<Self> {
        let s = Arc::new(Self {
            state: Mutex::new(State {
                take: None,
                audio: None,
                analysis: None,
                edits: Vec::new(),
                status: Status::Empty,
            }),
            renders: Handover::new(),
            records: Handover::new(),
            armed: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            playhead: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            recording: Mutex::new(None),
            jobs: Mutex::new(None),
            busy: AtomicUsize::new(0),
            version: AtomicU64::new(0),
            sample_rate: Mutex::new(48_000.0),
            dir: Mutex::new(data_dir()),
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

    /// Keeps takes in `dir` instead of the data folder (tests).
    pub fn set_dir(&self, dir: std::path::PathBuf) {
        *self.dir.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }

    pub fn dir(&self) -> std::path::PathBuf {
        self.dir.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_sample_rate(&self, fs: f32) {
        *self.sample_rate.lock().unwrap_or_else(|e| e.into_inner()) = fs;
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

    pub fn edits(&self) -> Vec<NoteEdit> {
        self.lock().edits.clone()
    }

    pub fn analysis(&self) -> Option<Arc<Analysis>> {
        self.lock().analysis.clone()
    }

    pub fn take(&self) -> Option<TakeInfo> {
        self.lock().take.clone()
    }

    /// What the plugin's state keeps: the take's id, place, rate, length and the edits (JSON).
    pub fn to_json(&self) -> Option<String> {
        let st = self.lock();
        let t = st.take.as_ref()?;
        let edits: Vec<serde_json::Value> = st
            .edits
            .iter()
            .map(|e| {
                serde_json::json!([
                    e.shift_cents,
                    e.drift,
                    e.offset,
                    e.stretch,
                    e.formant_semitones,
                    e.gain_db
                ])
            })
            .collect();
        Some(serde_json::json!({"id": t.id, "start": t.start, "fs": t.sample_rate, "len": t.len, "edits": edits}).to_string())
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
                            gain_db: g(5, 0.0),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let info = TakeInfo {
            id: id.to_owned(),
            start,
            sample_rate: fs,
            len: len as usize,
        };
        if self.lock().take.as_ref() == Some(&info) {
            self.set_edits(edits);
            return;
        }
        self.lock().status = Status::Analysing;
        self.send(Job::Load(info, edits));
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

fn worker(session: Weak<Session>, rx: mpsc::Receiver<Job>) {
    // The current render, kept here so an edit re-renders only its stretch.
    let mut current: Option<Rendered> = None;
    while let Ok(job) = rx.recv() {
        let Some(s) = session.upgrade() else { return };
        // Several renders waiting: one full one does for all.
        let mut jobs = vec![job];
        while let Ok(more) = rx.try_recv() {
            jobs.push(more);
        }
        let n = jobs.len();
        let mut render_all = false;
        let mut notes: Vec<usize> = Vec::new();
        for job in jobs {
            match job {
                Job::Recorded(audio, start, fs) => {
                    let id = new_id();
                    let dir = s.dir();
                    let problem = std::fs::create_dir_all(&dir)
                        .and_then(|_| {
                            std::fs::write(
                                take_path(&dir, &id),
                                audio
                                    .iter()
                                    .flat_map(|v| v.to_le_bytes())
                                    .collect::<Vec<u8>>(),
                            )
                        })
                        .err();
                    let a = analyse(&audio, fs);
                    let mut st = s.lock();
                    st.edits = vec![NoteEdit::default(); a.notes.len()];
                    st.take = Some(TakeInfo {
                        id,
                        start,
                        sample_rate: fs,
                        len: audio.len(),
                    });
                    st.audio = Some(Arc::new(audio));
                    st.analysis = Some(Arc::new(a));
                    st.status = match problem {
                        Some(e) => Status::Problem(format!(
                            "the take could not be saved ({e}); it plays until the project is closed"
                        )),
                        None => Status::Ready,
                    };
                    drop(st);
                    s.version.fetch_add(1, Ordering::AcqRel);
                    render_all = true;
                }
                Job::Load(info, edits) => match std::fs::read(take_path(&s.dir(), &info.id)) {
                    Ok(bytes) => {
                        let audio: Vec<f32> = bytes
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                            .collect();
                        let a = analyse(&audio, info.sample_rate);
                        let mut st = s.lock();
                        st.edits = if edits.len() == a.notes.len() {
                            edits
                        } else {
                            vec![NoteEdit::default(); a.notes.len()]
                        };
                        st.take = Some(info);
                        st.audio = Some(Arc::new(audio));
                        st.analysis = Some(Arc::new(a));
                        st.status = Status::Ready;
                        drop(st);
                        s.version.fetch_add(1, Ordering::AcqRel);
                        render_all = true;
                    }
                    Err(e) => {
                        s.lock().status = Status::Problem(format!(
                            "the take's file is missing ({e}): transfer it again"
                        ))
                    }
                },
                Job::Render(None) => render_all = true,
                Job::Render(Some(i)) => notes.push(i),
            }
        }
        let (audio, analysis, edits, take) = {
            let st = s.lock();
            (
                st.audio.clone(),
                st.analysis.clone(),
                st.edits.clone(),
                st.take.clone(),
            )
        };
        if let (Some(x), Some(a), Some(t)) = (audio, analysis, take) {
            let len = x.len();
            let fresh = match current.take() {
                Some(mut r) if !render_all && r.start == t.start && r.audio.len() == len => {
                    for i in notes {
                        let range = affected(&a, i, len);
                        let part = render_range(&x, &a, &edits, range.clone());
                        r.audio[range].copy_from_slice(&part);
                    }
                    r
                }
                _ => {
                    let mut audio = render(&x, &a, &edits);
                    audio.resize(len, 0.0);
                    Rendered {
                        start: t.start,
                        audio,
                    }
                }
            };
            s.renders.send(Box::new(Rendered {
                start: fresh.start,
                audio: fresh.audio.clone(),
            }));
            current = Some(fresh);
        }
        s.busy.fetch_sub(n, Ordering::AcqRel);
    }
}
