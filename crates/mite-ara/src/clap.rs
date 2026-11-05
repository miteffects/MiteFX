//! ARA's CLAP binding (ara_api/ARACLAP.h), by hand: the extension names a host may ask for and
//! the two structs behind them.

use std::ffi::{c_char, c_void};

use crate::sys::{ARADocumentControllerRef, ARAFactory, ARAPlugInExtensionInstance, ARAPlugInInstanceRoleFlags};

/// The plug-in extension's names: ARA SDK 2.3, its draft, and the pre-release name REAPER asks
/// for (found in its binary, 2026-11-05).
pub const PLUGIN_EXTENSION_IDS: [&[u8]; 3] = [b"org.ara-audio.ara.pluginextension/2\0", b"org.ara-audio.ara.pluginextension.draft/2\0", b"com.celemony.ara.plugin_extension.draft/1\0"];

/// The factory's names on the CLAP entry (the same three generations).
pub const FACTORY_IDS: [&[u8]; 3] = [b"org.ara-audio.ara.factory/2\0", b"org.ara-audio.ara.factory.draft/2\0", b"com.celemony.ara.factory.draft/1\0"];

/// The CLAP feature saying a plug-in supports ARA.
pub const FEATURE_ARA_SUPPORTED: &str = "ara:supported";

/// `clap_ara_factory_t`.
#[repr(C)]
pub struct ClapAraFactory {
    pub get_factory_count: unsafe extern "C" fn(factory: *const ClapAraFactory) -> u32,
    pub get_ara_factory: unsafe extern "C" fn(factory: *const ClapAraFactory, index: u32) -> *const ARAFactory,
    pub get_plugin_id: unsafe extern "C" fn(factory: *const ClapAraFactory, index: u32) -> *const c_char,
}

/// `clap_ara_plugin_extension_t` (`plugin` is the `clap_plugin_t`).
#[repr(C)]
pub struct ClapAraPluginExtension {
    pub get_factory: unsafe extern "C" fn(plugin: *const c_void) -> *const ARAFactory,
    pub bind_to_document_controller: unsafe extern "C" fn(plugin: *const c_void, document_controller: ARADocumentControllerRef, known_roles: ARAPlugInInstanceRoleFlags, assigned_roles: ARAPlugInInstanceRoleFlags) -> *const ARAPlugInExtensionInstance,
}

/// Whether `id` (a C string from the host) is one of `ids`.
///
/// # Safety
/// `id` must be null or a valid NUL-terminated string.
pub unsafe fn is_one_of(id: *const c_char, ids: &[&[u8]]) -> bool {
    if id.is_null() {
        return false;
    }
    // SAFETY: the caller promises a valid C string.
    let id = unsafe { std::ffi::CStr::from_ptr(id) }.to_bytes_with_nul();
    ids.contains(&id)
}
