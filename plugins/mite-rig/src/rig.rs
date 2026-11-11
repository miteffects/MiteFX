//! What a capture or cabinet block plays: its file, where it came from (TONE3000, for
//! attribution), what the loader made of it, and what gear it is.

use std::path::PathBuf;

use mite_nam::ModelInfo;
use serde::{Deserialize, Serialize};

use crate::ir::IrInfo;

/// What a capture is of, from TONE3000's gear or a `.nam` file's `gear_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gear {
    /// Drives, boosts, outboard.
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

    /// The word on a block: "Pedal", "Amp", "Full rig", "Cab".
    pub fn name(self) -> &'static str {
        match self {
            Gear::Pedal => "Pedal",
            Gear::Amp => "Amp",
            Gear::FullRig => "Full rig",
            Gear::Cab => "Cab",
            Gear::Unknown => "Capture",
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
    /// The tone's cover image on TONE3000 (shown on the block).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SlotFile {
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

impl SlotFile {
    /// What gear it is: TONE3000's word when it came from there, else the loaded file's.
    pub fn gear(&self, status: &Status) -> Gear {
        match (&self.source, status) {
            (Some(s), _) if !s.gear.is_empty() => Gear::of_tone3000(&s.gear, ""),
            (_, Status::Model(info)) => Gear::of_nam(info.gear_type.as_deref()),
            (_, Status::Cab(_)) => Gear::Cab,
            _ => Gear::Unknown,
        }
    }
}

/// What the loader made of a block's file.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Status {
    #[default]
    Empty,
    Loading,
    Model(Box<ModelInfo>),
    Cab(Box<IrInfo>),
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gear_reads_from_tone3000_and_nam_files() {
        assert_eq!(Gear::of_tone3000("outboard", "nam"), Gear::Pedal);
        assert_eq!(Gear::of_tone3000("amp-cab", "nam"), Gear::FullRig);
        assert_eq!(Gear::of_tone3000("", "ir"), Gear::Cab);
        assert_eq!(Gear::of_nam(Some("amp_cab")), Gear::FullRig);
        assert_eq!(Gear::of_nam(None), Gear::Unknown);
    }
}
