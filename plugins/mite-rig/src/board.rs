//! The board (docs/design/rig-brief.md §1): a chain read left to right, whose items are blocks
//! or splits; a split forks into 2 or 3 lanes (plain chains) that merge again, each with a level
//! and a pan.
//!
//! The model lives here, behind a lock, for the editor, the state and the loader. What the audio
//! thread reads lives in each block's [`Live`] (shared): its values and bypass as atomics (a knob
//! never rebuilds anything), its loaded file's handovers and status. Structural edits bump the
//! version; the loader compiles the new graph and hands it over.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use mite_core::handover::Handover;
use mite_core::AtomicF32;
use serde::{Deserialize, Serialize};

use crate::blocks::Kind;
use crate::parts::{CabLoad, ModelLoad};
use crate::rig::{SlotFile, Status};

/// Most lanes a split has.
pub const MAX_LANES: usize = 3;
/// Most blocks on a board (all lanes together).
pub const MAX_BLOCKS: usize = 48;

/// What the audio thread and the editor share for one block.
pub struct Live {
    pub id: u64,
    pub kind: Kind,
    pub values: Vec<AtomicF32>,
    pub bypass: AtomicBool,
    /// Captures and cabinets: what the loader built for the block's file.
    pub model: Handover<ModelLoad>,
    pub cab: Handover<CabLoad>,
    file: Mutex<SlotFile>,
    status: Mutex<Status>,
    /// Set by the audio thread when a graph swap lost the block's loaded file (the loader then
    /// sends it again).
    pub reload: AtomicBool,
    /// Whether it has a file (read on the audio thread: no lock, no copy).
    has_path: AtomicBool,
    /// What the block shows of itself (the tuner's pitch and clarity, the comp's reduction).
    pub readout: [AtomicF32; 3],
    /// A block that is one of the suite's plugins: its engine's state, telemetry and editor.
    pub embed: Option<Arc<dyn crate::embed::Embed>>,
    /// An embedded block's values changed since its settings were computed.
    dirty: AtomicBool,
}

impl Live {
    pub fn new(id: u64, kind: Kind) -> Self {
        Self {
            id,
            kind,
            values: kind.params().iter().map(|p| AtomicF32::new(p.spec.default)).collect(),
            bypass: AtomicBool::new(false),
            model: Handover::new(),
            cab: Handover::new(),
            file: Mutex::new(SlotFile::default()),
            status: Mutex::new(Status::Empty),
            reload: AtomicBool::new(false),
            has_path: AtomicBool::new(false),
            readout: [AtomicF32::new(0.0), AtomicF32::new(0.0), AtomicF32::new(0.0)],
            embed: crate::embed::state(kind),
            dirty: AtomicBool::new(false),
        }
    }

    /// An embedded block's settings, computed again if its values changed (allocates: UI or
    /// loader thread). Returns whether they were.
    pub fn settle(&self) -> bool {
        match &self.embed {
            Some(e) if self.dirty.swap(false, Ordering::AcqRel) => {
                e.recompute(self);
                true
            }
            _ => false,
        }
    }

