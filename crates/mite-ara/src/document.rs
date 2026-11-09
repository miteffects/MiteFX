//! The plug-in side of ARA 2 (docs/design/pitch-brief.md §1a): the factory, the document
//! controller and the plug-in instances' renderer and view roles, over a plain Rust model of the
//! host's document. The model keeps what Mite Pitch needs: audio sources (their audio read through
//! the host's audio reader on a worker, mixed to mono), audio modifications (an opaque state each,
//! what the archive keeps), playback regions (where a modification plays on the song timeline).
//! The plug-in polls [`Document::version`] and reads [`Document::snapshot`]; nothing here runs
//! on the audio thread.
//!
//! Every ARA object handed to the host is an id (never dereferenced by the host); the document
//! controller and plug-in instance refs are `Arc` pointers.

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::sys::*;

/// What the factory says about the plug-in (static strings, NUL-terminated).
pub struct FactoryInfo {
    pub plugin_name: &'static CStr,
    pub manufacturer: &'static CStr,
    pub url: &'static CStr,
    pub version: &'static CStr,
    /// Unique per plug-in, reverse-domain.
    pub factory_id: &'static CStr,
    /// The archives' format id: change it only with a migration.
    pub archive_id: &'static CStr,
}

struct Factory(ARAFactory);
// SAFETY: the factory only holds pointers to 'static data and functions.
unsafe impl Sync for Factory {}
unsafe impl Send for Factory {}

static FACTORY: OnceLock<Factory> = OnceLock::new();
static INFO: OnceLock<&'static FactoryInfo> = OnceLock::new();

/// The ARA factory (made on first call; later calls return the same one whatever `info`).
pub fn factory(info: &'static FactoryInfo) -> *const ARAFactory {
    let _ = INFO.set(info);
    let f = FACTORY.get_or_init(|| {
        let info = INFO.get().copied().unwrap_or(info);
        Factory(ARAFactory {
            structSize: std::mem::size_of::<ARAFactory>(),
            lowestSupportedApiGeneration: kARAAPIGeneration_2_0_Final as ARAAPIGeneration,
            highestSupportedApiGeneration: kARAAPIGeneration_2_0_Final as ARAAPIGeneration,
            factoryID: info.factory_id.as_ptr(),
            initializeARAWithConfiguration: Some(initialize),
            uninitializeARA: Some(uninitialize),
            plugInName: info.plugin_name.as_ptr(),
            manufacturerName: info.manufacturer.as_ptr(),
            informationURL: info.url.as_ptr(),
            version: info.version.as_ptr(),
            createDocumentControllerWithDocument: Some(create_document_controller),
            documentArchiveID: info.archive_id.as_ptr(),
            compatibleDocumentArchiveIDsCount: 0,
            compatibleDocumentArchiveIDs: std::ptr::null(),
            analyzeableContentTypesCount: 0,
            analyzeableContentTypes: std::ptr::null(),
            supportedPlaybackTransformationFlags: 0,
            supportsStoringAudioFileChunks: 0,
        })
    });
    &f.0
}

unsafe extern "C" fn initialize(_config: *const ARAInterfaceConfiguration) {}
unsafe extern "C" fn uninitialize() {}

