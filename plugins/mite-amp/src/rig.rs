//! The rig: which file each slot plays (and where it came from), and what the loader made of
//! it. Shared by the state, the editor and the loader thread; the audio thread never touches it
//! (its models arrive through `Handover`s).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

use mite_nam::ModelInfo;
use serde::{Deserialize, Serialize};

use crate::ir::IrInfo;

/// The three slots, in signal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Pedal = 0,
    Amp = 1,
    Cab = 2,
}

impl Slot {
    pub const ALL: [Slot; 3] = [Slot::Pedal, Slot::Amp, Slot::Cab];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn is_model(self) -> bool {
        self != Slot::Cab
    }

    pub fn name(self) -> &'static str {
        match self {
            Slot::Pedal => "Pedal",
            Slot::Amp => "Amp",
            Slot::Cab => "Cab",
        }
    }

    /// Whether a capture of `kind` belongs here (unknown: trust the user).
    pub fn accepts(self, kind: Gear) -> bool {
        match (self, kind) {
            (_, Gear::Unknown) => self != Slot::Cab,
            (Slot::Pedal, Gear::Pedal) => true,
            (Slot::Amp, Gear::Amp | Gear::FullRig) => true,
            (Slot::Cab, Gear::Cab) => true,
            _ => false,
        }
    }
}

/// What a capture is of, from TONE3000's gear or a `.nam` file's `gear_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gear {
    /// Drives, boosts, outboard: before the amp.
    Pedal,
    Amp,
    /// Amp and cabinet in one capture.
    FullRig,
    Cab,
    Unknown,
}

impl Gear {
    /// From TONE3000's `gear` and `format`.
    pub fn of_tone3000(gear: &str, format: &str) -> Self {
        if format == "ir" {
            return Gear::Cab;
        }
        match gear {
            "pedal" | "outboard" => Gear::Pedal,
            "amp" => Gear::Amp,
            "amp-cab" | "full-rig" => Gear::FullRig,
            "cab" | "ir" => Gear::Cab,
            _ => Gear::Unknown,
        }
    }

    /// From a `.nam` file's `gear_type` (NAM's trainer: amp, pedal, pedal_amp, amp_cab,
    /// amp_pedal_cab, preamp, studio).
    pub fn of_nam(gear_type: Option<&str>) -> Self {
        match gear_type.map(|g| g.to_ascii_lowercase().replace('-', "_")).as_deref() {
            Some("pedal" | "studio") => Gear::Pedal,
            Some("amp" | "preamp" | "pedal_amp") => Gear::Amp,
            Some("amp_cab" | "amp_pedal_cab" | "full_rig") => Gear::FullRig,
            _ => Gear::Unknown,
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Gear::Pedal => "a pedal capture",
            Gear::Amp => "an amp capture",
            Gear::FullRig => "a full rig (amp and cabinet)",
            Gear::Cab => "a cabinet IR",
            Gear::Unknown => "a capture",
        }
    }

    /// The slot it goes in.
    pub fn home(self) -> Slot {
        match self {
            Gear::Pedal => Slot::Pedal,
            Gear::Cab => Slot::Cab,
            _ => Slot::Amp,
        }
    }
}

/// Where a file came from, kept for attribution (TONE3000's terms: creator and license stay
/// with the tone).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Source {
    pub tone_id: u64,
    pub model_id: u64,
    pub title: String,
    pub creator: String,
    pub license: String,
    pub url: String,
    /// TONE3000's gear ("amp", "pedal", "amp-cab", …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gear: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SlotFile {
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// What the loader made of a slot's file.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Status {
    #[default]
    Empty,
    Loading,
    Model(Box<ModelInfo>),
    Cab(Box<IrInfo>),
    Failed(String),
}

#[derive(Default)]
struct Inner {
    files: [SlotFile; 3],
    status: [Status; 3],
}

