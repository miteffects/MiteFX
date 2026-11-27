//! A host in Rust for tests: the host side of ARA's interfaces (audio access from memory, archives
//! in memory, transport requests and model updates recorded), and a document built through a
//! plug-in's factory, as REAPER would build one. Test-only, but compiled always so plug-in tests
//! can use it.
#![allow(clippy::missing_safety_doc, clippy::not_unsafe_ptr_arg_deref, clippy::undocumented_unsafe_blocks)]

use std::ffi::{c_void, CStr};
use std::sync::Mutex;

use crate::sys::*;

/// The source's audio: two channels.
pub struct Audio {
    pub fs: f64,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

/// The thread the document is driven from (ARA's model thread), and whether the host is inside
/// `notifyModelUpdates()`: calls the rules forbid elsewhere are recorded as violations.
/// Per host: its model thread (the audio access controller's host ref points at it).
struct HostCtx {
    model: std::thread::ThreadId,
}

thread_local! {
    static IN_NOTIFY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn on_model_thread(ctx: ARAAudioAccessControllerHostRef, what: &str) {
    let ctx = ctx as *const HostCtx;
    if !ctx.is_null() && unsafe { (*ctx).model } != std::thread::current().id() {
        call(format!("VIOLATION: {what} off the model thread"));
    }
}

/// The ARA threading rules broken so far (empty: none).
pub fn violations() -> Vec<String> {
    calls().into_iter().filter(|c| c.starts_with("VIOLATION")).collect()
}

unsafe extern "C" fn create_reader(c: ARAAudioAccessControllerHostRef, s: ARAAudioSourceHostRef, _b64: ARABool) -> ARAAudioReaderHostRef {
    on_model_thread(c, "createAudioReaderForSource");
    s.cast()
}
unsafe extern "C" fn read_samples(_c: ARAAudioAccessControllerHostRef, reader: ARAAudioReaderHostRef, pos: ARASamplePosition, n: ARASampleCount, bufs: *const *mut c_void) -> ARABool {
    let a = unsafe { &*(reader as *const Audio) };
    for (ch, src) in [&a.l, &a.r].into_iter().enumerate() {
        let out = unsafe { std::slice::from_raw_parts_mut(*bufs.add(ch) as *mut f32, n as usize) };
        out.copy_from_slice(&src[pos as usize..(pos + n) as usize]);
    }
    1
}
unsafe extern "C" fn destroy_reader(c: ARAAudioAccessControllerHostRef, _r: ARAAudioReaderHostRef) {
    on_model_thread(c, "destroyAudioReader");
}

static AUDIO_ACCESS: ARAAudioAccessControllerInterface = ARAAudioAccessControllerInterface { structSize: std::mem::size_of::<ARAAudioAccessControllerInterface>(), createAudioReaderForSource: Some(create_reader), readAudioSamples: Some(read_samples), destroyAudioReader: Some(destroy_reader) };

unsafe extern "C" fn archive_size(_c: ARAArchivingControllerHostRef, r: ARAArchiveReaderHostRef) -> ARASize {
    unsafe { &*(r as *const Vec<u8>) }.len()
}
unsafe extern "C" fn archive_read(_c: ARAArchivingControllerHostRef, r: ARAArchiveReaderHostRef, pos: ARASize, n: ARASize, out: *mut ARAByte) -> ARABool {
    let v = unsafe { &*(r as *const Vec<u8>) };
    unsafe { std::ptr::copy_nonoverlapping(v[pos..pos + n].as_ptr(), out, n) };
    1
}
unsafe extern "C" fn archive_write(_c: ARAArchivingControllerHostRef, w: ARAArchiveWriterHostRef, pos: ARASize, n: ARASize, data: *const ARAByte) -> ARABool {
    let v = unsafe { &mut *(w as *mut Vec<u8>) };
    if v.len() < pos + n {
        v.resize(pos + n, 0);
    }
    unsafe { std::ptr::copy_nonoverlapping(data, v[pos..].as_mut_ptr(), n) };
    1
}
unsafe extern "C" fn archive_progress(_c: ARAArchivingControllerHostRef, _p: f32) {}
unsafe extern "C" fn archive_id(_c: ARAArchivingControllerHostRef, _r: ARAArchiveReaderHostRef) -> ARAPersistentID {
    c"".as_ptr()
}

static ARCHIVING: ARAArchivingControllerInterface = ARAArchivingControllerInterface { structSize: std::mem::size_of::<ARAArchivingControllerInterface>(), getArchiveSize: Some(archive_size), readBytesFromArchive: Some(archive_read), writeBytesToArchive: Some(archive_write), notifyDocumentArchivingProgress: Some(archive_progress), notifyDocumentUnarchivingProgress: Some(archive_progress), getDocumentArchiveID: Some(archive_id) };

/// The song's time as the test host gives it (the musical context's host ref points at it).
#[derive(Default)]
struct SongTime {
    tempo: Vec<ARAContentTempoEntry>,
    bars: Vec<ARAContentBarSignature>,
}

/// A content reader: a copy of the events of one type, each as raw bytes.
struct Events {
    size: usize,
    bytes: Vec<u8>,
}

fn song_time(ctx: ARAMusicalContextHostRef) -> &'static Mutex<SongTime> {
    unsafe { &*(ctx as *const Mutex<SongTime>) }
}

unsafe extern "C" fn ca_available(c: ARAContentAccessControllerHostRef, ctx: ARAMusicalContextHostRef, kind: ARAContentType) -> ARABool {
    on_model_thread(c.cast(), "isMusicalContextContentAvailable");
    let t = song_time(ctx).lock().unwrap();
    let kind = kind as u32;
    ARABool::from(if kind == kARAContentTypeTempoEntries {
        t.tempo.len() >= 2
    } else if kind == kARAContentTypeBarSignatures {
        !t.bars.is_empty()
    } else {
        false
    })
}
unsafe extern "C" fn ca_grade(_c: ARAContentAccessControllerHostRef, _ctx: ARAMusicalContextHostRef, _k: ARAContentType) -> ARAContentGrade {
    kARAContentGradeAdjusted as ARAContentGrade
}
unsafe extern "C" fn ca_create(c: ARAContentAccessControllerHostRef, ctx: ARAMusicalContextHostRef, kind: ARAContentType, _r: *const ARAContentTimeRange) -> ARAContentReaderHostRef {
    on_model_thread(c.cast(), "createMusicalContextContentReader");
    let t = song_time(ctx).lock().unwrap();
    fn raw<T>(v: &[T]) -> Vec<u8> {
        unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }.to_vec()
    }
    let kind = kind as u32;
    let ev = if kind == kARAContentTypeTempoEntries {
        Events { size: std::mem::size_of::<ARAContentTempoEntry>(), bytes: raw(&t.tempo) }
    } else if kind == kARAContentTypeBarSignatures {
        Events { size: std::mem::size_of::<ARAContentBarSignature>(), bytes: raw(&t.bars) }
    } else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(ev)).cast()
}
unsafe extern "C" fn ca_source_available(_c: ARAContentAccessControllerHostRef, _s: ARAAudioSourceHostRef, _k: ARAContentType) -> ARABool {
    0
}
unsafe extern "C" fn ca_source_grade(_c: ARAContentAccessControllerHostRef, _s: ARAAudioSourceHostRef, _k: ARAContentType) -> ARAContentGrade {
    kARAContentGradeInitial as ARAContentGrade
}
unsafe extern "C" fn ca_source_create(_c: ARAContentAccessControllerHostRef, _s: ARAAudioSourceHostRef, _k: ARAContentType, _r: *const ARAContentTimeRange) -> ARAContentReaderHostRef {
    std::ptr::null_mut()
}
unsafe extern "C" fn ca_count(_c: ARAContentAccessControllerHostRef, r: ARAContentReaderHostRef) -> ARAInt32 {
    let ev = unsafe { &*(r as *const Events) };
    (ev.bytes.len() / ev.size) as ARAInt32
}
unsafe extern "C" fn ca_data(_c: ARAContentAccessControllerHostRef, r: ARAContentReaderHostRef, i: ARAInt32) -> *const c_void {
    let ev = unsafe { &*(r as *const Events) };
    ev.bytes[i as usize * ev.size..].as_ptr().cast()
}
unsafe extern "C" fn ca_destroy(c: ARAContentAccessControllerHostRef, r: ARAContentReaderHostRef) {
    on_model_thread(c.cast(), "destroyContentReader");
    drop(unsafe { Box::from_raw(r as *mut Events) });
}
static CONTENT_ACCESS: ARAContentAccessControllerInterface = ARAContentAccessControllerInterface {
    structSize: std::mem::size_of::<ARAContentAccessControllerInterface>(),
    isMusicalContextContentAvailable: Some(ca_available),
    getMusicalContextContentGrade: Some(ca_grade),
    createMusicalContextContentReader: Some(ca_create),
    isAudioSourceContentAvailable: Some(ca_source_available),
    getAudioSourceContentGrade: Some(ca_source_grade),
    createAudioSourceContentReader: Some(ca_source_create),
    getContentReaderEventCount: Some(ca_count),
    getContentReaderDataForEvent: Some(ca_data),
    destroyContentReader: Some(ca_destroy),
};

