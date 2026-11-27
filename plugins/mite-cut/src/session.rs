//! Mite Cut's session: the ARA document as this instance sees it, the detection over every
//! track in it, and what the audio thread plays.
//!
//! - **Tracks:** each region sequence (the host's track) with clips. Its role is the user's
//!   (kept in each of its clips' modification state, so the project keeps it) or guessed from
//!   the track's name ("Tom 1", "Kick In", "OH L"); a name that says nothing is Off (passes).
//! - **Detection:** `mite_dsp::hits::detect` over every clip's audio source at once, aligned on
//!   the song (a clip starting later is padded in front), one run per document shared by all
//!   its instances (REAPER binds one per track, and a hidden player each): the first to need it
//!   runs it, the rest wait for it.
//! - **Cuts:** each clip's hits as its track's settings and its own overrides read them, as a
//!   gain curve over the clip's samples; each played region a `Segment` of its clip and curve.
//!
//! The worker thread does all of it; the editor and the audio thread read what it publishes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use mite_core::Handover;
use mite_dsp::hits::{Curve, Hit, Override, Role, Settings, TrackDetection, detect};
use mite_dsp::resample::SincTable;
use serde::{Deserialize, Serialize};

use crate::dsp::{Rendered, Segment};

/// What the editor shows of a track.
#[derive(Clone, Debug)]
pub struct Track {
    /// Its region sequence.
    pub sequence: u64,
    pub name: String,
    pub role: Role,
    /// The role was guessed from the name (the user has not chosen one).
    pub guessed: bool,
    pub settings: Settings,
    /// Its clips' hits (kept or not), each clip's in its own samples.
    pub clips: Vec<Clip>,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub modification: u64,
    /// Where the clip's sample 0 is on the song, s, and its rate.
    pub song_s: f64,
    pub fs: f64,
    pub len: usize,
    pub hits: Arc<Vec<Hit>>,
    pub curve: Arc<Curve>,
    /// Its waveform: the lowest and highest sample in each [`PEAK_HOP`].
    pub peaks: Peaks,
}

/// A waveform (see [`peaks_of`]).
pub type Peaks = Arc<Vec<(f32, f32)>>;

/// Samples per waveform peak.
pub const PEAK_HOP: usize = 128;

/// The waveform of `x`: its lowest and highest sample in each [`PEAK_HOP`].
pub fn peaks_of(x: &[f32]) -> Vec<(f32, f32)> {
    x.chunks(PEAK_HOP).map(|c| c.iter().fold((0.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)))).collect()
}

impl Track {
    /// Hits kept, and candidates.
    pub fn counts(&self) -> (usize, usize) {
        self.clips.iter().fold((0, 0), |(k, n), c| (k + c.hits.iter().filter(|h| h.kept).count(), n + c.hits.len()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Not bound to an ARA document: Mite Cut needs one (a VST3 host with ARA).
    Unbound,
    /// Reading the clips' audio from the host.
    Reading,
    Detecting,
    Ready,
}

/// What a clip's modification state holds (JSON): its track's role and settings, its overrides.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct ModState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<SettingsState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideState>,
}

/// [`Settings`] as saved.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct SettingsState {
    pub sensitivity: f32,
    pub bleed: f32,
    pub pre_roll_ms: f32,
    pub fade_in_ms: f32,
    pub hold_ms: f32,
    pub release_db: f32,
    pub fade_out_ms: f32,
    pub floor_db: f32,
    pub min_gap_ms: f32,
    pub min_length_ms: f32,
}

impl From<Settings> for SettingsState {
    fn from(s: Settings) -> Self {
        Self { sensitivity: s.sensitivity, bleed: s.bleed, pre_roll_ms: s.pre_roll_ms, fade_in_ms: s.fade_in_ms, hold_ms: s.hold_ms, release_db: s.release_db, fade_out_ms: s.fade_out_ms, floor_db: s.floor_db, min_gap_ms: s.min_gap_ms, min_length_ms: s.min_length_ms }
    }
}

impl From<SettingsState> for Settings {
    fn from(s: SettingsState) -> Self {
        Self { sensitivity: s.sensitivity, bleed: s.bleed, pre_roll_ms: s.pre_roll_ms, fade_in_ms: s.fade_in_ms, hold_ms: s.hold_ms, release_db: s.release_db, fade_out_ms: s.fade_out_ms, floor_db: s.floor_db, min_gap_ms: s.min_gap_ms, min_length_ms: s.min_length_ms }
    }
}

/// [`Override`] as saved (clip samples).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct OverrideState {
    pub at: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<usize>,
}

