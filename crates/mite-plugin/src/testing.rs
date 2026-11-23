//! A minimal CLAP host for tests: creates a plugin through its real `clap_entry` (the code a DAW
//! runs), saves and loads its state, and activates and runs it with parameter events, so golden
//! states and the quality gates (`gates`, BLUEPRINT §10) go through nih-plug's actual wrapper
//! rather than a copy of it.

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{clap_event_header, clap_event_param_value, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_BYPASS, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::process::clap_process;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    ptr::null()
}

unsafe extern "C" fn ignore(_: *const clap_host) {}

/// One plugin instance, initialised (activated by [`ClapInstance::activate`]). Dropping it
/// destroys the instance.
pub struct ClapInstance {
    entry: &'static clap_plugin_entry,
    plugin: *const clap_plugin,
    /// The host the plugin keeps a pointer to: it must outlive the plugin.
    _host: Box<clap_host>,
    active: bool,
    steady: i64,
}

/// A parameter as the plugin describes it to a host.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub stepped: bool,
    /// Hidden, read-only or the host's bypass: not one a host automates as a sound parameter.
    pub internal: bool,
}

/// A parameter change `offset` samples into a block, to `value` in the plugin's CLAP units.
#[derive(Debug, Clone, Copy)]
pub struct ParamChange {
    pub offset: u32,
    pub id: u32,
    pub value: f64,
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
            Self { entry, plugin, _host: host, active: false, steady: 0 }
        }
    }

    fn extension<T>(&self, id: &CStr) -> Option<&T> {
        // SAFETY: the plugin is alive; an extension it returns lives as long as it does.
        unsafe {
            let ext = ((*self.plugin).get_extension.expect("get_extension"))(self.plugin, id.as_ptr()) as *const T;
            ext.as_ref()
        }
    }

    /// Every parameter the plugin exposes.
    pub fn params(&self) -> Vec<ParamInfo> {
        let Some(ext) = self.extension::<clap_plugin_params>(CLAP_EXT_PARAMS) else { return Vec::new() };
        // SAFETY: indices below `count`, an info struct for the plugin to fill.
        unsafe {
            let n = (ext.count.expect("count"))(self.plugin);
            (0..n)
                .filter_map(|i| {
                    let mut info: clap_param_info = std::mem::zeroed();
                    (ext.get_info.expect("get_info"))(self.plugin, i, &mut info).then(|| ParamInfo {
                        id: info.id,
                        name: CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned(),
                        min: info.min_value,
                        max: info.max_value,
                        default: info.default_value,
                        stepped: info.flags & CLAP_PARAM_IS_STEPPED != 0,
                        internal: info.flags & (CLAP_PARAM_IS_HIDDEN | CLAP_PARAM_IS_READONLY | CLAP_PARAM_IS_BYPASS) != 0,
                    })
                })
                .collect()
        }
    }

    /// A parameter's current value (CLAP units).
    pub fn param_value(&self, id: u32) -> Option<f64> {
        let ext = self.extension::<clap_plugin_params>(CLAP_EXT_PARAMS)?;
        let mut v = 0.0;
        // SAFETY: an out value for the plugin to fill.
        unsafe { (ext.get_value.expect("get_value"))(self.plugin, id, &mut v) }.then_some(v)
    }

    /// The channel counts of the input (or output) ports, in order.
    pub fn ports(&self, input: bool) -> Vec<u32> {
        let Some(ext) = self.extension::<clap_plugin_audio_ports>(CLAP_EXT_AUDIO_PORTS) else { return vec![2] };
        // SAFETY: indices below `count`, an info struct for the plugin to fill.
        unsafe {
            let n = (ext.count.expect("count"))(self.plugin, input);
            (0..n)
                .map(|i| {
                    let mut info: clap_audio_port_info = std::mem::zeroed();
                    if (ext.get.expect("get"))(self.plugin, i, input, &mut info) { info.channel_count } else { 0 }
                })
                .collect()
        }
    }

    /// The latency the plugin reports now (samples).
    pub fn latency(&self) -> u32 {
        // SAFETY: the plugin is alive.
        self.extension::<clap_plugin_latency>(CLAP_EXT_LATENCY).map_or(0, |ext| unsafe { (ext.get.expect("get"))(self.plugin) })
    }

    /// Activates the plugin at `sample_rate` for blocks of 1…`max_block` samples and starts
    /// processing (deactivating it first if it was active).
    pub fn activate(&mut self, sample_rate: f64, max_block: u32) {
        self.deactivate();
        // SAFETY: the plugin is initialised and not active.
        unsafe {
            assert!(((*self.plugin).activate.expect("activate"))(self.plugin, sample_rate, 1, max_block), "activate failed at {sample_rate} Hz");
            assert!(((*self.plugin).start_processing.expect("start_processing"))(self.plugin), "start_processing failed");
        }
        self.active = true;
        self.steady = 0;
    }

    pub fn deactivate(&mut self) {
        if self.active {
            // SAFETY: the plugin is active and processing.
            unsafe {
                ((*self.plugin).stop_processing.expect("stop_processing"))(self.plugin);
                ((*self.plugin).deactivate.expect("deactivate"))(self.plugin);
            }
            self.active = false;
        }
    }

    /// The plugin's `reset()`: what a host calls on a transport jump (clears the DSP state).
    pub fn reset(&mut self) {
        // SAFETY: the plugin is active.
        unsafe { ((*self.plugin).reset.expect("reset"))(self.plugin) }
    }

    /// Runs one block: `main` (the main input's channels, replaced by the output) with the other
    /// input ports silent, and `changes` (sorted by offset) as parameter events.
    pub fn process(&mut self, main: &mut [Vec<f32>], changes: &[ParamChange]) {
        assert!(self.active, "activate first");
        let frames = main.iter().map(Vec::len).min().unwrap_or(0);
        let ins = self.ports(true);
        let outs = self.ports(false);
        // Inputs: the main port gets a copy of `main`, the others silence.
        let mut in_data: Vec<Vec<Vec<f32>>> = ins.iter().enumerate().map(|(p, &ch)| (0..ch as usize).map(|c| if p == 0 { main.get(c).or(main.first()).map_or(vec![0.0; frames], |v| v[..frames].to_vec()) } else { vec![0.0; frames] }).collect()).collect();
        let mut out_data: Vec<Vec<Vec<f32>>> = outs.iter().map(|&ch| vec![vec![0.0f32; frames]; ch as usize]).collect();
        let mut in_ptrs: Vec<Vec<*mut f32>> = in_data.iter_mut().map(|p| p.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        let mut out_ptrs: Vec<Vec<*mut f32>> = out_data.iter_mut().map(|p| p.iter_mut().map(|c| c.as_mut_ptr()).collect()).collect();
        let buffer = |ptrs: &mut Vec<*mut f32>| clap_audio_buffer { data32: ptrs.as_mut_ptr(), data64: ptr::null_mut(), channel_count: ptrs.len() as u32, latency: 0, constant_mask: 0 };
        let inputs: Vec<clap_audio_buffer> = in_ptrs.iter_mut().map(buffer).collect();
        let mut outputs: Vec<clap_audio_buffer> = out_ptrs.iter_mut().map(buffer).collect();
        let events: Vec<clap_event_param_value> = changes
            .iter()
            .map(|c| clap_event_param_value {
                header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: c.offset, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_PARAM_VALUE, flags: 0 },
                param_id: c.id,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: c.value,
            })
            .collect();
        unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
            // SAFETY: `ctx` is the events Vec below.
            unsafe { (*((*list).ctx as *const Vec<clap_event_param_value>)).len() as u32 }
        }
        unsafe extern "C" fn get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
            // SAFETY: `ctx` is the events Vec below; the plugin asks for indices below `size`.
            unsafe {
                let events: &Vec<clap_event_param_value> = &*((*list).ctx as *const Vec<clap_event_param_value>);
                &events[index as usize].header
            }
        }
        unsafe extern "C" fn push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
            true
        }
        let in_events = clap_input_events { ctx: &events as *const _ as *mut c_void, size: Some(size), get: Some(get) };
        let out_events = clap_output_events { ctx: ptr::null_mut(), try_push: Some(push) };
        let process = clap_process {
            steady_time: self.steady,
            frames_count: frames as u32,
            transport: ptr::null(),
            audio_inputs: inputs.as_ptr(),
            audio_outputs: outputs.as_mut_ptr(),
            audio_inputs_count: inputs.len() as u32,
            audio_outputs_count: outputs.len() as u32,
            in_events: &in_events,
            out_events: &out_events,
        };
        // SAFETY: every buffer and event outlives the call.
        unsafe { ((*self.plugin).process.expect("process"))(self.plugin, &process) };
        self.steady += frames as i64;
        for (c, ch) in main.iter_mut().enumerate() {
            if let Some(out) = out_data.first().and_then(|p| p.get(c)) {
                ch[..frames].copy_from_slice(out);
            }
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
        self.deactivate();
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
