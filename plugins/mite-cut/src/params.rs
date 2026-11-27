//! Mite Cut parameters. Each track's role, tuning and hand edits are not parameters: they live
//! in the ARA document with the track's clips (`session::ModState`), so the host keeps them with
//! the project.
//!
//! IDs are frozen once released (`tests/golden/mite_cut.params.txt`).

use std::sync::Arc;

use mite_core::BoolSpec;
use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_plugin::bool_param;
use nih_plug::prelude::*;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

pub const BYPASS: BoolSpec = BoolSpec { id: "bypass", name: "Bypass", default: false };

/// Listen to what is cut (and only that): nothing real must be in it.
pub const REMOVED: BoolSpec = BoolSpec { id: "removed", name: "Listen Removed", default: false };

pub fn manifest() -> String {
    [BYPASS.manifest_line(), REMOVED.manifest_line()].join("\n") + "\n"
}

pub fn ids() -> Vec<String> {
    manifest().lines().filter_map(|l| l.split(" | ").next()).map(str::to_owned).collect()
}

/// Monitoring, not sound.
pub fn sound_ids() -> Vec<String> {
    Vec::new()
}

pub struct CutParams {
    pub bypass: BoolParam,
    pub removed: BoolParam,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
}

impl Default for CutParams {
    fn default() -> Self {
        Self {
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            removed: bool_param(&REMOVED, REMOVED.name.to_owned()),
            snapshots: Arc::new(Snapshots::new()),
            preset: Arc::new(CurrentPreset::default()),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for CutParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live)
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        vec![(BYPASS.id.to_owned(), self.bypass.as_ptr(), String::new()), (REMOVED.id.to_owned(), self.removed.as_ptr(), String::new())]
    }
}