impl ModState {
    pub fn parse(json: &str) -> Self {
        if json.trim().is_empty() { Self::default() } else { serde_json::from_str(json).unwrap_or_default() }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn role(&self) -> Option<Role> {
        self.role.as_deref().and_then(role_from_name)
    }

    fn overrides(&self) -> Vec<Override> {
        self.overrides.iter().map(|o| Override { at: o.at, keep: o.keep, start: o.start, end: o.end }).collect()
    }
}

/// A role by its saved name.
pub fn role_from_name(s: &str) -> Option<Role> {
    Role::ALL.iter().copied().find(|r| r.name().eq_ignore_ascii_case(s))
}

/// The role a track's name suggests: its words (and their starts: "tom1", "kickin").
pub fn guess_role(name: &str) -> Role {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |keys: &[&str]| words.iter().any(|w| keys.iter().any(|k| *w == *k || (k.len() >= 3 && w.starts_with(k))));
    if has(&["kick", "kik", "bd", "bassdrum", "sub"]) && !has(&["bass"]) || has(&["kick"]) {
        Role::Kick
    } else if has(&["snare", "snr", "sn", "sd"]) {
        Role::Snare
    } else if has(&["tom", "toms", "floor", "rack", "ft", "rt"]) {
        Role::Tom
    } else if has(&["hat", "hihat", "hh", "hats"]) {
        Role::Hat
    } else if has(&["ride", "crash", "cym", "china", "splash"]) {
        Role::Cymbal
    } else if has(&["oh", "overhead", "overheads", "ohs"]) {
        Role::Overhead
    } else if has(&["room", "amb", "ambience", "rooms"]) {
        Role::Room
    } else {
        Role::Off
    }
}

pub struct State {
    pub status: Status,
    pub tracks: Arc<Vec<Track>>,
    pub export: Option<Export>,
}

enum Job {
    /// Look at the ARA document now.
    Sync,
    /// The user chose track `sequence`'s role.
    Role(u64, Role),
    /// The user changed track `sequence`'s settings.
    Settings(u64, Settings),
    /// The user kept, removed or moved a hit of clip `modification`.
    Override(u64, OverrideState),
    /// Write every cut track to WAV files in this folder.
    Export(std::path::PathBuf),
}

/// An export of the cut tracks as audio files.
#[derive(Debug, Clone, PartialEq)]
pub enum Export {
    Writing,
    /// The folder, and the files written there.
    Done(std::path::PathBuf, Vec<String>),
    Failed(String),
}

pub struct Session {
    state: Mutex<State>,
    /// Renders for the audio thread.
    pub renders: Handover<Rendered>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    /// Jobs waiting or running (tests, the status).
    busy: AtomicUsize,
    /// Bumped whenever what the editor shows changes.
    pub version: AtomicU64,
    /// The transport as the audio thread last saw it (host samples), and whether it plays.
    pub playhead: AtomicI64,
    pub playing: AtomicBool,
    /// Detection runs this instance's worker made (tests: instances share one).
    pub detections_made: AtomicUsize,
    sample_rate: Mutex<f32>,
    ara: Mutex<Option<Arc<mite_ara::Instance>>>,
}

impl Session {
    pub fn new() -> Arc<Self> {
        let s = Arc::new(Self {
            state: Mutex::new(State { status: Status::Unbound, tracks: Arc::new(Vec::new()), export: None }),
            renders: Handover::new(),
            jobs: Mutex::new(None),
            busy: AtomicUsize::new(0),
            version: AtomicU64::new(0),
            playhead: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            detections_made: AtomicUsize::new(0),
            sample_rate: Mutex::new(48_000.0),
            ara: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        *s.jobs.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        let weak = Arc::downgrade(&s);
        let _ = std::thread::Builder::new().name("mite-cut-worker".into()).spawn(move || worker(weak, rx));
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

    /// Binds to an ARA document (the host's call).
    pub fn bind_ara(&self, inst: Arc<mite_ara::Instance>) {
        *self.ara.lock().unwrap_or_else(|e| e.into_inner()) = Some(inst);
        self.lock().status = Status::Reading;
        self.send(Job::Sync);
    }

    pub fn ara(&self) -> Option<Arc<mite_ara::Instance>> {
        self.ara.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Looks at the document now (the worker also does every 40 ms).
    pub fn sync_ara(&self) {
        self.send(Job::Sync);
    }

    pub fn set_sample_rate(&self, fs: f32) {
        *self.sample_rate.lock().unwrap_or_else(|e| e.into_inner()) = fs;
        self.send(Job::Sync);
    }

    pub fn host_rate(&self) -> f64 {
        f64::from(*self.sample_rate.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn status(&self) -> Status {
        self.lock().status.clone()
    }

    pub fn tracks(&self) -> Arc<Vec<Track>> {
        self.lock().tracks.clone()
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire) > 0
    }

    /// Waits until the worker has nothing left to do (tests).
    pub fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < timeout {
            if !self.busy() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    pub fn set_role(&self, sequence: u64, role: Role) {
        self.send(Job::Role(sequence, role));
    }

    pub fn set_settings(&self, sequence: u64, settings: Settings) {
        self.send(Job::Settings(sequence, settings));
    }

    pub fn set_override(&self, modification: u64, o: OverrideState) {
        self.send(Job::Override(modification, o));
    }

    /// Writes every cut track as WAV files into `dir` (by default a new folder, named by the
    /// time, in [`export_root`]).
    pub fn export(&self, dir: Option<std::path::PathBuf>) {
        let dir = dir.unwrap_or_else(|| export_root().join(stamp(std::time::SystemTime::now()).replace(':', "-")));
        self.lock().export = Some(Export::Writing);
        self.version.fetch_add(1, Ordering::AcqRel);
        self.send(Job::Export(dir));
    }

    /// The last export, if any.
    pub fn exported(&self) -> Option<Export> {
        self.lock().export.clone()
    }
}

/// Where exports go: `~/Music/Mite Cut` (or the home folder's `Mite Cut` without a Music
/// folder).
pub fn export_root() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let music = home.join("Music");
    if music.is_dir() { music.join("Mite Cut") } else { home.join("Mite Cut") }
}

/// `t` as `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn stamp(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days to the civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

/// A track's name as a file name (no path separators or characters file systems refuse).
fn file_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let n = n.trim().trim_matches('.').to_owned();
    if n.is_empty() { "Track".to_owned() } else { n }
}

/// Writes `x` (mono, `fs`) as a 32-bit float WAV with a broadcast-wave chunk whose time
/// reference is `song_at` (samples since the song's start: hosts that read it place the file
/// where its clip was).
pub fn write_wav(path: &std::path::Path, x: &[f32], fs: u32, song_at: u64, description: &str) -> std::io::Result<()> {
    let mut b: Vec<u8> = Vec::with_capacity(x.len() * 4 + 1024);
    let chunk = |b: &mut Vec<u8>, id: &[u8; 4], body: &[u8]| {
        b.extend_from_slice(id);
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(body);
        if body.len() % 2 == 1 {
            b.push(0);
        }
    };
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&fs.to_le_bytes());
    fmt.extend_from_slice(&(fs * 4).to_le_bytes());
    fmt.extend_from_slice(&4u16.to_le_bytes());
    fmt.extend_from_slice(&32u16.to_le_bytes());
    fmt.extend_from_slice(&0u16.to_le_bytes());
    // bext (EBU Tech 3285): description, originator, its reference, date, time, the time
    // reference (low, high), version, UMID, loudness, reserved.
    let mut bext = vec![0u8; 602];
    let put = |b: &mut [u8], at: usize, len: usize, s: &str| {
        let s = s.as_bytes();
        b[at..at + s.len().min(len)].copy_from_slice(&s[..s.len().min(len)]);
    };
    put(&mut bext, 0, 256, description);
    put(&mut bext, 256, 32, "Mite Cut");
    let now = stamp(std::time::SystemTime::now());
    put(&mut bext, 320, 10, &now[..10]);
    put(&mut bext, 330, 8, &now[11..].replace(':', "-"));
    bext[338..342].copy_from_slice(&(song_at as u32).to_le_bytes());
    bext[342..346].copy_from_slice(&((song_at >> 32) as u32).to_le_bytes());
    bext[346..348].copy_from_slice(&1u16.to_le_bytes());
    let data: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    chunk(&mut body, b"fmt ", &fmt);
    chunk(&mut body, b"fact", &(x.len() as u32).to_le_bytes());
    chunk(&mut body, b"bext", &bext);
    chunk(&mut body, b"data", &data);
    chunk(&mut b, b"RIFF", &body);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, b)
}

/// Where Mite Cut keeps its log.
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))).unwrap_or_else(std::env::temp_dir);
    base.join("MiteFX").join("Cut")
}

