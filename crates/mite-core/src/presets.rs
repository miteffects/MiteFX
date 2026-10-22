//! Presets (BLUEPRINT §7.2), for any plugin: factory presets built into the plugin, and the
//! user's own in a folder per OS. A preset is the plugin's sound parameters as plain values by
//! id; any it leaves out are at their defaults, so a factory preset lists only what it changes.
//! Monitoring (bypass, delta, solo) is not sound and never part of one, and neither are the
//! snapshots A–D: a preset loads into the snapshot being edited. The plugin decides which ids are
//! sound.
//!
//! All of this is file and string work for the UI thread; the audio thread never sees it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The preset file format version.
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

/// A preset file of `plugin` (its display name, "Mite EQ").
pub fn to_json(plugin: &str, name: &str, values: &PresetValues) -> String {
    let file = File { plugin: plugin.to_owned(), version: VERSION, name: name.to_owned(), params: values.clone() };
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// A preset file's contents, if it is one of `plugin`'s that this version can read.
pub fn from_json(plugin: &str, json: &str, factory: bool) -> Option<Preset> {
    let file: File = serde_json::from_str(json).ok()?;
    (file.plugin == plugin && file.version <= VERSION).then_some(Preset { name: file.name, values: file.params, factory })
}

/// Where `plugin`'s user presets live: the OS's settings folder (`~/.config/MiteFX/<plugin>/Presets`,
/// `~/Library/Application Support/…`, `%APPDATA%\MiteFX\…`).
pub fn user_dir(plugin: &str) -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?
    };
    Some(base.join("MiteFX").join(plugin).join("Presets"))
}

/// `plugin`'s presets in `dir`, by name.
pub fn user_in(plugin: &str, dir: &Path) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Preset> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| from_json(plugin, &std::fs::read_to_string(e.path()).ok()?, false))
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// A file name for preset `name` (characters file systems refuse become `_`).
fn file_name(name: &str) -> String {
    let safe: String = name.trim().chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    format!("{}.json", if safe.is_empty() { "Preset".to_owned() } else { safe })
}

/// Saves a user preset of `plugin` (replacing one of the same name) in `dir`.
pub fn save_in(plugin: &str, dir: &Path, name: &str, values: &PresetValues) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(name));
    std::fs::write(&path, to_json(plugin, name.trim(), values))?;
    Ok(path)
}

/// A plugin's presets: the factory ones, then the user's (a user preset shadows a factory one of
/// the same name).
pub struct PresetLibrary {
    plugin: &'static str,
    factory: Vec<Preset>,
    user: Vec<Preset>,
    dir: Option<PathBuf>,
}

impl PresetLibrary {
    /// `factory` holds the built-in preset files (the first is Init); unreadable ones are left
    /// out. User presets come from [`user_dir`].
    pub fn new(plugin: &'static str, factory: &[&str]) -> Self {
        let mut library = Self {
            plugin,
            factory: factory.iter().filter_map(|json| from_json(plugin, json, true)).collect(),
            user: Vec::new(),
            dir: user_dir(plugin),
        };
        library.reload();
        library
    }

    /// Keeps the user's presets in `dir` instead (tests, a portable setup).
    pub fn with_dir(mut self, dir: PathBuf) -> Self {
        self.set_dir(dir);
        self
    }

    pub fn set_dir(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
        self.reload();
    }

    pub fn plugin(&self) -> &'static str {
        self.plugin
    }

    /// Reads the user's folder again (another instance may have saved).
    pub fn reload(&mut self) {
        self.user = self.dir.as_deref().map(|d| user_in(self.plugin, d)).unwrap_or_default();
    }

    pub fn len(&self) -> usize {
        self.factory.len() + self.user.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn factory_len(&self) -> usize {
        self.factory.len()
    }

    pub fn user_len(&self) -> usize {
        self.user.len()
    }

    /// Factory presets, then the user's.
    pub fn get(&self, i: usize) -> Option<&Preset> {
        self.factory.get(i).or_else(|| self.user.get(i.checked_sub(self.factory.len())?))
    }

    /// Where preset `name` is in the list (the user's, if one shadows a factory preset).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        (0..self.len()).rev().find(|&i| self.get(i).is_some_and(|p| p.name == name))
    }

    pub fn find(&self, name: &str) -> Option<&Preset> {
        self.get(self.index_of(name)?)
    }

    /// Saves a user preset and reads the folder again.
    pub fn save(&mut self, name: &str, values: &PresetValues) -> std::io::Result<PathBuf> {
        let dir = self.dir.clone().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no preset folder"))?;
        let path = save_in(self.plugin, &dir, name, values)?;
        self.reload();
        Ok(path)
    }
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

    const INIT: &str = r#"{"plugin":"Test","version":1,"name":"Init","params":{}}"#;
    const LOUD: &str = r#"{"plugin":"Test","version":1,"name":"Loud","params":{"gain":6.0}}"#;

    fn temp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mitefx-presets-{tag}-{}", std::process::id()))
    }

    #[test]
    fn a_user_preset_saves_and_reads_back() {
        let dir = temp("save");
        let values: PresetValues = [("b01_gain".to_owned(), 4.5)].into_iter().collect();
        let path = save_in("Test", &dir, "My: bright/vocal", &values).expect("saved");
        assert!(path.file_name().is_some_and(|f| f == "My_ bright_vocal.json"));
        let read = user_in("Test", &dir);
        assert_eq!(read, vec![Preset { name: "My: bright/vocal".into(), values, factory: false }]);
        // Another plugin's file is not ours.
        assert!(user_in("Other", &dir).is_empty());
        assert!(from_json("Test", r#"{"plugin":"Other","version":1,"name":"x","params":{}}"#, false).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_library_lists_factory_then_user_and_a_user_preset_shadows_its_namesake() {
        let dir = temp("library");
        let mut lib = PresetLibrary::new("Test", &[INIT, LOUD, "not json"]).with_dir(dir.clone());
        assert_eq!((lib.factory_len(), lib.user_len()), (2, 0));
        assert_eq!(lib.index_of("Loud"), Some(1));
        let mine: PresetValues = [("gain".to_owned(), 9.0)].into_iter().collect();
        lib.save("Loud", &mine).expect("saved");
        lib.save("Quiet", &PresetValues::new()).expect("saved");
        assert_eq!(lib.len(), 4);
        assert_eq!(lib.get(2).map(|p| p.name.as_str()), Some("Loud"));
        assert_eq!(lib.index_of("Loud"), Some(2), "the user's Loud shadows the factory one");
        assert_eq!(lib.find("Loud").map(|p| p.values["gain"]), Some(9.0));
        assert!(lib.get(4).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