pub struct Rig {
    inner: Mutex<Inner>,
    /// Moves on every change of the files (the loader wakes for it) and of the status (the
    /// editor redraws for it).
    files_version: AtomicU64,
    status_version: AtomicU64,
    wake: Condvar,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

impl Rig {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner::default()), files_version: AtomicU64::new(0), status_version: AtomicU64::new(0), wake: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding it cannot leave the plain data half-written in a way that
        // matters here: keep going.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn file(&self, slot: Slot) -> SlotFile {
        self.lock().files[slot.index()].clone()
    }

    pub fn files(&self) -> [SlotFile; 3] {
        self.lock().files.clone()
    }

    /// Puts a file in a slot (none: empties it) and wakes the loader.
    pub fn set_file(&self, slot: Slot, file: SlotFile) {
        let mut g = self.lock();
        if g.files[slot.index()] == file {
            return;
        }
        g.status[slot.index()] = if file.path.is_some() { Status::Loading } else { Status::Empty };
        g.files[slot.index()] = file;
        drop(g);
        self.files_version.fetch_add(1, Ordering::AcqRel);
        self.status_version.fetch_add(1, Ordering::AcqRel);
        self.wake.notify_all();
    }

    pub fn status(&self, slot: Slot) -> Status {
        self.lock().status[slot.index()].clone()
    }

    pub fn set_status(&self, slot: Slot, status: Status) {
        self.lock().status[slot.index()] = status;
        self.status_version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn files_version(&self) -> u64 {
        self.files_version.load(Ordering::Acquire)
    }

    pub fn status_version(&self) -> u64 {
        self.status_version.load(Ordering::Acquire)
    }

    /// Wakes the loader without a file change (a size or rate change).
    pub fn poke(&self) {
        self.files_version.fetch_add(1, Ordering::AcqRel);
        self.wake.notify_all();
    }

    /// The loader's wait: until a change past `seen`, or `timeout`.
    pub fn wait(&self, seen: u64, timeout: std::time::Duration) {
        let g = self.lock();
        if self.files_version() != seen {
            return;
        }
        let _ = self.wake.wait_timeout(g, timeout);
    }

    pub fn to_json(&self) -> Option<String> {
        let files = self.files();
        if files.iter().all(|f| f.path.is_none()) {
            return None;
        }
        serde_json::to_string(&files).ok()
    }

    /// From the state (none: an empty rig).
    pub fn load_json(&self, json: Option<&str>) {
        let files: [SlotFile; 3] = json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
        for (slot, file) in Slot::ALL.into_iter().zip(files) {
            self.set_file(slot, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rig_round_trips_through_its_state_field() {
        let rig = Rig::new();
        assert!(rig.to_json().is_none());
        let source = Source { tone_id: 7, model_id: 9, title: "Plexi".into(), creator: "someone".into(), license: "cc-by".into(), url: "https://www.tone3000.com/tones/plexi-7".into(), gear: "amp".into() };
        rig.set_file(Slot::Amp, SlotFile { path: Some("/x/plexi.nam".into()), source: Some(source.clone()) });
        rig.set_file(Slot::Cab, SlotFile { path: Some("/x/v30.wav".into()), source: None });
        assert_eq!(rig.status(Slot::Amp), Status::Loading);
        let json = rig.to_json().unwrap();
        let back = Rig::new();
        back.load_json(Some(&json));
        assert_eq!(back.files(), rig.files());
        assert_eq!(back.file(Slot::Amp).source, Some(source));
        back.load_json(None);
        assert!(back.files().iter().all(|f| f.path.is_none()));
    }
}

#[cfg(test)]
mod gear_tests {
    use super::*;

    #[test]
    fn each_slot_takes_only_its_gear() {
        assert!(Slot::Pedal.accepts(Gear::of_tone3000("pedal", "nam")));
        assert!(Slot::Pedal.accepts(Gear::of_tone3000("outboard", "nam")));
        assert!(!Slot::Pedal.accepts(Gear::of_tone3000("amp-cab", "nam")));
        assert!(!Slot::Pedal.accepts(Gear::of_tone3000("cab", "ir")));
        assert!(Slot::Amp.accepts(Gear::of_tone3000("amp-cab", "nam")));
        assert!(!Slot::Amp.accepts(Gear::of_tone3000("pedal", "nam")));
        assert!(Slot::Cab.accepts(Gear::of_tone3000("", "ir")));
        assert!(!Slot::Cab.accepts(Gear::of_tone3000("amp", "nam")));
        assert_eq!(Gear::of_nam(Some("amp_cab")), Gear::FullRig);
        assert!(Slot::Pedal.accepts(Gear::of_nam(None)));
        assert!(!Slot::Cab.accepts(Gear::Unknown));
    }
}