/// One clip as detection sees it.
#[derive(Clone)]
struct Unit {
    modification: u64,
    source: u64,
    sequence: u64,
    audio: Arc<Vec<f32>>,
    fs: f64,
    /// Where the clip's sample 0 is on the song, s.
    song_s: f64,
    role: Role,
    state: ModState,
}

/// What a detection ran on: (source, its audio, role, its song start in samples, rate).
type DetKey = Vec<(u64, usize, Role, i64, u64)>;

/// A detection over a document's clips: per clip (by source), its detection and how many
/// samples it was padded in front.
struct Detected {
    key: DetKey,
    per_source: HashMap<u64, (Arc<TrackDetection>, usize)>,
}

type Slot = Arc<Mutex<Option<Arc<Detected>>>>;

/// One detection per document, shared by its instances (keyed by the document's address).
static DETECTIONS: Mutex<Vec<(usize, Slot)>> = Mutex::new(Vec::new());

fn slot_of(doc: &Arc<mite_ara::Document>) -> Slot {
    let mut all = DETECTIONS.lock().unwrap_or_else(|e| e.into_inner());
    let at = Arc::as_ptr(doc) as usize;
    // (A document gone and another at its address: the key, made of the audio's addresses,
    // tells them apart.)
    if let Some((_, s)) = all.iter().find(|(a, _)| *a == at) {
        return s.clone();
    }
    let s: Slot = Arc::new(Mutex::new(None));
    all.push((at, s.clone()));
    s
}

