//! A host in Rust driving the plug-in side of ARA through its C interfaces, as REAPER would:
//! a document with a source, a modification and a region; the audio read; an instance bound;
//! states archived and restored into a fresh document; transport requests reaching the host.

use std::ffi::{c_void, CStr};
use std::sync::Mutex;
use std::time::Duration;

use mite_ara::sys::*;
use mite_ara::FactoryInfo;

static INFO: FactoryInfo = FactoryInfo {
    plugin_name: c"Mite Test",
    manufacturer: c"MiteFX",
    url: c"https://example.com",
    version: c"0.1.0",
    factory_id: c"io.github.ansabic.mitefx.test.ara",
    archive_id: c"io.github.ansabic.mitefx.test.archive.1",
};

/// The source's audio: two channels.
struct Audio {
    l: Vec<f32>,
    r: Vec<f32>,
}

unsafe extern "C" fn create_reader(_c: ARAAudioAccessControllerHostRef, s: ARAAudioSourceHostRef, _b64: ARABool) -> ARAAudioReaderHostRef {
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
unsafe extern "C" fn destroy_reader(_c: ARAAudioAccessControllerHostRef, _r: ARAAudioReaderHostRef) {}

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
    INFO.archive_id.as_ptr()
}

static ARCHIVING: ARAArchivingControllerInterface = ARAArchivingControllerInterface { structSize: std::mem::size_of::<ARAArchivingControllerInterface>(), getArchiveSize: Some(archive_size), readBytesFromArchive: Some(archive_read), writeBytesToArchive: Some(archive_write), notifyDocumentArchivingProgress: Some(archive_progress), notifyDocumentUnarchivingProgress: Some(archive_progress), getDocumentArchiveID: Some(archive_id) };

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
        contentAccessControllerInterface: std::ptr::null(),
        modelUpdateControllerHostRef: std::ptr::null_mut(),
        modelUpdateControllerInterface: &MODEL_UPDATE,
        playbackControllerHostRef: std::ptr::null_mut(),
        playbackControllerInterface: &PLAYBACK,
    }
}

/// A document controller through the factory, as the host makes one.
struct Dc {
    r: ARADocumentControllerRef,
    i: ARADocumentControllerInterface,
}

impl Dc {
    fn new() -> Self {
        let f = unsafe { std::ptr::read_unaligned(mite_ara::factory(&INFO)) };
        assert_eq!(unsafe { CStr::from_ptr(f.factoryID) }, INFO.factory_id);
        let config = ARAInterfaceConfiguration { structSize: std::mem::size_of::<ARAInterfaceConfiguration>(), desiredApiGeneration: kARAAPIGeneration_2_0_Final as ARAAPIGeneration, assertFunctionAddress: std::ptr::null_mut() };
        unsafe { f.initializeARAWithConfiguration.unwrap()(&config) };
        let h = host();
        let props = ARADocumentProperties { structSize: std::mem::size_of::<ARADocumentProperties>(), name: c"Song".as_ptr() };
        let inst = unsafe { std::ptr::read_unaligned(f.createDocumentControllerWithDocument.unwrap()(&h, &props)) };
        Self { r: inst.documentControllerRef, i: unsafe { std::ptr::read_unaligned(inst.documentControllerInterface) } }
    }
}

