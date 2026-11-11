//! Macros (docs/design/rig-brief.md §4): 8 knobs and 4 switches the host sees as parameters (a
//! board's blocks change, a plugin's parameter list cannot), each driving any number of block
//! controls, a switch also a block's on/off. Each can follow a MIDI CC (learned), and a MIDI
//! program change picks a preset's board.
//!
//! The mapping is part of the board (saved with it and its presets). The loader compiles it into
//! a table of the blocks' live values; the audio thread writes them when a macro moves (atomics
//! only: no lock, no allocation).

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use mite_core::handover::Handover;
use mite_core::{AtomicF32, ParamSpec};
use serde::{Deserialize, Serialize};

use crate::board::{Board, Live};
use crate::parts::Stash;

pub const KNOBS: usize = 8;
pub const SWITCHES: usize = 4;
pub const SLOTS: usize = KNOBS + SWITCHES;

/// No CC learned.
const NO_CC: u8 = 255;

/// What a macro drives: a block's control by its id, or (empty) the block's on/off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub block: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub param: String,
}

/// The mapping as saved: each slot's targets and CC.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MacroDoc {
    #[serde(default)]
    pub targets: Vec<Vec<Target>>,
    #[serde(default)]
    pub cc: Vec<Option<u8>>,
}

impl MacroDoc {
    fn normalized(mut self) -> Self {
        self.targets.resize(SLOTS, Vec::new());
        self.cc.resize(SLOTS, None);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.targets.iter().all(Vec::is_empty) && self.cc.iter().all(Option::is_none)
    }
}

/// The macros' shared side: the mapping (UI and loader), and what the audio thread reads and
/// writes (atomics).
pub struct Macros {
    doc: Mutex<MacroDoc>,
    version: AtomicU64,
    cc: [AtomicU8; SLOTS],
    /// Each slot's value in force (0…1), for the editor.
    pub now: [AtomicF32; SLOTS],
    /// A slot whose CC is being learned (−1: none).
    pub learning: AtomicI32,
    /// A CC a slot just learned on the audio thread ((slot + 1) << 8 | cc; 0: none), for the UI
    /// or the loader to save.
    pub learned: AtomicU32,
    /// A program change that arrived (−1: none): the loader loads that preset's board.
    pub program: AtomicI32,
    /// A CC moved a slot (bit per slot): the editor writes it into the host parameter.
    pub from_cc: AtomicU32,
}

impl Default for Macros {
    fn default() -> Self {
        Self::new()
    }
}

impl Macros {
    pub fn new() -> Self {
        Self {
            doc: Mutex::new(MacroDoc::default().normalized()),
            version: AtomicU64::new(0),
            cc: std::array::from_fn(|_| AtomicU8::new(NO_CC)),
            now: std::array::from_fn(|_| AtomicF32::new(0.0)),
            learning: AtomicI32::new(-1),
            learned: AtomicU32::new(0),
            program: AtomicI32::new(-1),
            from_cc: AtomicU32::new(0),
        }
    }

