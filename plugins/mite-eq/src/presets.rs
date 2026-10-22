//! Mite EQ's presets: which parameters are sound, and the factory presets (`presets/*.json`).
//! Files, folders and the list live in `mite_core::presets`; the header's preset controls in
//! `mite_ui::session`.

use mite_core::presets::{self as core, Preset};
pub use mite_core::presets::{CurrentPreset, PresetValues};

use crate::params::{manifest, BYPASS, DELTA, SOLO};

/// The plugin a preset file belongs to (also its folder's name).
pub const PLUGIN: &str = "Mite EQ";

/// The parameters a preset holds: every one but bypass, delta and the bands' solo.
pub fn preset_ids() -> Vec<String> {
    let solo = format!("_{}", SOLO.id);
    manifest()
        .lines()
        .filter_map(|l| l.split(" | ").next())
        .filter(|id| *id != BYPASS.id && *id != DELTA.id && !id.ends_with(&solo))
        .map(str::to_owned)
        .collect()
}

/// The built-in presets, Init first.
pub const FACTORY: &[&str] = &[
    include_str!("../presets/init.json"),
    include_str!("../presets/vocal-presence.json"),
    include_str!("../presets/kick-tight.json"),
    include_str!("../presets/mix-bus.json"),
    include_str!("../presets/de-mud.json"),
    include_str!("../presets/de-ess.json"),
    include_str!("../presets/resonance-tamer.json"),
    include_str!("../presets/telephone.json"),
];

pub fn factory() -> Vec<Preset> {
    FACTORY.iter().filter_map(|json| core::from_json(PLUGIN, json, true)).collect()
}

/// Mite EQ presets in `dir`.
pub fn user_in(dir: &std::path::Path) -> Vec<Preset> {
    core::user_in(PLUGIN, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_factory_preset_reads_and_names_only_real_parameters() {
        let ids = preset_ids();
        let presets = factory();
        assert_eq!(presets.len(), FACTORY.len(), "a factory preset failed to parse");
        assert_eq!(presets[0].name, "Init");
        assert!(presets[0].values.is_empty());
        for p in &presets {
            for id in p.values.keys() {
                assert!(ids.contains(id), "{}: unknown parameter {id}", p.name);
            }
        }
    }

    #[test]
    fn presets_leave_monitoring_out() {
        let ids = preset_ids();
        assert!(ids.contains(&"b01_freq".to_owned()) && ids.contains(&"out_gain".to_owned()) && ids.contains(&"mode".to_owned()));
        assert!(!ids.iter().any(|id| id == "bypass" || id == "delta" || id.ends_with("_solo")));
    }
}