    /// Asks for the settings to be computed again (the tempo moved).
    pub fn unsettle(&self) {
        if self.embed.is_some() {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// Whether the block has a file. Lock- and allocation-free.
    pub fn has_path(&self) -> bool {
        self.has_path.load(Ordering::Acquire)
    }

    pub fn value(&self, i: usize) -> f32 {
        self.values.get(i).map_or(0.0, AtomicF32::load)
    }

    pub fn set(&self, i: usize, v: f32) {
        if let Some(a) = self.values.get(i) {
            a.store(v);
            if self.embed.is_some() {
                self.dirty.store(true, Ordering::Release);
            }
        }
    }

    pub fn bypassed(&self) -> bool {
        self.bypass.load(Ordering::Relaxed)
    }

    pub fn file(&self) -> SlotFile {
        self.file.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_status(&self, s: Status) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    pub(crate) fn set_file_raw(&self, f: SlotFile) -> bool {
        let mut g = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if *g == f {
            return false;
        }
        self.has_path.store(f.path.is_some(), Ordering::Release);
        *g = f;
        true
    }
}

/// How a split feeds its lanes.
pub const ROUTINGS: [&str; 3] = ["Parallel", "L / R", "M / S"];

/// A split's routing (an index into [`ROUTINGS`]) and mix (the lanes against the dry input, 0…1).
pub struct SplitLive {
    pub routing: std::sync::atomic::AtomicU32,
    pub mix: AtomicF32,
}

impl SplitLive {
    pub fn new() -> Self {
        Self { routing: std::sync::atomic::AtomicU32::new(0), mix: AtomicF32::new(1.0) }
    }

    pub fn routing(&self) -> usize {
        self.routing.load(Ordering::Relaxed) as usize
    }
}

impl Default for SplitLive {
    fn default() -> Self {
        Self::new()
    }
}

/// A lane's level and pan at the merge.
pub struct LaneLive {
    pub level_db: AtomicF32,
    pub pan: AtomicF32,
}

impl LaneLive {
    pub fn new() -> Self {
        Self { level_db: AtomicF32::new(0.0), pan: AtomicF32::new(0.0) }
    }
}

impl Default for LaneLive {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct Block {
    pub live: Arc<Live>,
}

impl Block {
    pub fn id(&self) -> u64 {
        self.live.id
    }

    pub fn kind(&self) -> Kind {
        self.live.kind
    }
}

#[derive(Clone)]
pub struct Lane {
    pub blocks: Vec<Block>,
    pub live: Arc<LaneLive>,
}

#[derive(Clone)]
pub struct Split {
    pub id: u64,
    pub lanes: Vec<Lane>,
    pub live: Arc<SplitLive>,
}

#[derive(Clone)]
pub enum Item {
    Block(Block),
    Split(Split),
}

/// Where a block sits: in the main chain at `index`, or in a split's lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Main(usize),
    Lane { split: usize, lane: usize, index: usize },
}

#[derive(Clone, Default)]
struct Model {
    items: Vec<Item>,
    next_id: u64,
}

/// The board, shared by the editor, the state and the loader.
pub struct Board {
    model: Mutex<Model>,
    /// Moves on every structural change (the loader rebuilds the graph).
    structure: AtomicU64,
    /// Moves on any change at all (files, statuses, values from the state): the editor redraws.
    changed: AtomicU64,
    wake: Condvar,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

// ---- the saved form ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct BlockDoc {
    id: u64,
    kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    values: BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    bypass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<SlotFile>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn full() -> f32 {
    1.0
}

fn is_full(v: &f32) -> bool {
    (*v - 1.0).abs() < 1e-6
}

#[derive(Serialize, Deserialize, Default)]
struct LaneDoc {
    #[serde(default)]
    level_db: f32,
    #[serde(default)]
    pan: f32,
    #[serde(default)]
    blocks: Vec<BlockDoc>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ItemDoc {
    Block(BlockDoc),
    Split {
        id: u64,
        lanes: Vec<LaneDoc>,
        #[serde(default, skip_serializing_if = "is_zero")]
        routing: u32,
        #[serde(default = "full", skip_serializing_if = "is_full")]
        mix: f32,
    },
}

#[derive(Serialize, Deserialize, Default)]
struct BoardDoc {
    #[serde(default)]
    items: Vec<ItemDoc>,
}

impl Board {
    pub fn new() -> Self {
        Self { model: Mutex::new(Model { items: Vec::new(), next_id: 1 }), structure: AtomicU64::new(0), changed: AtomicU64::new(0), wake: Condvar::new() }
    }

    /// The board a new Rig starts with: a gate, an amp, a cabinet.
    pub fn with_default() -> Self {
        let b = Self::new();
        b.reset_default();
        b
    }

    pub fn reset_default(&self) {
        let mut m = self.lock();
        m.items.clear();
        for kind in [Kind::Gate, Kind::Amp, Kind::Cab] {
            let id = m.next_id;
            m.next_id += 1;
            m.items.push(Item::Block(Block { live: Arc::new(Live::new(id, kind)) }));
        }
        drop(m);
        self.structure_changed();
    }

    fn lock(&self) -> MutexGuard<'_, Model> {
        self.model.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn structure_changed(&self) {
        self.structure.fetch_add(1, Ordering::AcqRel);
        self.touch();
        self.wake.notify_all();
    }

    /// Something changed the editor should see (a value from the state, a status).
    pub fn touch(&self) {
        self.changed.fetch_add(1, Ordering::AcqRel);
    }

    /// Wakes the loader without a structural change (a file, a size or rate change).
    pub fn poke(&self) {
        self.touch();
        self.wake.notify_all();
    }

    pub fn structure_version(&self) -> u64 {
        self.structure.load(Ordering::Acquire)
    }

    pub fn version(&self) -> u64 {
        self.changed.load(Ordering::Acquire)
    }

    /// The loader's wait: until a change past `seen` (of [`version`](Self::version)), or
    /// `timeout`.
    pub fn wait(&self, seen: u64, timeout: std::time::Duration) {
        let g = self.lock();
        if self.version() != seen {
            return;
        }
        let _ = self.wake.wait_timeout(g, timeout);
    }

    /// A snapshot of the items (cheap: blocks are shared).
    pub fn items(&self) -> Vec<Item> {
        self.lock().items.clone()
    }

    /// Every block, in signal order (lanes top to bottom).
    pub fn blocks(&self) -> Vec<Block> {
        let mut out = Vec::new();
        for item in self.lock().items.iter() {
            match item {
                Item::Block(b) => out.push(b.clone()),
                Item::Split(s) => s.lanes.iter().for_each(|l| out.extend(l.blocks.iter().cloned())),
            }
        }
        out
    }

    pub fn block(&self, id: u64) -> Option<Block> {
        self.blocks().into_iter().find(|b| b.id() == id)
    }

    pub fn place_of(&self, id: u64) -> Option<Place> {
        for (i, item) in self.lock().items.iter().enumerate() {
            match item {
                Item::Block(b) if b.id() == id => return Some(Place::Main(i)),
                Item::Split(s) => {
                    for (l, lane) in s.lanes.iter().enumerate() {
                        if let Some(k) = lane.blocks.iter().position(|b| b.id() == id) {
                            return Some(Place::Lane { split: i, lane: l, index: k });
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Puts a file in a block (captures and cabinets) and wakes the loader.
    pub fn set_file(&self, id: u64, file: SlotFile) {
        let Some(b) = self.block(id) else { return };
        if b.live.set_file_raw(file.clone()) {
            b.live.set_status(if file.path.is_some() { Status::Loading } else { Status::Empty });
            self.poke();
        }
    }

    /// A new block of `kind` at `place`. Returns its id.
    pub fn insert(&self, kind: Kind, place: Place) -> Option<u64> {
        let mut m = self.lock();
        if count(&m.items) >= MAX_BLOCKS {
            return None;
        }
        let id = m.next_id;
        m.next_id += 1;
        let block = Block { live: Arc::new(Live::new(id, kind)) };
        insert_at(&mut m.items, block, place)?;
        drop(m);
        self.structure_changed();
        Some(id)
    }

    /// A split of `lanes` lanes at main index `at` (empty lanes: dry paths). Returns its id.
    pub fn insert_split(&self, at: usize, lanes: usize) -> u64 {
        let mut m = self.lock();
        let id = m.next_id;
        m.next_id += 1;
        let lanes = (0..lanes.clamp(2, MAX_LANES)).map(|_| Lane { blocks: Vec::new(), live: Arc::new(LaneLive::new()) }).collect();
        let at = at.min(m.items.len());
        m.items.insert(at, Item::Split(Split { id, lanes, live: Arc::new(SplitLive::new()) }));
        drop(m);
        self.structure_changed();
        id
    }

    /// Removes a block or a split (a split's blocks go with it).
    pub fn remove(&self, id: u64) {
        let mut m = self.lock();
        let before = count(&m.items) + m.items.len();
        m.items.retain(|i| !matches!(i, Item::Split(s) if s.id == id) && !matches!(i, Item::Block(b) if b.id() == id));
        for item in m.items.iter_mut() {
            if let Item::Split(s) = item {
                for lane in &mut s.lanes {
                    lane.blocks.retain(|b| b.id() != id);
                }
            }
        }
        let after = count(&m.items) + m.items.len();
        drop(m);
        if after != before {
            self.structure_changed();
        }
    }

    /// Moves block `id` to `place` (as it reads after the block is taken out).
    pub fn move_block(&self, id: u64, place: Place) {
        let mut m = self.lock();
        let Some(block) = take_block(&mut m.items, id) else { return };
        if insert_at(&mut m.items, block.clone(), place).is_none() {
            m.items.push(Item::Block(block));
        }
        drop(m);
        self.structure_changed();
    }

    /// Adds a lane to a split (up to three) or removes its last (down to two).
    pub fn set_lanes(&self, split: u64, lanes: usize) {
        let mut m = self.lock();
        let mut changed = false;
        for item in m.items.iter_mut() {
            if let Item::Split(s) = item {
                if s.id == split {
                    let n = lanes.clamp(2, MAX_LANES);
                    while s.lanes.len() < n {
                        s.lanes.push(Lane { blocks: Vec::new(), live: Arc::new(LaneLive::new()) });
                        changed = true;
                    }
                    while s.lanes.len() > n {
                        s.lanes.pop();
                        changed = true;
                    }
                }
            }
        }
        drop(m);
        if changed {
            self.structure_changed();
        }
    }

    pub fn to_json(&self) -> String {
        let block_doc = |b: &Block| {
            let params = b.kind().params();
            BlockDoc {
                id: b.id(),
                kind: b.kind().id().to_owned(),
                values: params.iter().enumerate().filter(|(i, p)| (b.live.value(*i) - p.spec.default).abs() > 1e-6).map(|(i, p)| (p.spec.id.to_owned(), (b.live.value(i) * 1e4).round() / 1e4)).collect(),
                bypass: b.live.bypassed(),
                file: Some(b.live.file()).filter(|f| f.path.is_some()),
            }
        };
        let doc = BoardDoc {
            items: self
                .lock()
                .items
                .iter()
                .map(|i| match i {
                    Item::Block(b) => ItemDoc::Block(block_doc(b)),
                    Item::Split(s) => ItemDoc::Split { id: s.id, lanes: s.lanes.iter().map(|l| LaneDoc { level_db: l.live.level_db.load(), pan: l.live.pan.load(), blocks: l.blocks.iter().map(block_doc).collect() }).collect(), routing: s.live.routing() as u32, mix: s.live.mix.load() },
                })
                .collect(),
        };
        serde_json::to_string(&doc).unwrap_or_default()
    }

    /// From the state (none or unreadable: the default board).
    pub fn load_json(&self, json: Option<&str>) {
        let Some(doc) = json.and_then(|j| serde_json::from_str::<BoardDoc>(j).ok()) else {
            self.reset_default();
            return;
        };
        let mut max_id = 0;
        let mut make = |d: &BlockDoc| -> Option<Block> {
            let kind = Kind::from_id(&d.kind)?;
            max_id = max_id.max(d.id);
            let live = Live::new(d.id, kind);
            for (id, v) in &d.values {
                // A slice-2 pedal's ids are the plugin's now.
                let id = crate::embed::renamed(kind, id).unwrap_or(id);
                if let Some(i) = kind.param_index(id) {
                    let p = &kind.params()[i];
                    live.set(i, v.clamp(p.spec.min, p.spec.max));
                }
            }
            live.settle();
            live.bypass.store(d.bypass, Ordering::Relaxed);
            if let Some(f) = &d.file {
                live.set_file_raw(f.clone());
                live.set_status(if f.path.is_some() { Status::Loading } else { Status::Empty });
            }
            Some(Block { live: Arc::new(live) })
        };
        let mut items = Vec::new();
        for item in &doc.items {
            match item {
                ItemDoc::Block(d) => items.extend(make(d).map(Item::Block)),
                ItemDoc::Split { id, lanes, routing, mix } => {
                    let lanes: Vec<Lane> = lanes
                        .iter()
                        .take(MAX_LANES)
                        .map(|l| {
                            let live = LaneLive::new();
                            live.level_db.store(l.level_db.clamp(-60.0, 12.0));
                            live.pan.store(l.pan.clamp(-1.0, 1.0));
                            Lane { blocks: l.blocks.iter().filter_map(&mut make).collect(), live: Arc::new(live) }
                        })
                        .collect();
                    if lanes.len() >= 2 {
                        let live = SplitLive::new();
                        live.routing.store((*routing).min(ROUTINGS.len() as u32 - 1), Ordering::Relaxed);
                        live.mix.store(mix.clamp(0.0, 1.0));
                        items.push(Item::Split(Split { id: *id, lanes, live: Arc::new(live) }));
                    }
                }
            }
        }
        let split_max = items.iter().filter_map(|i| if let Item::Split(s) = i { Some(s.id) } else { None }).max().unwrap_or(0);
        let mut m = self.lock();
        m.items = items;
        m.next_id = max_id.max(split_max) + 1;
        drop(m);
        self.structure_changed();
    }
}

fn count(items: &[Item]) -> usize {
    items.iter().map(|i| match i {
        Item::Block(_) => 1,
        Item::Split(s) => s.lanes.iter().map(|l| l.blocks.len()).sum(),
    }).sum()
}

fn take_block(items: &mut Vec<Item>, id: u64) -> Option<Block> {
    if let Some(i) = items.iter().position(|it| matches!(it, Item::Block(b) if b.id() == id)) {
        if let Item::Block(b) = items.remove(i) {
            return Some(b);
        }
    }
    for item in items.iter_mut() {
        if let Item::Split(s) = item {
            for lane in &mut s.lanes {
                if let Some(k) = lane.blocks.iter().position(|b| b.id() == id) {
                    return Some(lane.blocks.remove(k));
                }
            }
        }
    }
    None
}

fn insert_at(items: &mut Vec<Item>, block: Block, place: Place) -> Option<()> {
    match place {
        Place::Main(i) => {
            let i = i.min(items.len());
            items.insert(i, Item::Block(block));
        }
        Place::Lane { split, lane, index } => {
            let Some(Item::Split(s)) = items.get_mut(split) else { return None };
            let l = s.lanes.get_mut(lane)?;
            let k = index.min(l.blocks.len());
            l.blocks.insert(k, block);
        }
    }
    Some(())
}

/// Where downloaded and opened files are looked up from a path in the state.
pub fn file_path(f: &SlotFile) -> Option<PathBuf> {
    f.path.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_round_trips_through_its_state_field() {
        let b = Board::with_default();
        let split = b.insert_split(2, 3);
        let delay_place = Place::Lane { split: 2, lane: 1, index: 0 };
        let tone = b.insert(Kind::Tone, delay_place).unwrap();
        b.block(tone).unwrap().live.set(0, 4.5);
        b.block(tone).unwrap().live.bypass.store(true, Ordering::Relaxed);
        if let Item::Split(s) = &b.items()[2] {
            s.lanes[0].live.pan.store(-0.5);
        }
        let json = b.to_json();
        let back = Board::new();
        back.load_json(Some(&json));
        assert_eq!(back.to_json(), json);
        assert_eq!(back.place_of(tone), Some(delay_place));
        assert!(back.block(tone).unwrap().live.bypassed());
        // New blocks never reuse an id.
        let next = back.insert(Kind::Boost, Place::Main(0)).unwrap();
        assert!(next > tone && next > split);
    }

    #[test]
    fn blocks_move_between_the_chain_and_lanes() {
        let b = Board::with_default();
        let ids: Vec<u64> = b.blocks().iter().map(Block::id).collect();
        b.insert_split(1, 2);
        b.move_block(ids[2], Place::Lane { split: 1, lane: 0, index: 0 });
        assert_eq!(b.place_of(ids[2]), Some(Place::Lane { split: 1, lane: 0, index: 0 }));
        b.move_block(ids[2], Place::Main(0));
        assert_eq!(b.place_of(ids[2]), Some(Place::Main(0)));
        b.remove(ids[0]);
        assert!(b.block(ids[0]).is_none());
        b.load_json(Some("nonsense"));
        assert_eq!(b.blocks().len(), 3, "an unreadable state: the default board");
    }
}
