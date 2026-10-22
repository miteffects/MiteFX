//! A minimal CLAP host for tests: creates a plugin through its real `clap_entry` (the code a DAW
//! runs) and saves and loads its state, so golden state files are checked through nih-plug's
//! actual wrapper rather than a copy of it.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use clap_sys::entry::clap_plugin_entry;
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    ptr::null()
}

unsafe extern "C" fn ignore(_: *const clap_host) {}

/// One plugin instance, initialised (not activated). Dropping it destroys the instance.
pub struct ClapInstance {
    entry: &'static clap_plugin_entry,
    plugin: *const clap_plugin,
    /// The host the plugin keeps a pointer to: it must outlive the plugin.
    _host: Box<clap_host>,
}

impl ClapInstance {
    /// Creates the plugin `id` (its `CLAP_ID`) from `entry` (the plugin crate's `clap_entry`).
    pub fn new(entry: &'static clap_plugin_entry, id: &str) -> Self {
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: ptr::null_mut(),
            name: c"MiteFX test host".as_ptr(),
            vendor: c"MiteFX".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_extension),
            request_restart: Some(ignore),
            request_process: Some(ignore),
            request_callback: Some(ignore),
        });
        let id = CString::new(id).expect("plugin id");
        // SAFETY: the entry is a valid CLAP entry; the host lives in a Box kept beside the plugin.
        unsafe {
            assert!((entry.init.expect("init"))(c"".as_ptr()), "clap_entry.init failed");
            let factory = (entry.get_factory.expect("get_factory"))(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            assert!(!factory.is_null(), "no plugin factory");
            let plugin = ((*factory).create_plugin.expect("create_plugin"))(factory, &*host, id.as_ptr());
            assert!(!plugin.is_null(), "unknown plugin id {id:?}");
            assert!(((*plugin).init.expect("init"))(plugin), "plugin init failed");
            Self { entry, plugin, _host: host }
        }
    }

    fn state_ext(&self) -> &clap_plugin_state {
        // SAFETY: the plugin is alive and initialised; the extension, if any, lives as long.
        unsafe {
            let ext = ((*self.plugin).get_extension.expect("get_extension"))(self.plugin, CLAP_EXT_STATE.as_ptr()) as *const clap_plugin_state;
            assert!(!ext.is_null(), "plugin has no state extension");
            &*ext
        }
    }

    /// The state as the plugin writes it for a host, without nih-plug's CLAP framing (an 8-byte
    /// little-endian length before the JSON): the JSON itself.
    pub fn save(&self) -> Vec<u8> {
        unsafe extern "C" fn write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
            // SAFETY: `ctx` is the Vec below, and the plugin hands a valid buffer of `size` bytes.
            unsafe {
                let out = &mut *((*stream).ctx as *mut Vec<u8>);
                out.extend_from_slice(std::slice::from_raw_parts(buffer as *const u8, size as usize));
            }
            size as i64
        }
        let mut out: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: &mut out as *mut Vec<u8> as *mut c_void, write: Some(write) };
        // SAFETY: the stream and its buffer outlive the call.
        let ok = unsafe { (self.state_ext().save.expect("save"))(self.plugin, &stream) };
        assert!(ok, "state save failed");
        let (len, json) = out.split_at(8.min(out.len()));
        let len = u64::from_le_bytes(len.try_into().expect("length prefix")) as usize;
        assert_eq!(len, json.len(), "length prefix");
        json.to_vec()
    }

    /// Loads `state` (JSON, as [`Self::save`] returns it) as a host would: a project or preset
    /// being opened.
    pub fn load(&self, state: &[u8]) -> bool {
        let framed: Vec<u8> = (state.len() as u64).to_le_bytes().into_iter().chain(state.iter().copied()).collect();
        let state = framed.as_slice();
        struct Reader<'a>(&'a [u8]);
        unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
            // SAFETY: `ctx` is the Reader below, and the plugin hands a buffer of `size` bytes.
            unsafe {
                let reader = &mut *((*stream).ctx as *mut Reader<'_>);
                let n = (size as usize).min(reader.0.len());
                ptr::copy_nonoverlapping(reader.0.as_ptr(), buffer as *mut u8, n);
                reader.0 = &reader.0[n..];
                n as i64
            }
        }
        let mut reader = Reader(state);
        let stream = clap_istream { ctx: &mut reader as *mut Reader<'_> as *mut c_void, read: Some(read) };
        // SAFETY: the stream and its data outlive the call.
        unsafe { (self.state_ext().load.expect("load"))(self.plugin, &stream) }
    }

    /// The plugin's name from its descriptor.
    pub fn name(&self) -> String {
        // SAFETY: the descriptor and its strings live as long as the plugin.
        unsafe { CStr::from_ptr((*(*self.plugin).desc).name).to_string_lossy().into_owned() }
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        // SAFETY: created by this host and not used after.
        unsafe {
            ((*self.plugin).destroy.expect("destroy"))(self.plugin);
            (self.entry.deinit.expect("deinit"))();
        }
    }
}

/// A state's JSON with what changes between builds (the plugin version string) taken out, so
/// golden files compare by meaning.
pub fn normalized_state_json(state: &[u8]) -> serde_json::Value {
    let mut v: serde_json::Value = serde_json::from_slice(state).expect("state is JSON");
    if let Some(o) = v.as_object_mut() {
        o.remove("version");
    }
    v
}

/// The golden state check every plugin runs (BLUEPRINT §5). In `dir`, each
/// `<prefix>.state.v<N>.json` is a state saved by version N: loaded through `entry` and saved
/// again, it must come back as `<prefix>.state.v<N>.expected.json` when that exists (an old
/// version, migrated) or as itself (the current version). With `MITEFX_BLESS` set, a missing file
/// for `current` is written from `bless()`, a state that is not just defaults.
pub fn check_golden_states(entry: &'static clap_plugin_entry, id: &str, dir: &std::path::Path, prefix: &str, current: u32, bless: impl FnOnce() -> Vec<u8>) {
    let current_file = dir.join(format!("{prefix}.state.v{current}.json"));
    if std::env::var_os("MITEFX_BLESS").is_some() && !current_file.exists() {
        let pretty = serde_json::to_string_pretty(&normalized_state_json(&bless())).expect("JSON");
        std::fs::write(&current_file, pretty + "\n").expect("write golden state");
    }
    assert!(current_file.exists(), "no golden state for version {current}: run the test with MITEFX_BLESS=1 and commit it");
    let mut checked = 0;
    for entry_path in std::fs::read_dir(dir).expect("golden dir") {
        let path = entry_path.expect("entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        if !(name.starts_with(&format!("{prefix}.state.v")) && name.ends_with(".json")) || name.ends_with(".expected.json") {
            continue;
        }
        let golden = std::fs::read(&path).expect("read golden state");
        let plugin = ClapInstance::new(entry, id);
        assert!(plugin.load(&golden), "{name} did not load");
        let expected_path = path.with_file_name(name.replace(".json", ".expected.json"));
        let expected = if expected_path.exists() { std::fs::read(&expected_path).expect("read expected state") } else { golden };
        assert_eq!(normalized_state_json(&plugin.save()), normalized_state_json(&expected), "{name} came back different");
        checked += 1;
    }
    assert!(checked >= 1);
}