static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn call(s: String) {
    CALLS.lock().unwrap().push(s);
}
unsafe extern "C" fn pc_start(_c: ARAPlaybackControllerHostRef) {
    call("start".into());
}
unsafe extern "C" fn pc_stop(_c: ARAPlaybackControllerHostRef) {
    call("stop".into());
}
unsafe extern "C" fn pc_position(_c: ARAPlaybackControllerHostRef, t: ARATimePosition) {
    call(format!("position {t}"));
}
unsafe extern "C" fn pc_cycle(_c: ARAPlaybackControllerHostRef, t: ARATimePosition, d: ARATimeDuration) {
    call(format!("cycle {t} {d}"));
}
unsafe extern "C" fn pc_enable(_c: ARAPlaybackControllerHostRef, on: ARABool) {
    call(format!("loop {on}"));
}
static PLAYBACK: ARAPlaybackControllerInterface = ARAPlaybackControllerInterface { structSize: std::mem::size_of::<ARAPlaybackControllerInterface>(), requestStartPlayback: Some(pc_start), requestStopPlayback: Some(pc_stop), requestSetPlaybackPosition: Some(pc_position), requestSetCycleRange: Some(pc_cycle), requestEnableCycle: Some(pc_enable) };

unsafe extern "C" fn mu_progress(_c: ARAModelUpdateControllerHostRef, _s: ARAAudioSourceHostRef, _st: ARAAnalysisProgressState, _v: f32) {}
unsafe extern "C" fn mu_source(_c: ARAModelUpdateControllerHostRef, _s: ARAAudioSourceHostRef, _r: *const ARAContentTimeRange, _f: ARAContentUpdateFlags) {}
unsafe extern "C" fn mu_mod(_c: ARAModelUpdateControllerHostRef, _s: ARAAudioModificationHostRef, _r: *const ARAContentTimeRange, _f: ARAContentUpdateFlags) {}
unsafe extern "C" fn mu_region(_c: ARAModelUpdateControllerHostRef, _s: ARAPlaybackRegionHostRef, _r: *const ARAContentTimeRange, _f: ARAContentUpdateFlags) {}
unsafe extern "C" fn mu_data(_c: ARAModelUpdateControllerHostRef) {
    if !IN_NOTIFY.with(std::cell::Cell::get) {
        call("VIOLATION: notifyDocumentDataChanged outside notifyModelUpdates".into());
    }
    call("data changed".into());
}
static MODEL_UPDATE: ARAModelUpdateControllerInterface = ARAModelUpdateControllerInterface { structSize: std::mem::size_of::<ARAModelUpdateControllerInterface>(), notifyAudioSourceAnalysisProgress: Some(mu_progress), notifyAudioSourceContentChanged: Some(mu_source), notifyAudioModificationContentChanged: Some(mu_mod), notifyPlaybackRegionContentChanged: Some(mu_region), notifyDocumentDataChanged: Some(mu_data) };

