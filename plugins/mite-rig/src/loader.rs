//! The loader thread: compiles the board into a graph after each structural change, and builds
//! what each capture and cabinet block's file asks for (at the host's rate and the chosen size),
//! reporting it in the block's status. It also frees whatever the audio thread retires.
//! `initialize` builds synchronously through the same code, so a session plays from the first
//! block.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mite_core::AtomicF32;
use mite_nam::{LoadOptions, NamModel};

use crate::blocks::Kind;
use crate::board::{Board, Live};
use crate::engine::RigShared;
use crate::graph::Graph;
use crate::ir;
use crate::params::RigParams;
use crate::parts::{CabLoad, Loaded, ModelLoad};
use crate::rig::{Gear, Status};

/// Captures are built for blocks of this size (longer host blocks are cut).
pub const MODEL_BLOCK: usize = 512;

/// What a block's file was last built from.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: Option<PathBuf>,
    size: f64,
    rate: f32,
}

#[derive(Default)]
struct Built {
    /// The board structure the last graph was built from (and its rate).
    graph: Option<(u64, f32)>,
    files: HashMap<u64, (Key, bool)>,
}

pub struct LoaderCore {
    board: Arc<Board>,
    shared: Arc<RigShared>,
    params: Arc<RigParams>,
    rate: AtomicF32,
    built: Mutex<Built>,
    /// The tempo the embedded plugins' settings were last computed at.
    tempo: AtomicF32,
    /// The board and mapping versions the macro table was last compiled at.
    macros_built: Mutex<Option<(u64, u64)>>,
}

impl LoaderCore {
    pub fn new(board: Arc<Board>, shared: Arc<RigShared>, params: Arc<RigParams>) -> Self {
        Self { board, shared, params, rate: AtomicF32::new(0.0), built: Mutex::new(Built::default()), tempo: AtomicF32::new(crate::embed::tempo()), macros_built: Mutex::new(None) }
    }

    pub fn set_rate(&self, rate: f32) {
        self.rate.store(rate);
        self.board.poke();
    }

    fn key(&self, live: &Live) -> Key {
        Key { path: live.file().path, size: self.params.model_size(), rate: self.rate.load() }
    }

    fn build_model(&self, live: &Live, key: &Key) -> (Box<ModelLoad>, bool) {
        let Some(path) = &key.path else {
            live.set_status(Status::Empty);
            return (Box::new(ModelLoad { model: None, right: None }), false);
        };
        live.set_status(Status::Loading);
        let opts = LoadOptions { fast_tanh: true, size: key.size, max_block: MODEL_BLOCK };
        let result = NamModel::from_file(path, opts);
        self.board.touch();
        match result {
            Ok(model) => {
                let info = model.info().clone();
                let sized = info.sizes.len() > 1;
                let file = live.file();
                let gear = file.gear(&Status::Model(Box::new(info.clone())));
                if !live.kind.accepts(gear) {
                    let home = if gear == Gear::Pedal { "a Pedal" } else { "an Amp" };
                    live.set_status(Status::Failed(format!("This is {} capture: it goes in {home} block", gear.name().to_lowercase())));
                    return (Box::new(ModelLoad { model: None, right: None }), false);
                }
                live.set_status(Status::Model(Box::new(info)));
                let full_rig = gear == Gear::FullRig;
                let mut loaded = Loaded::new(model, f64::from(key.rate));
                loaded.full_rig = full_rig;
                // The right channel's own copy (played only while the block's input is stereo).
                let right = NamModel::from_file(path, opts).ok().map(|m| {
                    let mut r = Loaded::new(m, f64::from(key.rate));
                    r.full_rig = full_rig;
                    r
                });
                (Box::new(ModelLoad { model: Some(loaded), right }), sized)
            }
            Err(e) => {
                live.set_status(Status::Failed(e));
                (Box::new(ModelLoad { model: None, right: None }), false)
            }
        }
    }

    fn build_cab(&self, live: &Live, key: &Key) -> Box<CabLoad> {
        let Some(path) = &key.path else {
            live.set_status(Status::Empty);
            return Box::new(CabLoad { cab: None, right: None });
        };
        live.set_status(Status::Loading);
        let result = ir::load(path, f64::from(key.rate));
        self.board.touch();
        match result {
            Ok((cab, info)) => {
                live.set_status(Status::Cab(Box::new(info)));
                let right = ir::load(path, f64::from(key.rate)).ok().map(|(c, _)| c);
                Box::new(CabLoad { cab: Some(cab), right })
            }
            Err(e) => {
                live.set_status(Status::Failed(e));
                Box::new(CabLoad { cab: None, right: None })
            }
        }
    }