fn key_of(units: &[Unit]) -> DetKey {
    let mut k: DetKey = units.iter().map(|u| (u.source, Arc::as_ptr(&u.audio) as usize, u.role, (u.song_s * u.fs).round() as i64, u.fs.to_bits())).collect();
    k.sort_unstable_by_key(|e| e.0);
    k.dedup_by_key(|e| e.0);
    k
}

/// Detection over `units` (by source; those of one rate together, aligned on the song).
fn run_detection(units: &[Unit], key: DetKey) -> Detected {
    let mut per_source = HashMap::new();
    let mut sources: Vec<&Unit> = Vec::new();
    for u in units {
        if !sources.iter().any(|v| v.source == u.source) {
            sources.push(u);
        }
    }
    let mut rates: Vec<u64> = sources.iter().map(|u| u.fs.to_bits()).collect();
    rates.sort_unstable();
    rates.dedup();
    for rate in rates {
        let group: Vec<&Unit> = sources.iter().copied().filter(|u| u.fs.to_bits() == rate).collect();
        let starts: Vec<i64> = group.iter().map(|u| (u.song_s * u.fs).round() as i64).collect();
        let origin = starts.iter().copied().min().unwrap_or(0);
        let padded: Vec<(usize, std::borrow::Cow<'_, [f32]>)> = group
            .iter()
            .zip(&starts)
            .map(|(u, &st)| {
                let pad = (st - origin).max(0) as usize;
                if pad == 0 {
                    (0, std::borrow::Cow::Borrowed(u.audio.as_slice()))
                } else {
                    let mut v = vec![0.0f32; pad];
                    v.extend_from_slice(&u.audio);
                    (pad, std::borrow::Cow::Owned(v))
                }
            })
            .collect();
        let tracks: Vec<(&[f32], Role)> = padded.iter().zip(&group).map(|((_, a), u)| (a.as_ref(), u.role)).collect();
        let dets = detect(&tracks, f64::from_bits(rate) as f32);
        for ((d, (pad, _)), u) in dets.into_iter().zip(&padded).zip(&group) {
            per_source.insert(u.source, (Arc::new(d), *pad));
        }
    }
    Detected { key, per_source }
}

/// A clip's hits and cut: `det`'s reading under `settings` with the clip's overrides, moved
/// from the padded timeline to the clip's samples.
fn cut_of(det: &TrackDetection, pad: usize, settings: &Settings, overrides: &[Override], len: usize, fs: f64) -> (Vec<Hit>, Curve) {
    if !det.role.cuts() {
        // Off, overheads, room: they pass as they are.
        return (Vec::new(), Curve::new(&[], &Settings { floor_db: 0.0, ..*settings }, fs as f32));
    }
    let shifted: Vec<Override> = overrides.iter().map(|o| Override { at: o.at + pad, start: o.start.map(|v| v + pad), end: o.end.map(|v| v + pad), keep: o.keep }).collect();
    let back = |v: usize| v.saturating_sub(pad).min(len);
    let hits: Vec<Hit> = det.hits(settings, &shifted).into_iter().filter(|h| h.onset >= pad).map(|h| Hit { onset: back(h.onset), peak: back(h.peak), start: back(h.start), end: back(h.end), ..h }).collect();
    let curve = Curve::new(&hits, settings, fs as f32);
    (hits, curve)
}

/// A clip's cut: the state (and role) it was read with, its hits, its curve.
type Cut = (String, Arc<Vec<Hit>>, Arc<Curve>);

/// The worker's view of the document.
#[derive(Default)]
struct AraState {
    seen: u64,
    detected: Option<Arc<Detected>>,
    /// Each clip's cut.
    cuts: HashMap<u64, Cut>,
    /// The regions this instance plays: (modification, timeline start and length in the host's
    /// samples, offset in the clip's, the clip's samples per host sample).
    regions: Vec<(u64, i64, usize, usize, f64)>,
    sincs: Vec<((u64, u64), Arc<SincTable>)>,
    host: f64,
    audio: HashMap<u64, Arc<Vec<f32>>>,
    /// Each clip's waveform, with the audio it came from.
    peaks: HashMap<u64, (Arc<Vec<f32>>, Peaks)>,
    renders: bool,
}

impl AraState {
    /// Brings everything up to date with the document. Returns whether what plays changed.
    fn sync(&mut self, s: &Session, force: bool) -> bool {
        let Some(inst) = s.ara() else { return false };
        let v = inst.document.version.load(Ordering::Acquire).wrapping_add(inst.version.load(Ordering::Acquire));
        let host = s.host_rate();
        if v == self.seen && !force && host == self.host {
            return false;
        }
        self.seen = v;
        let model = inst.document.snapshot();
        let st = inst.state();
        self.renders = inst.roles & mite_ara::sys::kARAPlaybackRendererRole as i32 != 0;
        // Every clip in the document: its first region places it on the song and on a track.
        let mut units: Vec<Unit> = Vec::new();
        let mut pending = false;
        for md in model.modifications.values() {
            let Some(src) = model.sources.get(&md.source) else { continue };
            let Some(r) = model.regions.values().find(|r| r.modification == md.id) else { continue };
            let Some(audio) = src.audio.clone() else {
                pending = true;
                continue;
            };
            let state = ModState::parse(&md.state);
            let name = model.sequences.get(&r.sequence).cloned().unwrap_or_default();
            let role = state.role().unwrap_or_else(|| guess_role(&name));
            units.push(Unit { modification: md.id, source: src.id, sequence: r.sequence, audio, fs: src.sample_rate, song_s: r.start_in_playback - r.start_in_modification, role, state });
        }
        if pending && units.is_empty() {
            s.lock().status = Status::Reading;
            s.version.fetch_add(1, Ordering::AcqRel);
            return false;
        }
        // The detection: shared by the document's instances.
        let key = key_of(&units);
        if self.detected.as_ref().is_none_or(|d| d.key != key) {
            let slot = slot_of(&inst.document);
            let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
            let fresh = match &*held {
                Some(d) if d.key == key => d.clone(),
                _ => {
                    s.lock().status = Status::Detecting;
                    s.version.fetch_add(1, Ordering::AcqRel);
                    s.detections_made.fetch_add(1, Ordering::AcqRel);
                    let d = Arc::new(run_detection(&units, key.clone()));
                    *held = Some(d.clone());
                    d
                }
            };
            drop(held);
            self.detected = Some(fresh);
            self.cuts.clear();
        }
        let det = self.detected.clone().expect("detected");
        // Each clip's cut.
        let mut changed = false;
        for u in &units {
            let Some((d, pad)) = det.per_source.get(&u.source) else { continue };
            let state_json = u.state.to_json() + u.role.name();
            if self.cuts.get(&u.modification).is_some_and(|c| c.0 == state_json) {
                continue;
            }
            // The track's settings are any of its clips' (they are kept alike).
            let settings: Settings = units.iter().filter(|v| v.sequence == u.sequence).find_map(|v| v.state.settings).map(Settings::from).unwrap_or_default();
            // (The detection ran with this role: the role is in its key.)
            let (hits, curve) = cut_of(d, *pad, &settings, &u.state.overrides(), u.audio.len(), u.fs);
            self.cuts.insert(u.modification, (state_json, Arc::new(hits), Arc::new(curve)));
            changed = true;
        }
        self.cuts.retain(|m, _| units.iter().any(|u| u.modification == *m));
        self.audio = units.iter().map(|u| (u.modification, u.audio.clone())).collect();
        self.peaks.retain(|m, _| units.iter().any(|u| u.modification == *m));
        // The tracks for the editor.
        let mut tracks: Vec<Track> = Vec::new();
        for u in &units {
            let Some(cut) = self.cuts.get(&u.modification) else { continue };
            let peaks = match self.peaks.get(&u.modification) {
                Some((a, p)) if Arc::ptr_eq(a, &u.audio) => p.clone(),
                _ => {
                    let p = Arc::new(peaks_of(&u.audio));
                    self.peaks.insert(u.modification, (u.audio.clone(), p.clone()));
                    p
                }
            };
            let clip = Clip { modification: u.modification, song_s: u.song_s, fs: u.fs, len: u.audio.len(), hits: cut.1.clone(), curve: cut.2.clone(), peaks };
            match tracks.iter_mut().find(|t| t.sequence == u.sequence) {
                Some(t) => t.clips.push(clip),
                None => {
                    let name = model.sequences.get(&u.sequence).cloned().unwrap_or_default();
                    let settings = u.state.settings.map(Settings::from).unwrap_or_default();
                    tracks.push(Track { sequence: u.sequence, guessed: u.state.role().is_none(), name, role: u.role, settings, clips: vec![clip] });
                }
            }
        }
        // Where each played region sits (the host's samples on the timeline, the clip's in it).
        self.host = host;
        let playing: Vec<u64> = if self.renders { st.playback.clone() } else { Vec::new() };
        let regions: Vec<(u64, i64, usize, usize, f64)> = playing
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
        {
            let mut stl = s.lock();
            stl.tracks = Arc::new(tracks);
            stl.status = if pending { Status::Reading } else { Status::Ready };
        }
        s.version.fetch_add(1, Ordering::AcqRel);
        changed || force
    }

    fn segments(&self) -> Vec<Segment> {
        self.regions
            .iter()
            .filter_map(|&(m, start, offset, len, rate)| {
                let audio = self.audio.get(&m)?.clone();
                let curve = self.cuts.get(&m)?.2.clone();
                let len = len.min((audio.len().saturating_sub(offset) as f64 / rate) as usize);
                let same = (rate - 1.0).abs() <= 1e-9;
                let sinc = if same { None } else { self.sincs.iter().find(|(k, _)| *k == ((rate * self.host).to_bits(), self.host.to_bits())).map(|(_, t)| t.clone()) };
                if !same && sinc.is_none() {
                    return None;
                }
                Some(Segment { start, offset, len, audio, rate: if same { 1.0 } else { rate }, sinc, curve })
            })
            .collect()
    }

    /// Writes every cut track's clips, as they play, into `dir`: one file a clip, named by its
    /// track (and numbered when a track has more than one), at the clip's rate, from its first
    /// sample, stamped with where it sits on the song.
    fn export(&self, s: &Session, dir: &std::path::Path) -> Result<Vec<String>, String> {
        let tracks = s.tracks();
        let mut files = Vec::new();
        for t in tracks.iter().filter(|t| t.role.cuts()) {
            for (i, c) in t.clips.iter().enumerate() {
                let Some(audio) = self.audio.get(&c.modification) else { continue };
                let mut gain = vec![0.0f32; audio.len()];
                c.curve.fill(0, &mut gain);
                let out: Vec<f32> = audio.iter().zip(&gain).map(|(x, g)| x * g).collect();
                let base = file_name(&t.name);
                let name = if t.clips.len() > 1 { format!("{base} {}.wav", i + 1) } else { format!("{base}.wav") };
                let song_at = (c.song_s * c.fs).round().max(0.0) as u64;
                write_wav(&dir.join(&name), &out, c.fs.round() as u32, song_at, &format!("{} cut by Mite Cut", t.name)).map_err(|e| format!("{}: {e}", dir.join(&name).display()))?;
                files.push(name);
            }
        }
        if files.is_empty() {
            return Err("no track is cut: give the drum tracks a role first".to_owned());
        }
        Ok(files)
    }

    /// Writes `change` into the state of every clip `pick` selects (the host keeps it).
    fn write(&mut self, s: &Session, pick: impl Fn(&mite_ara::PlaybackRegion) -> bool, change: impl Fn(&mut ModState)) {
        let Some(inst) = s.ara() else { return };
        let model = inst.document.snapshot();
        for md in model.modifications.values() {
            if !model.regions.values().any(|r| r.modification == md.id && pick(r)) {
                continue;
            }
            let mut st = ModState::parse(&md.state);
            change(&mut st);
            let off = st.role().is_some_and(|r| !r.cuts());
            inst.document.set_state(md.id, st.to_json(), off);
        }
    }
}

fn worker(session: Weak<Session>, rx: mpsc::Receiver<Job>) {
    let mut ara = AraState::default();
    loop {
        let first = match rx.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(j) => Some(j),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Some(s) = session.upgrade() else { return };
        let mut jobs: Vec<Job> = first.into_iter().collect();
        while let Ok(more) = rx.try_recv() {
            jobs.push(more);
        }
        let n = jobs.len();
        let mut force = false;
        for job in jobs {
            match job {
                Job::Sync => force = true,
                Job::Role(seq, role) => {
                    ara.write(&s, |r| r.sequence == seq, |st| st.role = Some(role.name().to_owned()));
                    force = true;
                }
                Job::Settings(seq, settings) => {
                    ara.write(&s, |r| r.sequence == seq, |st| st.settings = Some(settings.into()));
                    force = true;
                }
                Job::Export(dir) => {
                    let result = ara.export(&s, &dir);
                    s.lock().export = Some(match result {
                        Ok(files) => Export::Done(dir, files),
                        Err(e) => Export::Failed(e),
                    });
                    s.version.fetch_add(1, Ordering::AcqRel);
                }
                Job::Override(m, o) => {
                    ara.write(
                        &s,
                        |r| r.modification == m,
                        |st| {
                            st.overrides.retain(|x| x.at.abs_diff(o.at) > 480);
                            if o.keep.is_some() || o.start.is_some() || o.end.is_some() {
                                st.overrides.push(o);
                            }
                        },
                    );
                    force = true;
                }
            }
        }
        if ara.sync(&s, force) {
            s.renders.send(Box::new(Rendered { segments: ara.segments(), ara: ara.renders }));
        }
        s.busy.fetch_sub(n, Ordering::AcqRel);
    }
}