fn host() -> ARADocumentControllerHostInstance {
    ARADocumentControllerHostInstance {
        structSize: std::mem::size_of::<ARADocumentControllerHostInstance>(),
        audioAccessControllerHostRef: std::ptr::null_mut(),
        audioAccessControllerInterface: &AUDIO_ACCESS,
        archivingControllerHostRef: std::ptr::null_mut(),
        archivingControllerInterface: &ARCHIVING,
        contentAccessControllerHostRef: std::ptr::null_mut(),
        contentAccessControllerInterface: &CONTENT_ACCESS,
        modelUpdateControllerHostRef: std::ptr::null_mut(),
        modelUpdateControllerInterface: &MODEL_UPDATE,
        playbackControllerHostRef: std::ptr::null_mut(),
        playbackControllerInterface: &PLAYBACK,
    }
}


/// The transport requests and notifications the host has had.
pub fn calls() -> Vec<String> {
    CALLS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A document made through `factory`, with a musical context and a region sequence.
pub struct TestHost {
    pub dc: ARADocumentControllerRef,
    pub i: ARADocumentControllerInterface,
    ctx: ARAMusicalContextRef,
    seq: ARARegionSequenceRef,
    time: &'static Mutex<SongTime>,
    // Boxed: the host refs point at them.
    #[allow(clippy::vec_box)]
    audio: Vec<Box<Audio>>,
}

macro_rules! call {
    ($self:ident . $f:ident ( $($a:expr),* )) => {
        unsafe { $self.i.$f.expect(stringify!($f))($self.dc $(, $a)*) }
    };
}

impl TestHost {
    pub fn new(factory: *const ARAFactory) -> Self {
        let ctx = Box::leak(Box::new(HostCtx { model: std::thread::current().id() }));
        let f = unsafe { std::ptr::read_unaligned(factory) };
        let config = ARAInterfaceConfiguration { structSize: std::mem::size_of::<ARAInterfaceConfiguration>(), desiredApiGeneration: kARAAPIGeneration_2_0_Final as ARAAPIGeneration, assertFunctionAddress: std::ptr::null_mut() };
        unsafe { f.initializeARAWithConfiguration.expect("init")(&config) };
        let mut h = host();
        h.audioAccessControllerHostRef = (ctx as *mut HostCtx).cast();
        h.contentAccessControllerHostRef = (ctx as *mut HostCtx).cast();
        let time: &'static Mutex<SongTime> = Box::leak(Box::new(Mutex::new(SongTime::default())));
        let props = ARADocumentProperties { structSize: std::mem::size_of::<ARADocumentProperties>(), name: c"Song".as_ptr() };
        let inst = unsafe { std::ptr::read_unaligned(f.createDocumentControllerWithDocument.expect("create")(&h, &props)) };
        let mut me = Self { dc: inst.documentControllerRef, i: unsafe { std::ptr::read_unaligned(inst.documentControllerInterface) }, ctx: std::ptr::null_mut(), seq: std::ptr::null_mut(), time, audio: Vec::new() };
        call!(me.beginEditing());
        let ctx_props = ARAMusicalContextProperties { structSize: std::mem::size_of::<ARAMusicalContextProperties>(), name: c"ctx".as_ptr(), orderIndex: 0, color: std::ptr::null() };
        me.ctx = call!(me.createMusicalContext((time as *const Mutex<SongTime>).cast_mut().cast(), &ctx_props));
        let seq_props = ARARegionSequenceProperties { structSize: std::mem::size_of::<ARARegionSequenceProperties>(), name: c"Vocal".as_ptr(), orderIndex: 0, musicalContextRef: me.ctx, color: std::ptr::null() };
        me.seq = call!(me.createRegionSequence(std::ptr::null_mut(), &seq_props));
        call!(me.endEditing());
        me
    }

    /// A clip: a source (stereo `audio`), its modification, a region playing from `mod_start` s
    /// of it at song time `play_start` s for `duration` s. Samples access is enabled.
    #[allow(clippy::too_many_arguments)]
    pub fn add_clip(&mut self, audio: Audio, src_id: &CStr, mod_id: &CStr, play_start: f64, mod_start: f64, duration: f64) -> (ARAAudioSourceRef, ARAAudioModificationRef, ARAPlaybackRegionRef) {
        let seq = self.seq;
        self.add_clip_to(seq, audio, src_id, mod_id, play_start, mod_start, duration)
    }

    /// Another track (region sequence) named `name`, in the song's musical context.
    pub fn add_track(&mut self, name: &str) -> ARARegionSequenceRef {
        let name = crate::document::leak(name);
        call!(self.beginEditing());
        let seq_props = ARARegionSequenceProperties { structSize: std::mem::size_of::<ARARegionSequenceProperties>(), name: name.as_ptr(), orderIndex: 0, musicalContextRef: self.ctx, color: std::ptr::null() };
        let seq = call!(self.createRegionSequence(std::ptr::null_mut(), &seq_props));
        call!(self.endEditing());
        seq
    }

    /// [`Self::add_clip`] on track `seq`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_clip_to(&mut self, seq: ARARegionSequenceRef, audio: Audio, src_id: &CStr, mod_id: &CStr, play_start: f64, mod_start: f64, duration: f64) -> (ARAAudioSourceRef, ARAAudioModificationRef, ARAPlaybackRegionRef) {
        let a = Box::new(audio);
        let host_ref = &*a as *const Audio as *mut c_void;
        let (n, fs) = (a.l.len() as i64, a.fs);
        self.audio.push(a);
        call!(self.beginEditing());
        let src_props = ARAAudioSourceProperties { structSize: std::mem::size_of::<ARAAudioSourceProperties>(), name: c"vox.wav".as_ptr(), persistentID: src_id.as_ptr(), sampleCount: n, sampleRate: fs, channelCount: 2, merits64BitSamples: 0, channelArrangementDataType: 0, channelArrangement: std::ptr::null() };
        let src = call!(self.createAudioSource(host_ref.cast(), &src_props));
        let mod_props = ARAAudioModificationProperties { structSize: std::mem::size_of::<ARAAudioModificationProperties>(), name: c"vox".as_ptr(), persistentID: mod_id.as_ptr() };
        let md = call!(self.createAudioModification(src, std::ptr::null_mut(), &mod_props));
        let reg_props = ARAPlaybackRegionProperties { structSize: std::mem::size_of::<ARAPlaybackRegionProperties>(), transformationFlags: 0, startInModificationTime: mod_start, durationInModificationTime: duration, startInPlaybackTime: play_start, durationInPlaybackTime: duration, musicalContextRef: self.ctx, regionSequenceRef: seq, name: c"vox".as_ptr(), color: std::ptr::null() };
        let rg = call!(self.createPlaybackRegion(md, std::ptr::null_mut(), &reg_props));
        call!(self.enableAudioSourceSamplesAccess(src, 1));
        call!(self.endEditing());
        (src, md, rg)
    }

    /// Hands region `rg` to a plug-in instance's playback and editor renderers and selects it.
    pub fn give(&self, ext: *const ARAPlugInExtensionInstance, rg: ARAPlaybackRegionRef) {
        let ext = unsafe { std::ptr::read_unaligned(ext) };
        unsafe {
            if !ext.playbackRendererInterface.is_null() {
                let pb = std::ptr::read_unaligned(ext.playbackRendererInterface);
                pb.addPlaybackRegion.expect("add")(ext.playbackRendererRef, rg);
            }
            if !ext.editorRendererInterface.is_null() {
                let ed = std::ptr::read_unaligned(ext.editorRendererInterface);
                ed.addPlaybackRegion.expect("add")(ext.editorRendererRef, rg);
            }
            if !ext.editorViewInterface.is_null() {
                let vw = std::ptr::read_unaligned(ext.editorViewInterface);
                let regs = [rg];
                let sel = ARAViewSelection { structSize: std::mem::size_of::<ARAViewSelection>(), playbackRegionRefsCount: 1, playbackRegionRefs: regs.as_ptr(), regionSequenceRefsCount: 0, regionSequenceRefs: std::ptr::null(), timeRange: std::ptr::null() };
                vw.notifySelection.expect("sel")(ext.editorViewRef, &sel);
            }
        }
    }

    /// The song's tempo map ((s, quarter) points) and bar signatures ((numerator, denominator,
    /// quarter)) change: the host tells the plug-in in an edit cycle.
    pub fn set_time(&self, tempo: &[(f64, f64)], bars: &[(i32, i32, f64)]) {
        {
            let mut t = self.time.lock().unwrap();
            t.tempo = tempo.iter().map(|&(s, q)| ARAContentTempoEntry { timePosition: s, quarterPosition: q }).collect();
            t.bars = bars.iter().map(|&(n, d, q)| ARAContentBarSignature { numerator: n, denominator: d, position: q }).collect();
        }
        call!(self.beginEditing());
        call!(self.updateMusicalContextContent(self.ctx, std::ptr::null(), kARAContentUpdateEverythingChanged as ARAContentUpdateFlags));
        call!(self.endEditing());
    }

    /// What a host does every so often while idle: lets the plug-in send its notifications.
    pub fn notify_model_updates(&self) {
        IN_NOTIFY.with(|f| f.set(true));
        call!(self.notifyModelUpdates());
        IN_NOTIFY.with(|f| f.set(false));
    }

    /// An empty edit cycle (hosts run them often): finished readers are destroyed in it.
    pub fn edit_cycle(&self) {
        call!(self.beginEditing());
        call!(self.endEditing());
    }

        /// Hands the document's track (region sequence) to a plug-in instance's editor renderer, as
    /// REAPER does for the visible FX (it never hands that one the clips).
    pub fn show_track(&self, ext: *const ARAPlugInExtensionInstance) {
        let ext = unsafe { std::ptr::read_unaligned(ext) };
        if !ext.editorRendererInterface.is_null() {
            let ed = unsafe { std::ptr::read_unaligned(ext.editorRendererInterface) };
            unsafe { ed.addRegionSequence.expect("add sequence")(ext.editorRendererRef, self.seq) };
        }
    }

    /// Hands track `seq` to a plug-in instance's editor renderer ([`Self::show_track`] for
    /// another track than the first).
    pub fn show(&self, ext: *const ARAPlugInExtensionInstance, seq: ARARegionSequenceRef) {
        let ext = unsafe { std::ptr::read_unaligned(ext) };
        if !ext.editorRendererInterface.is_null() {
            let ed = unsafe { std::ptr::read_unaligned(ext.editorRendererInterface) };
            unsafe { ed.addRegionSequence.expect("add sequence")(ext.editorRendererRef, seq) };
        }
    }

    /// Hands region `rg` to a plug-in instance's playback renderer only.
    pub fn render(&self, ext: *const ARAPlugInExtensionInstance, rg: ARAPlaybackRegionRef) {
        let ext = unsafe { std::ptr::read_unaligned(ext) };
        let pb = unsafe { std::ptr::read_unaligned(ext.playbackRendererInterface) };
        unsafe { pb.addPlaybackRegion.expect("add")(ext.playbackRendererRef, rg) };
    }

    /// Moves region `rg` to song time `play_start`.
    pub fn move_region(&self, md: ARAAudioModificationRef, rg: ARAPlaybackRegionRef, play_start: f64, mod_start: f64, duration: f64) {
        let _ = md;
        call!(self.beginEditing());
        let reg_props = ARAPlaybackRegionProperties { structSize: std::mem::size_of::<ARAPlaybackRegionProperties>(), transformationFlags: 0, startInModificationTime: mod_start, durationInModificationTime: duration, startInPlaybackTime: play_start, durationInPlaybackTime: duration, musicalContextRef: self.ctx, regionSequenceRef: self.seq, name: c"vox".as_ptr(), color: std::ptr::null() };
        call!(self.updatePlaybackRegionProperties(rg, &reg_props));
        call!(self.endEditing());
    }

    /// The modifications' states as the host would save them (partial persistence).
    pub fn store(&self, mods: &[ARAAudioModificationRef]) -> Vec<u8> {
        let mut bytes: Vec<u8> = Vec::new();
        let filter = ARAStoreObjectsFilter { structSize: std::mem::size_of::<ARAStoreObjectsFilter>(), documentData: 1, audioSourceRefsCount: 0, audioSourceRefs: std::ptr::null(), audioModificationRefsCount: mods.len(), audioModificationRefs: mods.as_ptr() };
        assert_eq!(call!(self.storeObjectsToArchive((&mut bytes as *mut Vec<u8>).cast(), &filter)), 1);
        bytes
    }

    /// Restores an archive (persistent ids as they were).
    pub fn restore(&self, bytes: &[u8]) -> bool {
        let v = bytes.to_vec();
        call!(self.restoreObjectsFromArchive((&v as *const Vec<u8> as *mut Vec<u8>).cast(), std::ptr::null())) != 0
    }

    /// Destroys the document controller (also done on drop).
    pub fn destroy(self) {}
}

impl Drop for TestHost {
    /// The document goes before the audio it reads: destroying it stops its reader threads,
    /// which would otherwise read the host's audio after it is freed.
    fn drop(&mut self) {
        call!(self.destroyDocumentController());
    }
}