// ---- the model ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSource {
    pub id: u64,
    pub name: String,
    pub persistent_id: String,
    pub sample_rate: f64,
    pub channels: u32,
    pub sample_count: i64,
    /// Its audio, mono, once read.
    pub audio: Option<Arc<Vec<f32>>>,
    host_ref: usize,
    access: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioModification {
    pub id: u64,
    pub source: u64,
    pub name: String,
    pub persistent_id: String,
    /// The plug-in's state for it (its edits): what the archive keeps.
    pub state: String,
    /// Whether it leaves its source as it is (no edits).
    pub preserving: bool,
    host_ref: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackRegion {
    pub id: u64,
    pub modification: u64,
    pub sequence: u64,
    pub name: String,
    /// Where it starts in its modification (= source) and how long, s.
    pub start_in_modification: f64,
    pub duration_in_modification: f64,
    /// Where it plays on the song timeline and how long, s.
    pub start_in_playback: f64,
    pub duration_in_playback: f64,
    host_ref: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub sources: BTreeMap<u64, AudioSource>,
    pub modifications: BTreeMap<u64, AudioModification>,
    pub regions: BTreeMap<u64, PlaybackRegion>,
    pub sequences: BTreeMap<u64, String>,
    contexts: BTreeMap<u64, String>,
    next: u64,
}

impl Model {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

fn string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: ARA strings are NUL-terminated UTF-8, valid for the call.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// The host's interfaces, copied at creation (plain pointers the host keeps valid for the
/// document controller's life).
#[derive(Clone, Copy)]
struct Host(ARADocumentControllerHostInstance);
// SAFETY: ARA allows the host interfaces to be called from the threads this module calls them on
// (audio readers on one worker each; the rest on the host's calling thread).
unsafe impl Send for Host {}
unsafe impl Sync for Host {}

/// A source's audio being read: the host's reader (made and destroyed on the model thread, as
/// ARA requires; only the reading runs on the worker), cancelled when access is withdrawn, the
/// reading done a chunk at a time under `busy` so withdrawing waits for the chunk in flight.
struct ReadJob {
    reader: usize,
    cancel: AtomicBool,
    done: AtomicBool,
    busy: Mutex<()>,
}

pub struct Document {
    host: Host,
    model: Mutex<Model>,
    /// Bumped on every change the plug-in may care about.
    pub version: AtomicU64,
    reads: Mutex<BTreeMap<u64, Arc<ReadJob>>>,
        /// Plug-in instances bound to this document.
    instances: Mutex<Vec<std::sync::Weak<Instance>>>,
    /// The document's data changed: tell the host in its next `notifyModelUpdates()` (the only
    /// place ARA allows it).
    dirty: AtomicBool,
    /// What one instance worked out for a modification's state (its render, say), for the
    /// others bound to the same document: a player instance takes the editor's render instead of
    /// rendering again. Keyed by modification, with the state it belongs to.
    shared: Mutex<BTreeMap<u64, (String, SharedValue)>>,
}

/// A value instances share through the document (see [`Document::share`]).
pub type SharedValue = Arc<dyn std::any::Any + Send + Sync>;

/// The chunk an audio reader reads at a time, frames.
const READ_CHUNK: i64 = 1 << 16;

impl Document {
    fn lock(&self) -> MutexGuard<'_, Model> {
        self.model.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// The document now.
    pub fn snapshot(&self) -> Model {
        self.lock().clone()
    }

    /// Sets an audio modification's state (the plug-in's edits for it) and whether it leaves its
    /// source as it is; tells the host the document changed (so it saves).
    pub fn set_state(&self, modification: u64, state: String, preserving: bool) {
        let changed = {
            let mut m = self.lock();
            match m.modifications.get_mut(&modification) {
                Some(md) if md.state != state || md.preserving != preserving => {
                    md.state = state;
                    md.preserving = preserving;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.touch();
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// Offers what this instance worked out for `modification` at `state` to the other instances
    /// (call before [`Self::set_state`] with the same state, so they find it when they look).
    pub fn share(&self, modification: u64, state: &str, value: SharedValue) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).insert(modification, (state.to_owned(), value));
    }

    /// What another instance shared for `modification` at exactly `state`.
    pub fn shared(&self, modification: u64, state: &str) -> Option<SharedValue> {
        let map = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&modification).filter(|(s, _)| s == state).map(|(_, v)| v.clone())
    }

    /// The host's `notifyModelUpdates()`: pending notifications go out (model thread).
    fn notify_model_updates(&self) {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let h = self.host.0;
        if h.modelUpdateControllerInterface.is_null() {
            return;
        }
        // SAFETY: the host's interface, called from inside its notifyModelUpdates().
        let mu: ARAModelUpdateControllerInterface = unsafe { std::ptr::read_unaligned(h.modelUpdateControllerInterface) };
        if let Some(f) = mu.notifyDocumentDataChanged {
            // SAFETY: as above.
            unsafe { f(h.modelUpdateControllerHostRef) };
        }
    }

    /// Asks the host to start or stop playing, to move to `seconds`, or to loop a range.
    pub fn request_play(&self, on: bool) {
        if let Some(p) = self.playback() {
            let f = if on { p.requestStartPlayback } else { p.requestStopPlayback };
            if let Some(f) = f {
                // SAFETY: the host's interface, with its own ref.
                unsafe { f(self.host.0.playbackControllerHostRef) };
            }
        }
    }

    pub fn request_position(&self, seconds: f64) {
        if let Some(f) = self.playback().and_then(|p| p.requestSetPlaybackPosition) {
            // SAFETY: as above.
            unsafe { f(self.host.0.playbackControllerHostRef, seconds) };
        }
    }

    pub fn request_cycle(&self, start: f64, duration: f64, on: bool) {
        let Some(p) = self.playback() else { return };
        if let Some(f) = p.requestSetCycleRange {
            // SAFETY: as above.
            unsafe { f(self.host.0.playbackControllerHostRef, start, duration) };
        }
        if let Some(f) = p.requestEnableCycle {
            // SAFETY: as above.
            unsafe { f(self.host.0.playbackControllerHostRef, ARABool::from(on)) };
        }
    }

    fn playback(&self) -> Option<ARAPlaybackControllerInterface> {
        let p = self.host.0.playbackControllerInterface;
        // SAFETY: a host interface pointer (null when the host offers no playback control).
        (!p.is_null()).then(|| unsafe { std::ptr::read_unaligned(p) })
    }

    fn audio_access(&self) -> Option<ARAAudioAccessControllerInterface> {
        let p = self.host.0.audioAccessControllerInterface;
        // SAFETY: a host interface pointer.
        (!p.is_null()).then(|| unsafe { std::ptr::read_unaligned(p) })
    }

    /// Starts reading a source's audio: the reader is made here (model thread), the reading
    /// runs on a worker.
    fn start_reading(self: &Arc<Self>, id: u64) {
        self.stop_reading(id);
        let (host_ref, channels, count) = {
            let m = self.lock();
            let Some(s) = m.sources.get(&id) else { return };
            (s.host_ref, s.channels.max(1) as usize, s.sample_count.max(0))
        };
        let Some(ac) = self.audio_access() else { return };
        let (Some(create), Some(read)) = (ac.createAudioReaderForSource, ac.readAudioSamples) else { return };
        let ctl = self.host.0.audioAccessControllerHostRef as usize;
        // SAFETY: the host's interface, on the model thread inside an audio source call or
        // endEditing (ARA's rule); 32-bit float samples.
        let reader = unsafe { create(ctl as ARAAudioAccessControllerHostRef, host_ref as ARAAudioSourceHostRef, 0) };
        if reader.is_null() {
            return;
        }
        let job = Arc::new(ReadJob { reader: reader as usize, cancel: AtomicBool::new(false), done: AtomicBool::new(false), busy: Mutex::new(()) });
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).insert(id, job.clone());
        let doc = Arc::clone(self);
        let _ = std::thread::Builder::new().name("mite-ara-reader".into()).spawn(move || {
            let mut mono = vec![0.0f32; count as usize];
            let mut bufs: Vec<Vec<f32>> = vec![vec![0.0f32; READ_CHUNK as usize]; channels];
            let mut pos = 0i64;
            let mut ok = true;
            while pos < count {
                let _g = job.busy.lock().unwrap_or_else(|e| e.into_inner());
                if job.cancel.load(Ordering::Acquire) {
                    ok = false;
                    break;
                }
                let n = READ_CHUNK.min(count - pos);
                let ptrs: Vec<*mut c_void> = bufs.iter_mut().map(|b| b.as_mut_ptr().cast()).collect();
                // SAFETY: reading may run on any non-realtime thread, one thread per reader; one
                // buffer of `n` floats per channel.
                if unsafe { read(ctl as ARAAudioAccessControllerHostRef, job.reader as ARAAudioReaderHostRef, pos, n, ptrs.as_ptr()) } == 0 {
                    ok = false;
                    break;
                }
                for (k, m) in mono[pos as usize..(pos + n) as usize].iter_mut().enumerate() {
                    *m = bufs.iter().map(|b| b[k]).sum::<f32>() / channels as f32;
                }
                pos += n;
            }
            if ok {
                if let Some(s) = doc.lock().sources.get_mut(&id) {
                    s.audio = Some(Arc::new(mono));
                }
                doc.touch();
            }
            job.done.store(true, Ordering::Release);
        });
    }

    /// Stops reading a source (waits for the chunk in flight) and destroys its reader (model
    /// thread).
    fn stop_reading(&self, id: u64) {
        let job = self.reads.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if let Some(job) = job {
            job.cancel.store(true, Ordering::Release);
            let _g = job.busy.lock().unwrap_or_else(|e| e.into_inner());
            self.destroy_reader(&job);
        }
    }

    fn destroy_reader(&self, job: &ReadJob) {
        if let Some(destroy) = self.audio_access().and_then(|ac| ac.destroyAudioReader) {
            // SAFETY: the reader we made, not in use (cancelled or finished), on the model thread.
            unsafe { destroy(self.host.0.audioAccessControllerHostRef, job.reader as ARAAudioReaderHostRef) };
        }
    }

    /// Destroys the readers of finished reads (from endEditing, on the model thread).
    fn reap_readers(&self) {
        let done: Vec<Arc<ReadJob>> = {
            let mut reads = self.reads.lock().unwrap_or_else(|e| e.into_inner());
            let ids: Vec<u64> = reads.iter().filter(|(_, j)| j.done.load(Ordering::Acquire)).map(|(id, _)| *id).collect();
            ids.into_iter().filter_map(|id| reads.remove(&id)).collect()
        };
        for job in done {
            let _g = job.busy.lock().unwrap_or_else(|e| e.into_inner());
            self.destroy_reader(&job);
        }
    }

    /// Waits until no source is being read (tests), at most `timeout`.
    pub fn wait_reads(&self, timeout: std::time::Duration) -> bool {
        let t0 = std::time::Instant::now();
        loop {
            let pending = {
                let m = self.lock();
                m.sources.values().any(|s| s.access && s.audio.is_none())
            };
            if !pending {
                return true;
            }
            if t0.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn archive_bytes(&self, only: Option<&[u64]>) -> Vec<u8> {
        let m = self.lock();
        let mods: serde_json::Map<String, serde_json::Value> = m
            .modifications
            .values()
            .filter(|md| only.is_none_or(|o| o.contains(&md.id)))
            .map(|md| (md.persistent_id.clone(), serde_json::Value::String(md.state.clone())))
            .collect();
        serde_json::json!({ "version": 1, "modifications": mods }).to_string().into_bytes()
    }

    /// Restores states from an archive: archived persistent ids map to current ones by `map`.
    fn restore_bytes(&self, bytes: &[u8], map: &BTreeMap<String, String>) -> bool {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { return false };
        let Some(mods) = v["modifications"].as_object() else { return false };
        {
            let mut m = self.lock();
            for (archived, state) in mods {
                let current = map.get(archived).unwrap_or(archived);
                if let Some(md) = m.modifications.values_mut().find(|md| &md.persistent_id == current) {
                    md.state = state.as_str().unwrap_or_default().to_owned();
                }
            }
        }
        self.touch();
        true
    }

    fn read_archive(&self, reader: ARAArchiveReaderHostRef) -> Option<Vec<u8>> {
        let h = self.host.0;
        // SAFETY: the host's archiving interface.
        let ai: ARAArchivingControllerInterface = unsafe { std::ptr::read_unaligned(h.archivingControllerInterface) };
        let (size, read) = (ai.getArchiveSize?, ai.readBytesFromArchive?);
        // SAFETY: as above.
        let n = unsafe { size(h.archivingControllerHostRef, reader) };
        let mut buf = vec![0u8; n];
        // SAFETY: `buf` holds `n` bytes.
        (n == 0 || unsafe { read(h.archivingControllerHostRef, reader, 0, n, buf.as_mut_ptr()) } != 0).then_some(buf)
    }

    fn write_archive(&self, writer: ARAArchiveWriterHostRef, bytes: &[u8]) -> bool {
        let h = self.host.0;
        // SAFETY: the host's archiving interface.
        let ai: ARAArchivingControllerInterface = unsafe { std::ptr::read_unaligned(h.archivingControllerInterface) };
        let Some(write) = ai.writeBytesToArchive else { return false };
        // SAFETY: `bytes` is valid for its length.
        unsafe { write(h.archivingControllerHostRef, writer, 0, bytes.len(), bytes.as_ptr()) != 0 }
    }

    fn notify_instances(&self) {
        let list = self.instances.lock().unwrap_or_else(|e| e.into_inner());
        for i in list.iter().filter_map(std::sync::Weak::upgrade) {
            i.version.fetch_add(1, Ordering::AcqRel);
        }
    }
}

// ---- the document controller's C interface ---------------------------------------------------

/// The document behind a ref (borrowed: the ref stays the host's until it destroys it).
///
/// # Safety
/// `r` must be a document controller ref this module made and the host has not destroyed.
pub unsafe fn document(r: ARADocumentControllerRef) -> Arc<Document> {
    let p = r as *const Document;
    // SAFETY: made by `Arc::into_raw` in `create_document_controller`; we add a count before
    // making a second `Arc` from it.
    unsafe {
        Arc::increment_strong_count(p);
        Arc::from_raw(p)
    }
}

fn id_of<T>(r: *mut T) -> u64 {
    r as usize as u64
}

fn ref_of<T>(id: u64) -> *mut T {
    id as usize as *mut T
}

struct Interface(ARADocumentControllerInterface);
// SAFETY: only function pointers.
unsafe impl Sync for Interface {}

static INTERFACE: Interface = Interface(ARADocumentControllerInterface {
    structSize: std::mem::size_of::<ARADocumentControllerInterface>(),
    destroyDocumentController: Some(dc_destroy),
    getFactory: Some(dc_get_factory),
    beginEditing: Some(dc_noop),
    endEditing: Some(dc_end_editing),
        notifyModelUpdates: Some(dc_notify),
    beginRestoringDocumentFromArchive: Some(dc_begin_restore),
    endRestoringDocumentFromArchive: Some(dc_end_restore),
    storeDocumentToArchive: Some(dc_store_document),
    updateDocumentProperties: Some(dc_update_document),
    createMusicalContext: Some(dc_create_context),
    updateMusicalContextProperties: Some(dc_update_context),
    updateMusicalContextContent: Some(dc_update_context_content),
    destroyMusicalContext: Some(dc_destroy_context),
    createAudioSource: Some(dc_create_source),
    updateAudioSourceProperties: Some(dc_update_source),
    updateAudioSourceContent: Some(dc_update_source_content),
    enableAudioSourceSamplesAccess: Some(dc_enable_access),
    deactivateAudioSourceForUndoHistory: Some(dc_deactivate_source),
    destroyAudioSource: Some(dc_destroy_source),
    createAudioModification: Some(dc_create_modification),
    cloneAudioModification: Some(dc_clone_modification),
    updateAudioModificationProperties: Some(dc_update_modification),
    deactivateAudioModificationForUndoHistory: Some(dc_deactivate_modification),
    destroyAudioModification: Some(dc_destroy_modification),
    createPlaybackRegion: Some(dc_create_region),
    updatePlaybackRegionProperties: Some(dc_update_region),
    destroyPlaybackRegion: Some(dc_destroy_region),
    isAudioSourceContentAvailable: Some(dc_source_content_available),
    isAudioSourceContentAnalysisIncomplete: Some(dc_source_analysis_incomplete),
    requestAudioSourceContentAnalysis: Some(dc_request_analysis),
    getAudioSourceContentGrade: Some(dc_source_grade),
    createAudioSourceContentReader: Some(dc_source_reader),
    isAudioModificationContentAvailable: Some(dc_modification_content_available),
    getAudioModificationContentGrade: Some(dc_modification_grade),
    createAudioModificationContentReader: Some(dc_modification_reader),
    isPlaybackRegionContentAvailable: Some(dc_region_content_available),
    getPlaybackRegionContentGrade: Some(dc_region_grade),
    createPlaybackRegionContentReader: Some(dc_region_reader),
    getContentReaderEventCount: Some(dc_reader_count),
    getContentReaderDataForEvent: Some(dc_reader_data),
    destroyContentReader: Some(dc_destroy_reader),
    createRegionSequence: Some(dc_create_sequence),
    updateRegionSequenceProperties: Some(dc_update_sequence),
    destroyRegionSequence: Some(dc_destroy_sequence),
    getPlaybackRegionHeadAndTailTime: Some(dc_head_tail),
    restoreObjectsFromArchive: Some(dc_restore_objects),
    storeObjectsToArchive: Some(dc_store_objects),
    getProcessingAlgorithmsCount: Some(dc_algorithms_count),
    getProcessingAlgorithmProperties: Some(dc_algorithm_properties),
    getProcessingAlgorithmForAudioSource: Some(dc_algorithm_for_source),
    requestProcessingAlgorithmForAudioSource: Some(dc_request_algorithm),
    isLicensedForCapabilities: Some(dc_licensed),
    storeAudioSourceToAudioFileChunk: Some(dc_store_chunk),
    isAudioModificationPreservingAudioSourceSignal: Some(dc_preserving),
});

unsafe extern "C" fn create_document_controller(host: *const ARADocumentControllerHostInstance, _props: *const ARADocumentProperties) -> *const ARADocumentControllerInstance {
    if host.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the host passes a valid host instance.
    let host = Host(unsafe { std::ptr::read_unaligned(host) });
    let doc = Arc::new(Document { host, model: Mutex::new(Model::default()), version: AtomicU64::new(0), reads: Mutex::new(BTreeMap::new()), instances: Mutex::new(Vec::new()), dirty: AtomicBool::new(false), shared: Mutex::new(BTreeMap::new()) });
    let r = Arc::into_raw(doc) as ARADocumentControllerRef;
    // The instance struct lives as long as the document (freed in `dc_destroy`).
    let inst = Box::new(ARADocumentControllerInstance { structSize: std::mem::size_of::<ARADocumentControllerInstance>(), documentControllerRef: r, documentControllerInterface: &INTERFACE.0 });
    let p = Box::into_raw(inst);
    INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).insert(r as usize, p as usize);
    p
}

/// The leaked instance structs, by document ref (to free on destroy).
static INSTANCES: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());

macro_rules! doc {
    ($r:expr) => {
        // SAFETY: the host passes refs we made and has not destroyed.
        unsafe { document($r) }
    };
}

unsafe extern "C" fn dc_destroy(r: ARADocumentControllerRef) {
    let d = doc!(r);
    let ids: Vec<u64> = d.lock().sources.keys().copied().collect();
    for id in ids {
        d.stop_reading(id);
    }
    drop(d);
    if let Some(p) = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).remove(&(r as usize)) {
        // SAFETY: made by `Box::into_raw` in `create_document_controller`.
        drop(unsafe { Box::from_raw(p as *mut ARADocumentControllerInstance) });
    }
    // SAFETY: the count `create_document_controller` gave the host.
    drop(unsafe { Arc::from_raw(r as *const Document) });
}

unsafe extern "C" fn dc_get_factory(_r: ARADocumentControllerRef) -> *const ARAFactory {
    FACTORY.get().map_or(std::ptr::null(), |f| &f.0 as *const ARAFactory)
}

unsafe extern "C" fn dc_noop(_r: ARADocumentControllerRef) {}

unsafe extern "C" fn dc_notify(r: ARADocumentControllerRef) {
    doc!(r).notify_model_updates();
}

unsafe extern "C" fn dc_end_editing(r: ARADocumentControllerRef) {
    let d = doc!(r);
    d.reap_readers();
    d.touch();
    d.notify_instances();
}

unsafe extern "C" fn dc_begin_restore(_r: ARADocumentControllerRef, _reader: ARAArchiveReaderHostRef) -> ARABool {
    1
}

unsafe extern "C" fn dc_end_restore(r: ARADocumentControllerRef, reader: ARAArchiveReaderHostRef) -> ARABool {
    let d = doc!(r);
    let Some(bytes) = d.read_archive(reader) else { return 0 };
    ARABool::from(bytes.is_empty() || d.restore_bytes(&bytes, &BTreeMap::new()))
}

unsafe extern "C" fn dc_store_document(r: ARADocumentControllerRef, writer: ARAArchiveWriterHostRef) -> ARABool {
    let d = doc!(r);
    let bytes = d.archive_bytes(None);
    ARABool::from(d.write_archive(writer, &bytes))
}

unsafe extern "C" fn dc_update_document(_r: ARADocumentControllerRef, _p: *const ARADocumentProperties) {}

unsafe extern "C" fn dc_create_context(r: ARADocumentControllerRef, _h: ARAMusicalContextHostRef, p: *const ARAMusicalContextProperties) -> ARAMusicalContextRef {
    let d = doc!(r);
    let mut m = d.lock();
    let id = m.id();
    // SAFETY: valid properties for the call.
    let name = if p.is_null() { String::new() } else { string(unsafe { std::ptr::read_unaligned(p) }.name) };
    m.contexts.insert(id, name);
    ref_of(id)
}

unsafe extern "C" fn dc_update_context(_r: ARADocumentControllerRef, _c: ARAMusicalContextRef, _p: *const ARAMusicalContextProperties) {}
unsafe extern "C" fn dc_update_context_content(_r: ARADocumentControllerRef, _c: ARAMusicalContextRef, _t: *const ARAContentTimeRange, _f: ARAContentUpdateFlags) {}

unsafe extern "C" fn dc_destroy_context(r: ARADocumentControllerRef, c: ARAMusicalContextRef) {
    doc!(r).lock().contexts.remove(&id_of(c));
}

fn source_props(p: *const ARAAudioSourceProperties) -> Option<ARAAudioSourceProperties> {
    // SAFETY: valid properties for the call.
    (!p.is_null()).then(|| unsafe { std::ptr::read_unaligned(p) })
}

unsafe extern "C" fn dc_create_source(r: ARADocumentControllerRef, h: ARAAudioSourceHostRef, p: *const ARAAudioSourceProperties) -> ARAAudioSourceRef {
    let d = doc!(r);
    let Some(p) = source_props(p) else { return std::ptr::null_mut() };
    let mut m = d.lock();
    let id = m.id();
    m.sources.insert(id, AudioSource { id, name: string(p.name), persistent_id: string(p.persistentID), sample_rate: p.sampleRate, channels: p.channelCount.max(1) as u32, sample_count: p.sampleCount, audio: None, host_ref: h as usize, access: false });
    drop(m);
    d.touch();
    ref_of(id)
}

unsafe extern "C" fn dc_update_source(r: ARADocumentControllerRef, s: ARAAudioSourceRef, p: *const ARAAudioSourceProperties) {
    let d = doc!(r);
    let Some(p) = source_props(p) else { return };
    let mut m = d.lock();
    if let Some(src) = m.sources.get_mut(&id_of(s)) {
        src.name = string(p.name);
        src.persistent_id = string(p.persistentID);
        // New audio format: what was read is stale (access is off while properties change).
        if src.sample_count != p.sampleCount || src.sample_rate != p.sampleRate || src.channels != p.channelCount.max(1) as u32 {
            src.audio = None;
        }
        src.sample_count = p.sampleCount;
        src.sample_rate = p.sampleRate;
        src.channels = p.channelCount.max(1) as u32;
    }
    drop(m);
    d.touch();
}

unsafe extern "C" fn dc_update_source_content(r: ARADocumentControllerRef, s: ARAAudioSourceRef, _t: *const ARAContentTimeRange, flags: ARAContentUpdateFlags) {
    let d = doc!(r);
    // The samples changed: read them again (if access is on).
    if flags & (kARAContentUpdateSignalScopeRemainsUnchanged as ARAContentUpdateFlags) == 0 {
        let id = id_of(s);
        let on = {
            let mut m = d.lock();
            m.sources.get_mut(&id).map(|src| {
                src.audio = None;
                src.access
            })
        };
        if on == Some(true) {
            d.stop_reading(id);
            d.start_reading(id);
        }
        d.touch();
    }
}

unsafe extern "C" fn dc_enable_access(r: ARADocumentControllerRef, s: ARAAudioSourceRef, enable: ARABool) {
    let d = doc!(r);
    let id = id_of(s);
    let need = {
        let mut m = d.lock();
        let Some(src) = m.sources.get_mut(&id) else { return };
        src.access = enable != 0;
        enable != 0 && src.audio.is_none()
    };
    if enable == 0 {
        d.stop_reading(id);
    } else if need {
        d.start_reading(id);
    }
}

unsafe extern "C" fn dc_deactivate_source(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _d: ARABool) {}

unsafe extern "C" fn dc_destroy_source(r: ARADocumentControllerRef, s: ARAAudioSourceRef) {
    let d = doc!(r);
    d.stop_reading(id_of(s));
    d.lock().sources.remove(&id_of(s));
    d.touch();
}

fn modification_props(p: *const ARAAudioModificationProperties) -> (String, String) {
    if p.is_null() {
        return (String::new(), String::new());
    }
    // SAFETY: valid properties for the call.
    let p = unsafe { std::ptr::read_unaligned(p) };
    (string(p.name), string(p.persistentID))
}

unsafe extern "C" fn dc_create_modification(r: ARADocumentControllerRef, s: ARAAudioSourceRef, h: ARAAudioModificationHostRef, p: *const ARAAudioModificationProperties) -> ARAAudioModificationRef {
    let d = doc!(r);
    let (name, persistent_id) = modification_props(p);
    let mut m = d.lock();
    let id = m.id();
    m.modifications.insert(id, AudioModification { id, source: id_of(s), name, persistent_id, state: String::new(), preserving: true, host_ref: h as usize });
    drop(m);
    d.touch();
    ref_of(id)
}

unsafe extern "C" fn dc_clone_modification(r: ARADocumentControllerRef, src: ARAAudioModificationRef, h: ARAAudioModificationHostRef, p: *const ARAAudioModificationProperties) -> ARAAudioModificationRef {
    let d = doc!(r);
    let (name, persistent_id) = modification_props(p);
    let mut m = d.lock();
    let Some(orig) = m.modifications.get(&id_of(src)).cloned() else { return std::ptr::null_mut() };
    let id = m.id();
    m.modifications.insert(id, AudioModification { id, name, persistent_id, host_ref: h as usize, ..orig });
    drop(m);
    d.touch();
    ref_of(id)
}

unsafe extern "C" fn dc_update_modification(r: ARADocumentControllerRef, md: ARAAudioModificationRef, p: *const ARAAudioModificationProperties) {
    let d = doc!(r);
    let (name, persistent_id) = modification_props(p);
    if let Some(x) = d.lock().modifications.get_mut(&id_of(md)) {
        x.name = name;
        x.persistent_id = persistent_id;
    }
    d.touch();
}

unsafe extern "C" fn dc_deactivate_modification(_r: ARADocumentControllerRef, _m: ARAAudioModificationRef, _d: ARABool) {}

unsafe extern "C" fn dc_destroy_modification(r: ARADocumentControllerRef, md: ARAAudioModificationRef) {
    let d = doc!(r);
    d.lock().modifications.remove(&id_of(md));
    d.shared.lock().unwrap_or_else(|e| e.into_inner()).remove(&id_of(md));
    d.touch();
}

fn region_props(p: *const ARAPlaybackRegionProperties) -> Option<ARAPlaybackRegionProperties> {
    // SAFETY: valid properties for the call.
    (!p.is_null()).then(|| unsafe { std::ptr::read_unaligned(p) })
}

unsafe extern "C" fn dc_create_region(r: ARADocumentControllerRef, md: ARAAudioModificationRef, h: ARAPlaybackRegionHostRef, p: *const ARAPlaybackRegionProperties) -> ARAPlaybackRegionRef {
    let d = doc!(r);
    let Some(p) = region_props(p) else { return std::ptr::null_mut() };
    let mut m = d.lock();
    let id = m.id();
    m.regions.insert(id, PlaybackRegion { id, modification: id_of(md), sequence: id_of(p.regionSequenceRef), name: string(p.name), start_in_modification: p.startInModificationTime, duration_in_modification: p.durationInModificationTime, start_in_playback: p.startInPlaybackTime, duration_in_playback: p.durationInPlaybackTime, host_ref: h as usize });
    drop(m);
    d.touch();
    ref_of(id)
}

unsafe extern "C" fn dc_update_region(r: ARADocumentControllerRef, rg: ARAPlaybackRegionRef, p: *const ARAPlaybackRegionProperties) {
    let d = doc!(r);
    let Some(p) = region_props(p) else { return };
    if let Some(x) = d.lock().regions.get_mut(&id_of(rg)) {
        x.sequence = id_of(p.regionSequenceRef);
        x.name = string(p.name);
        x.start_in_modification = p.startInModificationTime;
        x.duration_in_modification = p.durationInModificationTime;
        x.start_in_playback = p.startInPlaybackTime;
        x.duration_in_playback = p.durationInPlaybackTime;
    }
    d.touch();
}

unsafe extern "C" fn dc_destroy_region(r: ARADocumentControllerRef, rg: ARAPlaybackRegionRef) {
    let d = doc!(r);
    d.lock().regions.remove(&id_of(rg));
    d.touch();
}

// No content (notes, tempo) is offered to the host yet.
unsafe extern "C" fn dc_source_content_available(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _t: ARAContentType) -> ARABool {
    0
}
unsafe extern "C" fn dc_source_analysis_incomplete(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _t: ARAContentType) -> ARABool {
    0
}
unsafe extern "C" fn dc_request_analysis(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _n: ARASize, _t: *const ARAContentType) {}
unsafe extern "C" fn dc_source_grade(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _t: ARAContentType) -> ARAContentGrade {
    kARAContentGradeInitial as ARAContentGrade
}
unsafe extern "C" fn dc_source_reader(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _t: ARAContentType, _range: *const ARAContentTimeRange) -> ARAContentReaderRef {
    std::ptr::null_mut()
}
unsafe extern "C" fn dc_modification_content_available(_r: ARADocumentControllerRef, _m: ARAAudioModificationRef, _t: ARAContentType) -> ARABool {
    0
}
unsafe extern "C" fn dc_modification_grade(_r: ARADocumentControllerRef, _m: ARAAudioModificationRef, _t: ARAContentType) -> ARAContentGrade {
    kARAContentGradeInitial as ARAContentGrade
}
unsafe extern "C" fn dc_modification_reader(_r: ARADocumentControllerRef, _m: ARAAudioModificationRef, _t: ARAContentType, _range: *const ARAContentTimeRange) -> ARAContentReaderRef {
    std::ptr::null_mut()
}
unsafe extern "C" fn dc_region_content_available(_r: ARADocumentControllerRef, _p: ARAPlaybackRegionRef, _t: ARAContentType) -> ARABool {
    0
}
unsafe extern "C" fn dc_region_grade(_r: ARADocumentControllerRef, _p: ARAPlaybackRegionRef, _t: ARAContentType) -> ARAContentGrade {
    kARAContentGradeInitial as ARAContentGrade
}
unsafe extern "C" fn dc_region_reader(_r: ARADocumentControllerRef, _p: ARAPlaybackRegionRef, _t: ARAContentType, _range: *const ARAContentTimeRange) -> ARAContentReaderRef {
    std::ptr::null_mut()
}
unsafe extern "C" fn dc_reader_count(_r: ARADocumentControllerRef, _c: ARAContentReaderRef) -> ARAInt32 {
    0
}
unsafe extern "C" fn dc_reader_data(_r: ARADocumentControllerRef, _c: ARAContentReaderRef, _i: ARAInt32) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn dc_destroy_reader(_r: ARADocumentControllerRef, _c: ARAContentReaderRef) {}

unsafe extern "C" fn dc_create_sequence(r: ARADocumentControllerRef, _h: ARARegionSequenceHostRef, p: *const ARARegionSequenceProperties) -> ARARegionSequenceRef {
    let d = doc!(r);
    // SAFETY: valid properties for the call.
    let name = if p.is_null() { String::new() } else { string(unsafe { std::ptr::read_unaligned(p) }.name) };
    let mut m = d.lock();
    let id = m.id();
    m.sequences.insert(id, name);
    ref_of(id)
}

unsafe extern "C" fn dc_update_sequence(r: ARADocumentControllerRef, s: ARARegionSequenceRef, p: *const ARARegionSequenceProperties) {
    let d = doc!(r);
    // SAFETY: valid properties for the call.
    let name = if p.is_null() { String::new() } else { string(unsafe { std::ptr::read_unaligned(p) }.name) };
    if let Some(x) = d.lock().sequences.get_mut(&id_of(s)) {
        *x = name;
    }
    d.touch();
}

unsafe extern "C" fn dc_destroy_sequence(r: ARADocumentControllerRef, s: ARARegionSequenceRef) {
    doc!(r).lock().sequences.remove(&id_of(s));
}

unsafe extern "C" fn dc_head_tail(_r: ARADocumentControllerRef, _p: ARAPlaybackRegionRef, head: *mut ARATimeDuration, tail: *mut ARATimeDuration) {
    // SAFETY: the host passes writable pointers.
    unsafe {
        if !head.is_null() {
            *head = 0.0;
        }
        if !tail.is_null() {
            *tail = 0.0;
        }
    }
}

/// `count` C strings from the host, as Rust strings.
fn strings(count: ARASize, p: *const ARAPersistentID) -> Vec<String> {
    if p.is_null() {
        return Vec::new();
    }
    // SAFETY: the host passes `count` valid strings.
    (0..count).map(|i| string(unsafe { *p.add(i) })).collect()
}

unsafe extern "C" fn dc_restore_objects(r: ARADocumentControllerRef, reader: ARAArchiveReaderHostRef, filter: *const ARARestoreObjectsFilter) -> ARABool {
    let d = doc!(r);
    let Some(bytes) = d.read_archive(reader) else { return 0 };
    let mut map = BTreeMap::new();
    if !filter.is_null() {
        // SAFETY: a valid filter for the call.
        let f = unsafe { std::ptr::read_unaligned(filter) };
        let (a, c) = (strings(f.audioModificationIDsCount, f.audioModificationArchiveIDs), strings(f.audioModificationIDsCount, f.audioModificationCurrentIDs));
        map.extend(a.into_iter().zip(c));
    }
    ARABool::from(bytes.is_empty() || d.restore_bytes(&bytes, &map))
}

unsafe extern "C" fn dc_store_objects(r: ARADocumentControllerRef, writer: ARAArchiveWriterHostRef, filter: *const ARAStoreObjectsFilter) -> ARABool {
    let d = doc!(r);
    let only: Option<Vec<u64>> = (!filter.is_null()).then(|| {
        // SAFETY: a valid filter for the call.
        let f = unsafe { std::ptr::read_unaligned(filter) };
        if f.audioModificationRefs.is_null() {
            Vec::new()
        } else {
            // SAFETY: `count` refs.
            (0..f.audioModificationRefsCount).map(|i| id_of(unsafe { *f.audioModificationRefs.add(i) })).collect()
        }
    });
    let bytes = d.archive_bytes(only.as_deref());
    ARABool::from(d.write_archive(writer, &bytes))
}

unsafe extern "C" fn dc_algorithms_count(_r: ARADocumentControllerRef) -> ARAInt32 {
    0
}
unsafe extern "C" fn dc_algorithm_properties(_r: ARADocumentControllerRef, _i: ARAInt32) -> *const ARAProcessingAlgorithmProperties {
    std::ptr::null()
}
unsafe extern "C" fn dc_algorithm_for_source(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef) -> ARAInt32 {
    0
}
unsafe extern "C" fn dc_request_algorithm(_r: ARADocumentControllerRef, _s: ARAAudioSourceRef, _i: ARAInt32) {}
unsafe extern "C" fn dc_licensed(_r: ARADocumentControllerRef, _a: ARABool, _n: ARASize, _t: *const ARAContentType, _f: ARAPlaybackTransformationFlags) -> ARABool {
    1
}
unsafe extern "C" fn dc_store_chunk(_r: ARADocumentControllerRef, _w: ARAArchiveWriterHostRef, _s: ARAAudioSourceRef, _id: *mut ARAPersistentID, _o: *mut ARABool) -> ARABool {
    0
}

unsafe extern "C" fn dc_preserving(r: ARADocumentControllerRef, md: ARAAudioModificationRef) -> ARABool {
    ARABool::from(doc!(r).lock().modifications.get(&id_of(md)).is_none_or(|m| m.preserving))
}

// ---- plug-in instances ----------------------------------------------------------------------

/// A plug-in instance bound to a document: the playback regions it renders (playback renderer
/// role), those it shows (editor renderer), the host's selection (editor view).
pub struct Instance {
    pub document: Arc<Document>,
    pub roles: ARAPlugInInstanceRoleFlags,
    /// Bumped on every change to these lists (and on every document edit cycle).
    pub version: AtomicU64,
    state: Mutex<InstanceState>,
    c: Mutex<Option<ExtBox>>,
}

/// The extension struct handed to the host (pointers to statics and to its instance).
struct ExtBox(Box<ARAPlugInExtensionInstance>);
// SAFETY: only read by the host; it points at statics and at the instance that owns it.
unsafe impl Send for ExtBox {}
unsafe impl Sync for ExtBox {}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstanceState {
    /// Playback regions rendered by this instance.
    pub playback: Vec<u64>,
    /// Playback regions shown by this instance's editor.
    pub editor: Vec<u64>,
        /// The host's selection (playback regions).
    pub selection: Vec<u64>,
    /// Region sequences (tracks) shown by this instance's editor, and those selected.
    pub sequences: Vec<u64>,
    pub selected_sequences: Vec<u64>,
}

impl Instance {
    pub fn state(&self) -> InstanceState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn edit(&self, f: impl FnOnce(&mut InstanceState)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// The C struct the host is handed (valid while this instance lives).
    pub fn extension(&self) -> *const ARAPlugInExtensionInstance {
        self.c.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(std::ptr::null(), |p| &*p.0 as *const _)
    }
}

/// Binds a plug-in instance to the document controller `r` (from the CLAP plug-in extension's
/// `bind_to_document_controller`). The returned instance must outlive the host's use of its
/// extension struct (the plug-in instance keeps it).
///
/// # Safety
/// `r` must be a live document controller ref made by this module.
pub unsafe fn bind(r: ARADocumentControllerRef, _known: ARAPlugInInstanceRoleFlags, assigned: ARAPlugInInstanceRoleFlags) -> Arc<Instance> {
    // SAFETY: as the caller promises.
    let document = unsafe { document(r) };
    let inst = Arc::new(Instance { document: document.clone(), roles: assigned, version: AtomicU64::new(0), state: Mutex::new(InstanceState::default()), c: Mutex::new(None) });
        let me = Arc::as_ptr(&inst) as *mut c_void;
    // Every role's interface, whatever the host assigned: hosts use them anyway (REAPER, assigning
    // only the editor roles, calls the playback renderer and crashes on a null one, 2026-11-05),
    // and ARA's own library always fills all three.
    let has = |_role: u32| true;
    let c = Box::new(ARAPlugInExtensionInstance {
        structSize: std::mem::size_of::<ARAPlugInExtensionInstance>(),
        plugInExtensionRef: me.cast(),
        plugInExtensionInterface: &EXTENSION.0,
        playbackRendererRef: if has(kARAPlaybackRendererRole) { me.cast() } else { std::ptr::null_mut() },
        playbackRendererInterface: if has(kARAPlaybackRendererRole) { &PLAYBACK.0 } else { std::ptr::null() },
        editorRendererRef: if has(kARAEditorRendererRole) { me.cast() } else { std::ptr::null_mut() },
        editorRendererInterface: if has(kARAEditorRendererRole) { &EDITOR.0 } else { std::ptr::null() },
        editorViewRef: if has(kARAEditorViewRole) { me.cast() } else { std::ptr::null_mut() },
        editorViewInterface: if has(kARAEditorViewRole) { &VIEW.0 } else { std::ptr::null() },
    });
    *inst.c.lock().unwrap_or_else(|e| e.into_inner()) = Some(ExtBox(c));
    document.instances.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&inst));
    inst
}

