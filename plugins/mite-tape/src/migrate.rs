//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-11-26): auto gain.** A new parameter, on by default; sessions saved before it
//! get it off, so they sound as they did.

use nih_plug::wrapper::state::{ParamValue, PluginState};

pub fn v1_to_v2(state: &mut PluginState) {
    state.params.entry(crate::params::AUTO_GAIN.id.to_owned()).or_insert(ParamValue::Bool(false));
}