    pub fn doc(&self) -> MacroDoc {
        self.doc.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn edit(&self, f: impl FnOnce(&mut MacroDoc)) {
        let mut d = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut d);
        for (a, c) in self.cc.iter().zip(&d.cc) {
            a.store(c.unwrap_or(NO_CC), Ordering::Release);
        }
        drop(d);
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn set_doc(&self, doc: MacroDoc) {
        self.edit(|d| *d = doc.normalized());
    }

    /// Adds `target` to `slot`, or takes it off if it is there. Returns whether it is on now.
    pub fn toggle(&self, slot: usize, target: Target) -> bool {
        let mut on = false;
        self.edit(|d| {
            let list = &mut d.targets[slot.min(SLOTS - 1)];
            match list.iter().position(|t| *t == target) {
                Some(i) => {
                    list.remove(i);
                }
                None => {
                    list.push(target);
                    on = true;
                }
            }
        });
        on
    }

    pub fn clear(&self, slot: usize) {
        self.edit(|d| d.targets[slot.min(SLOTS - 1)].clear());
    }

    /// Binds `cc` to `slot` (and off any other slot), or unbinds it (None).
    pub fn set_cc(&self, slot: usize, cc: Option<u8>) {
        self.edit(|d| {
            if let Some(c) = cc {
                for other in d.cc.iter_mut() {
                    if *other == Some(c) {
                        *other = None;
                    }
                }
            }
            d.cc[slot.min(SLOTS - 1)] = cc;
        });
    }

    pub fn cc_of(&self, slot: usize) -> Option<u8> {
        let c = self.cc.get(slot)?.load(Ordering::Acquire);
        (c != NO_CC).then_some(c)
    }

    /// The slots driving a target.
    pub fn slots_of(&self, target: &Target) -> Vec<usize> {
        let d = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        (0..SLOTS).filter(|&s| d.targets[s].contains(target)).collect()
    }

    /// Saves a CC the audio thread learned (UI or loader thread). Returns whether there was one.
    pub fn take_learned(&self) -> bool {
        let v = self.learned.swap(0, Ordering::AcqRel);
        if v == 0 {
            return false;
        }
        self.set_cc(((v >> 8) - 1) as usize, Some((v & 0xFF) as u8));
        true
    }

    /// Audio thread: the slot a CC drives, or learns it.
    fn slot_of_cc(&self, cc: u8) -> Option<usize> {
        let learning = self.learning.load(Ordering::Acquire);
        if learning >= 0 {
            self.learning.store(-1, Ordering::Release);
            self.cc[learning as usize].store(cc, Ordering::Release);
            self.learned.store(((learning as u32 + 1) << 8) | u32::from(cc), Ordering::Release);
            return Some(learning as usize);
        }
        self.cc.iter().position(|a| a.load(Ordering::Acquire) == cc)
    }
}

/// One target, compiled: the block's live values, which one (None: its on/off), its spec.
struct Entry {
    slot: usize,
    live: Arc<Live>,
    index: Option<usize>,
    spec: ParamSpec,
    stepped: bool,
}

/// The mapping as the audio thread runs it.
#[derive(Default)]
pub struct MacroTable {
    entries: Vec<Entry>,
}

impl MacroTable {
    /// From the board's mapping (targets on blocks no longer there are left out). Allocates.
    pub fn compile(board: &Board) -> Self {
        let doc = board.macros.doc();
        let mut entries = Vec::new();
        for (slot, targets) in doc.targets.iter().enumerate() {
            for t in targets {
                let Some(b) = board.block(t.block) else { continue };
                if t.param.is_empty() {
                    let spec = ParamSpec { id: "", name: "", unit: "", min: 0.0, max: 1.0, default: 1.0, scale: mite_core::ParamScale::Linear, smoothing_ms: 0.0, decimals: 0, wheel_step: 1.0, wheel_step_fine: 1.0, display: mite_core::ParamDisplay::Number };
                    entries.push(Entry { slot, live: b.live.clone(), index: None, spec, stepped: true });
                } else if let Some(i) = b.kind().param_index(&t.param) {
                    let p = &b.kind().params()[i];
                    entries.push(Entry { slot, live: b.live.clone(), index: Some(i), spec: p.spec, stepped: p.toggle || !p.choices.is_empty() });
                }
            }
        }
        Self { entries }
    }
}

/// The audio thread's side: the table in force, each slot's value and what was last written.
pub struct MacroRunner {
    table: Option<Box<MacroTable>>,
    stash: Stash<MacroTable>,
    value: [f32; SLOTS],
    host: [f32; SLOTS],
    applied: [f32; SLOTS],
}

impl Default for MacroRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl MacroRunner {
    pub fn new() -> Self {
        Self { table: None, stash: Stash::new(), value: [0.0; SLOTS], host: [f32::NAN; SLOTS], applied: [f32::NAN; SLOTS] }
    }

    /// A new table from the loader (all of it is written again).
    pub fn take(&mut self, handover: &Handover<MacroTable>) {
        self.stash.flush(handover);
        if let Some(t) = handover.take() {
            if let Some(old) = self.table.replace(t) {
                self.stash.retire(handover, old);
            }
            self.applied = [f32::NAN; SLOTS];
        }
    }

    /// The host's value of a slot (0…1): it counts when it moves (so a CC holds until then).
    pub fn host(&mut self, slot: usize, v: f32) {
        if v != self.host[slot] {
            self.host[slot] = v;
            self.value[slot] = v;
        }
    }

