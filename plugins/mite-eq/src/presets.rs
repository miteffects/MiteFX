//! Presets (BLUEPRINT §7.2): factory presets built in from `presets/*.json`, and the user's own
//! in a folder per OS. A preset is the plugin's parameters as plain values by id; any it leaves
//! out are at their defaults, so a factory preset lists only what it changes. Bypass, delta and
//! solo are monitoring, not sound, and never part of one. The snapshots A–D are not either: a
//! preset loads into the snapshot being edited.
//!
//! All of this is file and string work for the UI thread; the audio thread never sees it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::params::{manifest, BYPASS, DELTA, SOLO};

/// The plugin a preset file belongs to, and the format version.
const PLUGIN: &str = "Mite EQ";
const VERSION: u32 = 1;

/// Plain values by parameter id.
pub type PresetValues = BTreeMap<String, f32>;

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub values: PresetValues,
    /// Built in (cannot be overwritten), or the user's file.
    pub factory: bool,
}

#[derive(Serialize, Deserialize)]
struct File {
    plugin: String,
    version: u32,
    name: String,
    params: PresetValues,
}

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

pub fn to_json(name: &str, values: &PresetValues) -> String {
    let file = File { plugin: PLUGIN.to_owned(), version: VERSION, name: name.to_owned(), params: values.clone() };
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// A preset file's contents, if it is a Mite EQ preset this version can read.
pub fn from_json(json: &str, factory: bool) -> Option<Preset> {
    let file: File = serde_json::from_str(json).ok()?;
    (file.plugin == PLUGIN && file.version <= VERSION).then_some(Preset { name: file.name, values: file.params, factory })
}

const FACTORY: [&str; 8] = [
    include_str!("../presets/init.json"),
    include_str!("../presets/vocal-presence.json"),
    include_str!("../presets/kick-tight.json"),
    include_str!("../presets/mix-bus.json"),
    include_str!("../presets/de-mud.json"),
    include_str!("../presets/de-ess.json"),
    include_str!("../presets/resonance-tamer.json"),
    include_str!("../presets/telephone.json"),
];

/// The built-in presets, Init first.
pub fn factory() -> Vec<Preset> {
    FACTORY.iter().filter_map(|json| from_json(json, true)).collect()
}

/// Where the user's presets live: the OS's settings folder (`~/.config/MiteFX/Mite EQ/Presets`,
/// `~/Library/Application Support/…`, `%APPDATA%\MiteFX\…`).
pub fn user_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?
    };
    Some(base.join("MiteFX").join(PLUGIN).join("Presets"))
}

/// The user's presets in `dir`, by name.
pub fn user_in(dir: &Path) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Preset> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| from_json(&std::fs::read_to_string(e.path()).ok()?, false))
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}


/// A file name for preset `name` (characters file systems refuse become `_`).
fn file_name(name: &str) -> String {
    let safe: String = name.trim().chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    format!("{}.json", if safe.is_empty() { "Preset".to_owned() } else { safe })
}

/// Saves a user preset (replacing one of the same name) in `dir`.
pub fn save_in(dir: &Path, name: &str, values: &PresetValues) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(name));
    std::fs::write(&path, to_json(name.trim(), values))?;
    Ok(path)
}


/// The name of the preset last loaded or saved (saved with the plugin state), for the header.
#[derive(Default)]
pub struct CurrentPreset(Mutex<Option<String>>);

impl CurrentPreset {
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, name: Option<String>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = name;
    }
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

    #[test]
    fn a_user_preset_saves_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("mitefx-presets-{}", std::process::id()));
        let values: PresetValues = [("b01_gain".to_owned(), 4.5)].into_iter().collect();
        let path = save_in(&dir, "My: bright/vocal", &values).expect("saved");
        assert!(path.file_name().is_some_and(|f| f == "My_ bright_vocal.json"));
        let read = user_in(&dir);
        assert_eq!(read, vec![Preset { name: "My: bright/vocal".into(), values, factory: false }]);
        // Another plugin's file is not ours.
        assert!(from_json(r#"{"plugin":"Other","version":1,"name":"x","params":{}}"#, false).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
