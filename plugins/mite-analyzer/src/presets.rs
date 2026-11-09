//! Mite Analyzer's presets: how it reads (target, view, tilt, speed), `presets/*.json`.

use mite_core::presets::{self as core, Preset};

pub use crate::params::sound_ids as preset_ids;

/// The plugin a preset file belongs to (also its folder's name).
pub const PLUGIN: &str = "Mite Analyzer";

pub const FACTORY: &[&str] = &[include_str!("../presets/init.json"), include_str!("../presets/broadcast.json"), include_str!("../presets/mid-side.json")];

pub fn factory() -> Vec<Preset> {
    FACTORY.iter().filter_map(|json| core::from_json(PLUGIN, json, true)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ids;

    #[test]
    fn every_factory_preset_reads_and_names_only_real_parameters() {
        let presets = factory();
        assert_eq!(presets.len(), FACTORY.len(), "a factory preset failed to parse");
        assert_eq!(presets[0].name, "Init");
        for p in &presets {
            for id in p.values.keys() {
                assert!(ids().contains(id), "{}: unknown parameter {id}", p.name);
            }
        }
    }
}
