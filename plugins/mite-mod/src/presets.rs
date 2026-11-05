//! Mite Mod's presets: which parameters are sound, and the factory presets (`presets/*.json`).

use mite_core::presets::{self as core, Preset};

pub use crate::params::sound_ids as preset_ids;

/// The plugin a preset file belongs to (also its folder's name).
pub const PLUGIN: &str = "Mite Mod";

pub const FACTORY: &[&str] = &[
    include_str!("../presets/init.json"),
    include_str!("../presets/lush-chorus.json"),
    include_str!("../presets/eighties-chorus.json"),
    include_str!("../presets/string-ensemble.json"),
    include_str!("../presets/jet-flanger.json"),
    include_str!("../presets/tape-flange.json"),
    include_str!("../presets/four-stage-swirl.json"),
    include_str!("../presets/deep-phaser.json"),
    include_str!("../presets/gentle-vibrato.json"),
    include_str!("../presets/seasick.json"),
    include_str!("../presets/rotary-chorale.json"),
    include_str!("../presets/rotary-tremolo.json"),
    include_str!("../presets/warm-cabinet.json"),
    include_str!("../presets/micro-shift.json"),
    include_str!("../presets/thicken.json"),
    include_str!("../presets/wide-detune.json"),
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
