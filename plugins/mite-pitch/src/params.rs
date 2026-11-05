//! Mite Pitch parameters (docs/design/pitch-brief.md). The take and its edits are not
//! parameters: they are saved with the state (`take`, see `session::Session::to_json`), the take's
//! audio in its own file.
//!
//! IDs are frozen once released (`tests/golden/mite_pitch.params.txt`).

use std::sync::Arc;

use mite_core::BoolSpec;
use mite_core::presets::CurrentPreset;
use mite_core::snapshots::Snapshots;
use mite_plugin::bool_param;
use nih_plug::prelude::*;

use crate::session::Session;

/// The state format version (`mite_plugin::state`).
pub const STATE_VERSION: u32 = 1;

pub const BYPASS: BoolSpec = BoolSpec {
    id: "bypass",
    name: "Bypass",
    default: false,
};

/// The state field holding the take and its edits.
pub const TAKE_KEY: &str = "take";

pub fn manifest() -> String {
    BYPASS.manifest_line() + "\n"
}

pub fn ids() -> Vec<String> {
    manifest()
        .lines()
        .filter_map(|l| l.split(" | ").next())
        .map(str::to_owned)
        .collect()
}

/// Monitoring, not sound.
pub fn sound_ids() -> Vec<String> {
    Vec::new()
}

pub struct PitchParams {
    pub bypass: BoolParam,
    pub snapshots: Arc<Snapshots>,
    pub preset: Arc<CurrentPreset>,
    pub session: Arc<Session>,
}

impl Default for PitchParams {
    fn default() -> Self {
        Self {
            bypass: bool_param(&BYPASS, BYPASS.name.to_owned()),
            snapshots: Arc::new(Snapshots::new()),
            preset: Arc::new(CurrentPreset::default()),
            session: Session::new(),
        }
    }
}

// SAFETY: every `ParamPtr` points into `self`, which nih-plug keeps alive (in an `Arc`) for as
// long as it uses the map. The ids match `manifest()` (checked by tests/params.rs).
unsafe impl Params for PitchParams {
    fn serialize_fields(&self) -> std::collections::BTreeMap<String, String> {
        let live = mite_plugin::state::live_values(self, &sound_ids());
        let mut fields =
            mite_plugin::state::session_fields(STATE_VERSION, &self.snapshots, &self.preset, live);
        if let Some(json) = self.session.to_json() {
            fields.insert(TAKE_KEY.to_owned(), json);
        }
        fields
    }

    fn deserialize_fields(&self, serialized: &std::collections::BTreeMap<String, String>) {
        mite_plugin::state::load_session_fields(serialized, &self.snapshots, &self.preset);
        if let Some(json) = serialized.get(TAKE_KEY) {
            self.session.load_json(json);
        }
    }

    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        vec![(BYPASS.id.to_owned(), self.bypass.as_ptr(), String::new())]
    }
}
