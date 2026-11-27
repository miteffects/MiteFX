//! Mite Cut's presets: which parameters are settings, and the factory list (Init). The tuning
//! lives with each track in the ARA document, not in parameters.

use mite_core::presets::{self as core, Preset};

pub use crate::params::sound_ids as preset_ids;

/// The plugin a preset file belongs to (also its folder's name).
pub const PLUGIN: &str = "Mite Cut";

pub const FACTORY: &[&str] = &[include_str!("../presets/init.json")];

pub fn factory() -> Vec<Preset> {
    FACTORY.iter().filter_map(|json| core::from_json(PLUGIN, json, true)).collect()
}