/// The instance behind a renderer or view ref (borrowed for the call).
///
/// # Safety
/// `r` must point at a live [`Instance`] (the plug-in keeps it alive while the host may call).
unsafe fn instance<'a, T>(r: *mut T) -> &'a Instance {
    // SAFETY: as the caller promises.
    unsafe { &*(r as *const Instance) }
}

struct Ext(ARAPlugInExtensionInterface);
unsafe impl Sync for Ext {}
static EXTENSION: Ext = Ext(ARAPlugInExtensionInterface { structSize: std::mem::size_of::<ARAPlugInExtensionInterface>(), setPlaybackRegion: Some(ext_set_region), removePlaybackRegion: Some(ext_remove_region) });

struct Playback(ARAPlaybackRendererInterface);
unsafe impl Sync for Playback {}
static PLAYBACK: Playback = Playback(ARAPlaybackRendererInterface { structSize: std::mem::size_of::<ARAPlaybackRendererInterface>(), addPlaybackRegion: Some(pb_add), removePlaybackRegion: Some(pb_remove) });

struct Editor(ARAEditorRendererInterface);
unsafe impl Sync for Editor {}
static EDITOR: Editor = Editor(ARAEditorRendererInterface { structSize: std::mem::size_of::<ARAEditorRendererInterface>(), addPlaybackRegion: Some(ed_add), removePlaybackRegion: Some(ed_remove), addRegionSequence: Some(ed_add_sequence), removeRegionSequence: Some(ed_remove_sequence) });

