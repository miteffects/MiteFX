//! Plugin state beyond the parameters (BLUEPRINT §5: "State is versioned and migrated, with
//! golden tests for every version").
//!
//! nih-plug saves the parameters as plain values by id, plus the string fields a plugin's
//! `Params::serialize_fields` returns. MiteFX adds:
//!
//! - [`STATE_VERSION_KEY`]: the state format version, written by every plugin. States saved
//!   before it existed are version 1.
//! - the session (snapshots A–D under `snapshots`, the current preset under `preset`), for
//!   plugins with the shared header.
//!
//! A plugin bumps its version when a release changes what a state means (a parameter renamed,
//! a range remapped, a field reshaped), and adds the step that lifts the old form to the new one
//! to the list it passes to [`migrate`] from `Plugin::filter_state`. Its golden test keeps a
//! state file of every version (`tests/golden/<plugin>.state.v<N>.json`) and loads them all.

use std::collections::BTreeMap;

use mite_core::presets::CurrentPreset;
use mite_core::snapshots::{Snapshots, Values};
use nih_plug::prelude::Params;
use nih_plug::wrapper::state::PluginState;

/// The state format version's field (frozen).
pub const STATE_VERSION_KEY: &str = "mitefx_state";
/// The snapshots' and the current preset's fields (frozen).
pub const SNAPSHOTS_KEY: &str = "snapshots";
pub const PRESET_KEY: &str = "preset";

/// One migration: lifts a state of version N to N + 1.
pub type Migration = fn(&mut PluginState);

/// The version a state was saved with.
pub fn version_of(state: &PluginState) -> u32 {
    state.fields.get(STATE_VERSION_KEY).and_then(|v| v.parse().ok()).unwrap_or(1)
}

/// Brings `state` up to `current`: `steps[k]` lifts version k + 1 to k + 2, so a plugin at
/// version N passes N − 1 steps. A state newer than this build is left alone (its unknown
/// parameters and fields are ignored on load).
pub fn migrate(state: &mut PluginState, current: u32, steps: &[Migration]) {
    debug_assert_eq!(steps.len() + 1, current as usize, "one step per version after the first");
    let mut version = version_of(state);
    while version < current {
        if let Some(step) = steps.get(version as usize - 1) {
            step(state);
        }
        version += 1;
    }
    if version_of(state) < current {
        state.fields.insert(STATE_VERSION_KEY.to_owned(), current.to_string());
    }
}

/// The fields every plugin saves: its state version.
pub fn version_fields(current: u32) -> BTreeMap<String, String> {
    [(STATE_VERSION_KEY.to_owned(), current.to_string())].into_iter().collect()
}

/// The fields of a plugin with the shared header: the version, the snapshots (with `live`, the
/// snapshot parameters now, as the active slot) and the current preset.
pub fn session_fields(current: u32, snapshots: &Snapshots, preset: &CurrentPreset, live: Values) -> BTreeMap<String, String> {
    let mut out = version_fields(current);
    out.insert(SNAPSHOTS_KEY.to_owned(), snapshots.to_json(live));
    if let Some(name) = preset.get() {
        out.insert(PRESET_KEY.to_owned(), name);
    }
    out
}

/// Restores what [`session_fields`] saved.
pub fn load_session_fields(fields: &BTreeMap<String, String>, snapshots: &Snapshots, preset: &CurrentPreset) {
    if let Some(json) = fields.get(SNAPSHOTS_KEY) {
        snapshots.load_json(json);
    }
    preset.set(fields.get(PRESET_KEY).cloned());
}

/// The normalized values of the parameters `ids` (for a snapshot), unmodulated.
pub fn live_values(params: &dyn Params, ids: &[String]) -> Values {
    let ptrs: std::collections::HashMap<String, _> = params.param_map().into_iter().map(|(id, ptr, _)| (id, ptr)).collect();
    ids.iter()
        // SAFETY: `param_map`'s pointers point into `params`, which outlives this call.
        .filter_map(|id| ptrs.get(id).map(|p| (id.clone(), unsafe { p.unmodulated_normalized_value() })))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(fields: &[(&str, &str)]) -> PluginState {
        PluginState {
            version: String::new(),
            params: BTreeMap::new(),
            fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn states_without_a_version_are_version_one_and_steps_run_in_order() {
        assert_eq!(version_of(&state(&[])), 1);
        fn one_to_two(s: &mut PluginState) {
            s.fields.insert("trail".into(), "2".into());
        }
        fn two_to_three(s: &mut PluginState) {
            let t = s.fields.get("trail").cloned().unwrap_or_default();
            s.fields.insert("trail".into(), t + "3");
        }
        let mut old = state(&[]);
        migrate(&mut old, 3, &[one_to_two, two_to_three]);
        assert_eq!(old.fields["trail"], "23");
        assert_eq!(version_of(&old), 3);
        let mut mid = state(&[(STATE_VERSION_KEY, "2")]);
        migrate(&mut mid, 3, &[one_to_two, two_to_three]);
        assert_eq!(mid.fields["trail"], "3", "only the steps after its version");
        let mut newer = state(&[(STATE_VERSION_KEY, "9")]);
        migrate(&mut newer, 3, &[one_to_two, two_to_three]);
        assert_eq!(version_of(&newer), 9, "a newer state is left alone");
    }
}
