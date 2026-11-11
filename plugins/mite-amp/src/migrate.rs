//! Mite Amp state migrations (`mite_plugin::state`).

use nih_plug::prelude::PluginState;

/// 1 → 2: each slot's gain, level and the pedal's tone were appended; they start neutral (their
/// defaults), so nothing in an old state changes meaning.
pub fn v1_to_v2(_state: &mut PluginState) {}

/// 2 → 3: `cab_on_rig` appended, off (as the Cab behaved).
pub fn v2_to_v3(_state: &mut PluginState) {}