struct View(ARAEditorViewInterface);
unsafe impl Sync for View {}
static VIEW: View = View(ARAEditorViewInterface { structSize: std::mem::size_of::<ARAEditorViewInterface>(), notifySelection: Some(view_selection), notifyHideRegionSequences: Some(view_hide) });

// ARA 1's single-region binding (deprecated; some hosts still call it): as the playback role.
unsafe extern "C" fn ext_set_region(r: ARAPlugInExtensionRef, p: ARAPlaybackRegionRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| {
        s.playback = vec![id_of(p)];
        s.editor = vec![id_of(p)];
    });
}
unsafe extern "C" fn ext_remove_region(r: ARAPlugInExtensionRef, p: ARAPlaybackRegionRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| {
        s.playback.retain(|&x| x != id_of(p));
        s.editor.retain(|&x| x != id_of(p));
    });
}
unsafe extern "C" fn pb_add(r: ARAPlaybackRendererRef, p: ARAPlaybackRegionRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| s.playback.push(id_of(p)));
}
unsafe extern "C" fn pb_remove(r: ARAPlaybackRendererRef, p: ARAPlaybackRegionRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| s.playback.retain(|&x| x != id_of(p)));
}
unsafe extern "C" fn ed_add(r: ARAEditorRendererRef, p: ARAPlaybackRegionRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| s.editor.push(id_of(p)));
}
unsafe extern "C" fn ed_remove(r: ARAEditorRendererRef, p: ARAPlaybackRegionRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| s.editor.retain(|&x| x != id_of(p)));
}
unsafe extern "C" fn ed_add_sequence(r: ARAEditorRendererRef, q: ARARegionSequenceRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| s.sequences.push(id_of(q)));
}
unsafe extern "C" fn ed_remove_sequence(r: ARAEditorRendererRef, q: ARARegionSequenceRef) {
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|s| s.sequences.retain(|&x| x != id_of(q)));
}
unsafe extern "C" fn view_selection(r: ARAEditorViewRef, sel: *const ARAViewSelection) {
    if sel.is_null() {
        return;
    }
    // SAFETY: a valid selection for the call.
    let s = unsafe { std::ptr::read_unaligned(sel) };
    let ids: Vec<u64> = if s.playbackRegionRefs.is_null() {
        Vec::new()
    } else {
        // SAFETY: `count` refs.
        (0..s.playbackRegionRefsCount).map(|i| id_of(unsafe { *s.playbackRegionRefs.add(i) })).collect()
    };
        let seqs: Vec<u64> = if s.regionSequenceRefs.is_null() {
        Vec::new()
    } else {
        // SAFETY: `count` refs.
        (0..s.regionSequenceRefsCount).map(|i| id_of(unsafe { *s.regionSequenceRefs.add(i) })).collect()
    };
    // SAFETY: refs we made.
    unsafe { instance(r) }.edit(|st| {
        st.selection = ids;
        st.selected_sequences = seqs;
    });
}
unsafe extern "C" fn view_hide(_r: ARAEditorViewRef, _n: ARASize, _s: *const ARARegionSequenceRef) {}

/// Keeps a C string alive for as long as the program (for ids given to the host).
pub fn leak(s: &str) -> &'static CStr {
    Box::leak(CString::new(s).unwrap_or_default().into_boxed_c_str())
}