    /// A MIDI CC (0…1).
    pub fn cc(&mut self, macros: &Macros, cc: u8, v: f32) {
        if let Some(slot) = macros.slot_of_cc(cc) {
            // A switch flips on a press (≥ 64) only, like a footswitch.
            self.value[slot] = if slot >= KNOBS { if v >= 0.5 { 1.0 } else { 0.0 } } else { v };
            macros.from_cc.fetch_or(1 << slot, Ordering::AcqRel);
        }
    }

    /// Writes the targets of the slots that moved.
    pub fn apply(&mut self, macros: &Macros) {
        for slot in 0..SLOTS {
            let v = self.value[slot];
            if v == self.applied[slot] {
                continue;
            }
            self.applied[slot] = v;
            macros.now[slot].store(v);
            let Some(table) = &self.table else { continue };
            for e in table.entries.iter().filter(|e| e.slot == slot) {
                match e.index {
                    None => e.live.bypass.store(v < 0.5, Ordering::Relaxed),
                    Some(i) => {
                        // A switch on a stepped control: its last step when on, its first when off.
                        let n = if slot >= KNOBS { if v >= 0.5 { 1.0 } else { 0.0 } } else { v };
                        let mut plain = e.spec.denormalize(n);
                        if e.stepped {
                            plain = plain.round();
                        }
                        e.live.set(i, plain);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Kind;
    use crate::board::Place;

    /// A macro knob turns a block's control, a switch its on/off; a CC learned drives the macro.
    #[test]
    fn macros_and_midi_drive_the_board() {
        let board = Board::new();
        let wah = board.insert(Kind::Wah, Place::Main(0)).unwrap();
        let delay = board.insert(Kind::Delay, Place::Main(1)).unwrap();
        board.macros.toggle(0, Target { block: wah, param: "position".into() });
        board.macros.toggle(KNOBS, Target { block: delay, param: String::new() });
        let shared: Handover<MacroTable> = Handover::new();
        shared.send(Box::new(MacroTable::compile(&board)));
        let mut run = MacroRunner::new();
        run.take(&shared);
        let pos = Kind::Wah.param_index("position").unwrap();

        run.host(0, 0.25);
        run.host(KNOBS, 0.0);
        run.apply(&board.macros);
        assert!((board.block(wah).unwrap().live.value(pos) - 25.0).abs() < 1e-3);
        assert!(board.block(delay).unwrap().live.bypassed(), "switch off: the delay is off");

        // MIDI learn: the next CC binds to macro 1 and drives it.
        board.macros.learning.store(0, Ordering::Release);
        run.cc(&board.macros, 11, 0.9);
        run.apply(&board.macros);
        assert!((board.block(wah).unwrap().live.value(pos) - 90.0).abs() < 1e-3);
        assert!(board.macros.take_learned());
        assert_eq!(board.macros.cc_of(0), Some(11));
        // A footswitch CC on switch 1.
        board.macros.set_cc(KNOBS, Some(64));
        run.cc(&board.macros, 64, 1.0);
        run.apply(&board.macros);
        assert!(!board.block(delay).unwrap().live.bypassed());
        // The host's value holds until it moves: an unchanged host value leaves the CC's.
        run.host(0, 0.25);
        run.apply(&board.macros);
        assert!((board.block(wah).unwrap().live.value(pos) - 90.0).abs() < 1e-3);
    }

    /// The mapping is saved with the board; loading a board keeps the blocks still there.
    #[test]
    fn the_mapping_rides_with_the_board_and_a_load_keeps_live_blocks() {
        let board = Board::new();
        let wah = board.insert(Kind::Wah, Place::Main(0)).unwrap();
        board.macros.toggle(2, Target { block: wah, param: "q".into() });
        board.macros.set_cc(2, Some(7));
        let json = board.to_json();
        let live = board.block(wah).unwrap().live.clone();
        let structure = board.structure_version();
        live.set(1, 80.0);
        board.load_json(Some(&json));
        assert!(Arc::ptr_eq(&live, &board.block(wah).unwrap().live), "the same block, kept");
        assert_eq!(live.value(1), 50.0, "its value is the saved one");
        assert_eq!(board.structure_version(), structure, "no rebuild for values only");
        let other = Board::new();
        other.load_json(Some(&json));
        assert_eq!(other.macros.cc_of(2), Some(7));
        assert_eq!(other.macros.doc().targets[2], vec![Target { block: wah, param: "q".into() }]);
    }
}
