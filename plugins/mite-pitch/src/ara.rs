//! Mite Pitch over ARA 2 (docs/design/pitch-brief.md §1a, step A5): the ARA factory on the CLAP
//! entry, the CLAP plug-in extension (under the three names hosts ask for), and binding an
//! instance to the host's document controller (the session then takes its clips).

use std::ffi::{c_char, c_void, CStr};

use mite_ara::clap::{ClapAraFactory, ClapAraPluginExtension, FACTORY_IDS, PLUGIN_EXTENSION_IDS};
use mite_ara::sys::{ARADocumentControllerRef, ARAFactory, ARAPlugInExtensionInstance, ARAPlugInInstanceRoleFlags};
use mite_ara::FactoryInfo;

use crate::{MitePitch, PitchShared};

pub static INFO: FactoryInfo = FactoryInfo {
    plugin_name: c"Mite Pitch",
    manufacturer: c"MiteFX",
    url: c"https://github.com/ansabic/MiteFX",
    version: c"0.1.0",
    factory_id: c"io.github.ansabic.mitefx.pitch.ara",
    archive_id: c"io.github.ansabic.mitefx.pitch.archive.1",
};

/// The plug-in's CLAP id, as the ARA factory reports it.
const PLUGIN_ID: &CStr = c"io.github.ansabic.mitefx.pitch";

unsafe extern "C" fn factory_count(_f: *const ClapAraFactory) -> u32 {
    1
}
unsafe extern "C" fn factory_get(_f: *const ClapAraFactory, index: u32) -> *const ARAFactory {
    if index == 0 { mite_ara::factory(&INFO) } else { std::ptr::null() }
}
unsafe extern "C" fn factory_plugin_id(_f: *const ClapAraFactory, index: u32) -> *const c_char {
    if index == 0 { PLUGIN_ID.as_ptr() } else { std::ptr::null() }
}

static CLAP_FACTORY: ClapAraFactory = ClapAraFactory { get_factory_count: factory_count, get_ara_factory: factory_get, get_plugin_id: factory_plugin_id };

unsafe extern "C" fn ext_factory(_plugin: *const c_void) -> *const ARAFactory {
    mite_ara::factory(&INFO)
}

unsafe extern "C" fn ext_bind(plugin: *const c_void, dc: ARADocumentControllerRef, known: ARAPlugInInstanceRoleFlags, assigned: ARAPlugInInstanceRoleFlags) -> *const ARAPlugInExtensionInstance {
    // SAFETY: the host passes the `clap_plugin` of a live Mite Pitch instance; its context is
    // `Arc::as_ptr` of the instance's `PitchShared`, which the instance keeps alive.
    let ctx = unsafe { nih_plug::wrapper::clap::extension_context::<MitePitch>(plugin) } as *const PitchShared;
    // SAFETY: as above; `dc` comes from the host.
    unsafe { bind(ctx, dc, known, assigned) }
}

/// Binds the instance behind `ctx` (`Arc::as_ptr` of its `PitchShared`) to document controller
/// `dc`: the CLAP extension's and the VST3 entry point's common path.
///
/// # Safety
/// `ctx` must be null or point at a live `PitchShared`; `dc` must be null or a document controller
/// made by our factory.
pub unsafe fn bind(ctx: *const PitchShared, dc: ARADocumentControllerRef, known: ARAPlugInInstanceRoleFlags, assigned: ARAPlugInInstanceRoleFlags) -> *const ARAPlugInExtensionInstance {
    log(&format!("ara bind: dc null {}, known {known}, assigned {assigned}", dc.is_null()));
    if ctx.is_null() || dc.is_null() {
        return std::ptr::null();
    }
    // SAFETY: as the caller promises.
    let shared = unsafe { &*ctx };
    // SAFETY: as the caller promises.
    let inst = unsafe { mite_ara::bind(dc, known, assigned) };
    let ext = inst.extension();
    shared.session.bind_ara(inst);
    ext
}

static CLAP_EXTENSION: ClapAraPluginExtension = ClapAraPluginExtension { get_factory: ext_factory, bind_to_document_controller: ext_bind };

/// Diagnostics: each ARA binding and the roles the host gave, appended to
/// `<data dir>/MiteFX/Pitch/ara-log.txt` (main-thread calls only, never the audio thread).
pub fn log(what: &str) {
    use std::io::Write;
    let dir = crate::session::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("ara-log.txt")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "{t} {what}");
    }
}

/// `ClapPlugin::clap_factory`.
pub fn clap_factory(id: &CStr) -> *const c_void {
    if !offered() {
        return std::ptr::null();
    }
    // SAFETY: `id` is a valid C string.
    if unsafe { mite_ara::clap::is_one_of(id.as_ptr(), &FACTORY_IDS) } { &CLAP_FACTORY as *const _ as *const c_void } else { std::ptr::null() }
}

/// Whether ARA is offered over CLAP. Off unless `MITE_PITCH_CLAP_ARA` is set: REAPER 7 (Linux)
/// binds over CLAP and then crashes in its own code (an ARA record it never links to an
/// instance, as a track FX and as a take FX; traced 2026-11-05, docs/design/pitch-brief.md §1a).
pub fn offered() -> bool {
    std::env::var_os("MITE_PITCH_CLAP_ARA").is_some()
}

/// `ClapPlugin::clap_extension`.
pub fn clap_extension(id: &CStr) -> *const c_void {
    if !offered() {
        return std::ptr::null();
    }
    // SAFETY: as above.
    if unsafe { mite_ara::clap::is_one_of(id.as_ptr(), &PLUGIN_EXTENSION_IDS) } { &CLAP_EXTENSION as *const _ as *const c_void } else { std::ptr::null() }
}
