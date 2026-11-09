//! Mite Verb's presets: which parameters are sound, and the factory presets (`presets/*.json`).

use mite_core::presets::{self as core, Preset};

pub use crate::params::sound_ids as preset_ids;

/// The plugin a preset file belongs to (also its folder's name).
pub const PLUGIN: &str = "Mite Verb";

/// The built-in presets, Init first.
pub const FACTORY: &[&str] = &[
    include_str!("../presets/init.json"),
    include_str!("../presets/vocal-plate.json"),
    include_str!("../presets/drum-room.json"),
    include_str!("../presets/tight-booth.json"),
    include_str!("../presets/large-hall.json"),
    include_str!("../presets/ducked-vocal.json"),
    include_str!("../presets/endless.json"),
    include_str!("../presets/shimmer-hall.json"),
    include_str!("../presets/concert-hall.json"),
    include_str!("../presets/bright-plate.json"),
    include_str!("../presets/studio-chamber.json"),
    include_str!("../presets/spring-tank.json"),
];

pub fn factory() -> Vec<Preset> {
    FACTORY.iter().filter_map(|json| core::from_json(PLUGIN, json, true)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{is_monitoring, manifest};

    #[test]
    fn every_factory_preset_reads_and_names_only_real_sound_parameters() {
        let ids = preset_ids();
        let presets = factory();
        assert_eq!(presets.len(), FACTORY.len(), "a factory preset failed to parse");
        assert_eq!(presets[0].name, "Init");
        assert!(presets[0].values.is_empty());
        for p in &presets {
            for (id, v) in &p.values {
                assert!(ids.contains(id), "{}: unknown parameter {id}", p.name);
                let line = manifest().lines().find(|l| l.starts_with(&format!("{id} |"))).map(str::to_owned).unwrap_or_default();
                let fields: Vec<&str> = line.split(" | ").collect();
                if let [_, _, min, max, _, _] = fields[..] {
                    let (min, max) = (min.parse::<f32>().unwrap_or(f32::MIN), max.parse::<f32>().unwrap_or(f32::MAX));
                    assert!((min..=max).contains(v), "{}: {id} = {v} outside {min}…{max}", p.name);
                }
            }
        }
    }

    #[test]
    fn presets_leave_monitoring_out() {
        assert!(preset_ids().iter().all(|id| !is_monitoring(id)));
    }
}