/// Builds source (persistent id `src`), modification (`m`), region at song time 2 s.
fn populate(dc: &Dc, audio: &Audio, m_id: &CStr) -> (ARAAudioSourceRef, ARAAudioModificationRef, ARAPlaybackRegionRef) {
    unsafe {
        let i = &dc.i;
        i.beginEditing.unwrap()(dc.r);
        let ctx_props = ARAMusicalContextProperties { structSize: std::mem::size_of::<ARAMusicalContextProperties>(), name: c"ctx".as_ptr(), orderIndex: 0, color: std::ptr::null() };
        let ctx = i.createMusicalContext.unwrap()(dc.r, std::ptr::null_mut(), &ctx_props);
        let seq_props = ARARegionSequenceProperties { structSize: std::mem::size_of::<ARARegionSequenceProperties>(), name: c"Vocal".as_ptr(), orderIndex: 0, musicalContextRef: ctx, color: std::ptr::null() };
        let seq = i.createRegionSequence.unwrap()(dc.r, std::ptr::null_mut(), &seq_props);
        let src_props = ARAAudioSourceProperties { structSize: std::mem::size_of::<ARAAudioSourceProperties>(), name: c"vox.wav".as_ptr(), persistentID: c"src-1".as_ptr(), sampleCount: audio.l.len() as i64, sampleRate: 48_000.0, channelCount: 2, merits64BitSamples: 0, channelArrangementDataType: 0, channelArrangement: std::ptr::null() };
        let src = i.createAudioSource.unwrap()(dc.r, audio as *const Audio as *mut _, &src_props);
        let mod_props = ARAAudioModificationProperties { structSize: std::mem::size_of::<ARAAudioModificationProperties>(), name: c"vox".as_ptr(), persistentID: m_id.as_ptr() };
        let md = i.createAudioModification.unwrap()(dc.r, src, std::ptr::null_mut(), &mod_props);
        let reg_props = ARAPlaybackRegionProperties { structSize: std::mem::size_of::<ARAPlaybackRegionProperties>(), transformationFlags: 0, startInModificationTime: 0.5, durationInModificationTime: 1.0, startInPlaybackTime: 2.0, durationInPlaybackTime: 1.0, musicalContextRef: ctx, regionSequenceRef: seq, name: c"vox".as_ptr(), color: std::ptr::null() };
        let rg = i.createPlaybackRegion.unwrap()(dc.r, md, std::ptr::null_mut(), &reg_props);
        i.enableAudioSourceSamplesAccess.unwrap()(dc.r, src, 1);
        i.endEditing.unwrap()(dc.r);
        (src, md, rg)
    }
}

fn audio() -> Audio {
    let n = 96_000;
    Audio { l: (0..n).map(|k| (k as f32 * 0.01).sin() * 0.5).collect(), r: (0..n).map(|k| (k as f32 * 0.01).sin() * 0.3).collect() }
}