    /// The graph for the board as it is, if its structure (or the rate) changed since the last.
    pub fn build_graph(&self, force: bool) -> Option<Box<Graph>> {
        let rate = self.rate.load();
        if rate <= 0.0 {
            return None;
        }
        let version = self.board.structure_version();
        let mut built = self.built.lock().unwrap_or_else(|e| e.into_inner());
        if !force && built.graph == Some((version, rate)) {
            return None;
        }
        built.graph = Some((version, rate));
        drop(built);
        Some(Box::new(Graph::build(&self.board.items(), rate)))
    }

    /// Builds and sends the files that changed (or that a swap lost). Returns whether any were.
    pub fn build_files(&self) -> bool {
        if self.rate.load() <= 0.0 {
            return false;
        }
        let mut built = self.built.lock().unwrap_or_else(|e| e.into_inner());
        let mut any = false;
        let blocks = self.board.blocks();
        for b in &blocks {
            let live = &b.live;
            if !live.kind.has_file() {
                continue;
            }
            live.model.collect();
            live.cab.collect();
            let key = self.key(live);
            let reload = live.reload.swap(false, Ordering::AcqRel);
            let same = built.files.get(&live.id).is_some_and(|(k, sized)| k.path == key.path && k.rate == key.rate && (!sized || k.size == key.size));
            if same && !reload {
                continue;
            }
            match live.kind {
                Kind::Pedal | Kind::Amp => {
                    let (m, sized) = self.build_model(live, &key);
                    live.model.send(m);
                    built.files.insert(live.id, (key, sized));
                }
                _ => {
                    live.cab.send(self.build_cab(live, &key));
                    built.files.insert(live.id, (key, false));
                }
            }
            any = true;
        }
        // Blocks no longer on the board are forgotten.
        built.files.retain(|id, _| blocks.iter().any(|b| b.id() == *id));
        if any {
            self.board.touch();
        }
        any
    }

    /// The suite's plugins on the board take their values (and the tempo, for synced times).
    pub fn settle(&self) {
        let tempo = crate::embed::tempo();
        let moved = (tempo - self.tempo.load()).abs() > 1e-3;
        self.tempo.store(tempo);
        for b in self.board.blocks() {
            if moved {
                b.live.unsettle();
            }
            b.live.settle();
        }
    }

    /// The macro table for the mapping and board as they are, if either changed since the last.
    pub fn build_macros(&self, force: bool) -> Option<Box<crate::macros::MacroTable>> {
        let key = (self.board.structure_version(), self.board.macros.version());
        let mut built = self.macros_built.lock().unwrap_or_else(|e| e.into_inner());
        if !force && *built == Some(key) {
            return None;
        }
        *built = Some(key);
        Some(Box::new(crate::macros::MacroTable::compile(&self.board)))
    }

    /// A program change: the board of that preset (factory first, then the user's).
    fn program_change(&self) {
        let p = self.board.macros.program.swap(-1, Ordering::AcqRel);
        if p < 0 {
            return;
        }
        let library = mite_core::presets::PresetLibrary::new(crate::presets::PLUGIN, crate::presets::FACTORY);
        if let Some(preset) = library.get(p as usize) {
            self.board.load_json(preset.extra.as_deref());
            self.params.preset.set(Some(preset.name.clone()));
        }
    }

    /// One round: the graph (sent), the files, the collection of retired objects.
    pub fn step(&self) -> bool {
        self.shared.graph.collect();
        self.shared.macros.collect();
        self.board.macros.take_learned();
        self.program_change();
        self.settle();
        if let Some(t) = self.build_macros(false) {
            self.shared.macros.send(t);
        }
        let mut any = false;
        if let Some(g) = self.build_graph(false) {
            self.shared.graph.send(g);
            any = true;
        }
        any |= self.build_files();
        any
    }
}

/// The loader thread (stopped and joined on drop).
pub struct Loader {
    pub core: Arc<LoaderCore>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Loader {
    pub fn new(core: LoaderCore) -> Self {
        let core = Arc::new(core);
        let stop = Arc::new(AtomicBool::new(false));
        let (c, s) = (core.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("mite-rig-loader".into())
            .spawn(move || {
                while !s.load(Ordering::Acquire) {
                    let seen = c.board.version();
                    c.step();
                    // A size change only shows in the parameter, a lost file in a flag: look
                    // again now and then.
                    // 10 ms: a macro or MIDI moving a plugin block's value is heard that soon.
                    c.board.wait(seen, Duration::from_millis(10));
                }
            })
            .ok();
        Self { core, stop, thread }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.core.board.poke();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