#[test]
fn a_document_is_read_bound_archived_and_restored() {
    let a = audio();
    let dc = Dc::new();
    let (src, md, rg) = populate(&dc, &a, c"mod-1");
    let doc = unsafe { mite_ara::document(dc.r) };
    assert!(doc.wait_reads(Duration::from_secs(10)), "the audio is read");
    let m = doc.snapshot();
    let s = m.sources.values().next().unwrap();
    assert_eq!((s.persistent_id.as_str(), s.sample_count, s.channels), ("src-1", 96_000, 2));
    let mono = s.audio.as_ref().unwrap();
    assert!((mono[1234] - 0.5 * (a.l[1234] + a.r[1234])).abs() < 1e-7);
    let r = m.regions.values().next().unwrap();
    assert_eq!((r.start_in_playback, r.start_in_modification, r.duration_in_playback), (2.0, 0.5, 1.0));
    assert_eq!(m.sequences.values().next().unwrap(), "Vocal");

    // An instance: the host gives it the region to render and to show, and a selection.
    let inst = unsafe { mite_ara::bind(dc.r, 7, 7) };
    let ext = unsafe { std::ptr::read_unaligned(inst.extension()) };
    let pb = unsafe { std::ptr::read_unaligned(ext.playbackRendererInterface) };
    let ed = unsafe { std::ptr::read_unaligned(ext.editorRendererInterface) };
    let vw = unsafe { std::ptr::read_unaligned(ext.editorViewInterface) };
    unsafe {
        pb.addPlaybackRegion.unwrap()(ext.playbackRendererRef, rg);
        ed.addPlaybackRegion.unwrap()(ext.editorRendererRef, rg);
        let regs = [rg];
        let sel = ARAViewSelection { structSize: std::mem::size_of::<ARAViewSelection>(), playbackRegionRefsCount: 1, playbackRegionRefs: regs.as_ptr(), regionSequenceRefsCount: 0, regionSequenceRefs: std::ptr::null(), timeRange: std::ptr::null() };
        vw.notifySelection.unwrap()(ext.editorViewRef, &sel);
    }
    let st = inst.state();
    assert_eq!((st.playback.len(), st.editor.len(), st.selection.len()), (1, 1, 1));
    assert_eq!(st.playback[0], *m.regions.keys().next().unwrap());

    // The plug-in's edits for the modification: the host hears the document changed.
    let mid = *m.modifications.keys().next().unwrap();
    doc.set_state(mid, "{\"edits\":[1,2,3]}".into(), false);
    assert!(!CALLS.lock().unwrap().iter().any(|c| c == "data changed"), "not outside notifyModelUpdates");
    unsafe { dc.i.notifyModelUpdates.unwrap()(dc.r) };
    assert!(CALLS.lock().unwrap().iter().any(|c| c == "data changed"));
    assert_eq!(unsafe { dc.i.isAudioModificationPreservingAudioSourceSignal.unwrap()(dc.r, md) }, 0);

    // Transport requests reach the host.
    doc.request_position(3.25);
    doc.request_play(true);
    doc.request_cycle(2.0, 1.0, true);
    doc.request_play(false);
    {
        let calls = CALLS.lock().unwrap();
        for c in ["position 3.25", "start", "cycle 2 1", "loop 1", "stop"] {
            assert!(calls.iter().any(|x| x == c), "{c}: {calls:?}");
        }
    }

    // Archive (partial persistence, as ARA 2 hosts do), restore into a fresh document whose
    // modification has a new persistent id (the filter maps it).
    let mut bytes: Vec<u8> = Vec::new();
    let mods = [md];
    let filter = ARAStoreObjectsFilter { structSize: std::mem::size_of::<ARAStoreObjectsFilter>(), documentData: 1, audioSourceRefsCount: 0, audioSourceRefs: std::ptr::null(), audioModificationRefsCount: 1, audioModificationRefs: mods.as_ptr() };
    assert_eq!(unsafe { dc.i.storeObjectsToArchive.unwrap()(dc.r, (&mut bytes as *mut Vec<u8>).cast(), &filter) }, 1);
    let dc2 = Dc::new();
    let (_, _, _) = populate(&dc2, &a, c"mod-copy");
    let archived = [c"mod-1".as_ptr()];
    let current = [c"mod-copy".as_ptr()];
    let rf = ARARestoreObjectsFilter { structSize: std::mem::size_of::<ARARestoreObjectsFilter>(), documentData: 1, audioSourceIDsCount: 0, audioSourceArchiveIDs: std::ptr::null(), audioSourceCurrentIDs: std::ptr::null(), audioModificationIDsCount: 1, audioModificationArchiveIDs: archived.as_ptr(), audioModificationCurrentIDs: current.as_ptr() };
    assert_eq!(unsafe { dc2.i.restoreObjectsFromArchive.unwrap()(dc2.r, (&bytes as *const Vec<u8> as *mut Vec<u8>).cast(), &rf) }, 1);
    let doc2 = unsafe { mite_ara::document(dc2.r) };
    assert_eq!(doc2.snapshot().modifications.values().next().unwrap().state, "{\"edits\":[1,2,3]}");
    // Whole-document archives too.
    let mut whole: Vec<u8> = Vec::new();
    assert_eq!(unsafe { dc.i.storeDocumentToArchive.unwrap()(dc.r, (&mut whole as *mut Vec<u8>).cast()) }, 1);
    let dc3 = Dc::new();
    unsafe { dc3.i.beginRestoringDocumentFromArchive.unwrap()(dc3.r, (&whole as *const Vec<u8> as *mut Vec<u8>).cast()) };
    populate(&dc3, &a, c"mod-1");
    assert_eq!(unsafe { dc3.i.endRestoringDocumentFromArchive.unwrap()(dc3.r, (&whole as *const Vec<u8> as *mut Vec<u8>).cast()) }, 1);
    assert_eq!(unsafe { mite_ara::document(dc3.r) }.snapshot().modifications.values().next().unwrap().state, "{\"edits\":[1,2,3]}");

    // Teardown in the host's order: instance, objects, document.
    drop(inst);
    unsafe {
        dc.i.beginEditing.unwrap()(dc.r);
        dc.i.destroyPlaybackRegion.unwrap()(dc.r, rg);
        dc.i.destroyAudioModification.unwrap()(dc.r, md);
        dc.i.enableAudioSourceSamplesAccess.unwrap()(dc.r, src, 0);
        dc.i.destroyAudioSource.unwrap()(dc.r, src);
        dc.i.endEditing.unwrap()(dc.r);
    }
    assert!(doc.snapshot().sources.is_empty());
    drop(doc);
    drop(doc2);
    for d in [&dc, &dc2, &dc3] {
        unsafe { d.i.destroyDocumentController.unwrap()(d.r) };
    }
}

#[test]
fn the_clap_extension_names() {
    use mite_ara::clap::*;
    for id in PLUGIN_EXTENSION_IDS.iter().chain(&FACTORY_IDS) {
        assert!(unsafe { is_one_of(id.as_ptr().cast(), &PLUGIN_EXTENSION_IDS) } || unsafe { is_one_of(id.as_ptr().cast(), &FACTORY_IDS) });
    }
    assert!(unsafe { is_one_of(c"com.celemony.ara.plugin_extension.draft/1".as_ptr(), &PLUGIN_EXTENSION_IDS) });
    assert!(!unsafe { is_one_of(c"clap.params".as_ptr(), &PLUGIN_EXTENSION_IDS) });
}
