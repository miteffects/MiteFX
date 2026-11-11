//! The loader thread: builds what each slot's file asks for (a capture at the host's rate and
//! the chosen size, or a cabinet), reports it in the rig's status, and hands it to the audio
//! thread. It also frees whatever the audio thread retires. `initialize` builds synchronously
//! through the same code, so a session plays its rig from the first block.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mite_core::AtomicF32;
use mite_nam::{LoadOptions, NamModel};

use crate::dsp::{AmpShared, CabLoad, Loaded, ModelLoad};
use crate::ir;
use crate::params::AmpParams;
use crate::rig::{Rig, Slot, Status};

/// Captures are built for blocks of this size (longer host blocks are cut).
pub const MODEL_BLOCK: usize = 512;

/// What a slot was last built from.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    path: Option<PathBuf>,
    size: f64,
    rate: f32,
}

pub struct LoaderCore {
    rig: Arc<Rig>,
    shared: Arc<AmpShared>,
    params: Arc<AmpParams>,
    rate: AtomicF32,
    /// What each slot holds (or was sent), and whether its capture comes in sizes.
    sent: Mutex<([Option<Key>; 3], [bool; 2])>,
}

impl LoaderCore {
    pub fn new(rig: Arc<Rig>, shared: Arc<AmpShared>, params: Arc<AmpParams>) -> Self {
        Self { rig, shared, params, rate: AtomicF32::new(0.0), sent: Mutex::new(([None, None, None], [false, false])) }
    }

    pub fn set_rate(&self, rate: f32) {
        self.rate.store(rate);
        self.rig.poke();
    }

    fn wanted(&self, slot: Slot) -> Key {
        Key { path: self.rig.file(slot).path, size: self.params.model_size(), rate: self.rate.load() }
    }

    fn same(slot: Slot, a: &Key, b: &Key, sized: bool) -> bool {
        a.path == b.path && a.rate == b.rate && (!slot.is_model() || !sized || a.size == b.size)
    }

    fn build_model(&self, slot: Slot, key: &Key) -> (Box<ModelLoad>, bool) {
        let Some(path) = &key.path else {
            self.rig.set_status(slot, Status::Empty);
            return (Box::new(ModelLoad { model: None }), false);
        };
        self.rig.set_status(slot, Status::Loading);
        let opts = LoadOptions { fast_tanh: true, size: key.size, max_block: MODEL_BLOCK };
        match NamModel::from_file(path, opts) {
            Ok(model) => {
                let info = model.info().clone();
                let sized = info.sizes.len() > 1;
                self.rig.set_status(slot, Status::Model(Box::new(info)));
                (Box::new(ModelLoad { model: Some(Loaded::new(model, f64::from(key.rate))) }), sized)
            }
            Err(e) => {
                self.rig.set_status(slot, Status::Failed(e));
                (Box::new(ModelLoad { model: None }), false)
            }
        }
    }

    fn build_cab(&self, key: &Key) -> Box<CabLoad> {
        let Some(path) = &key.path else {
            self.rig.set_status(Slot::Cab, Status::Empty);
            return Box::new(CabLoad { cab: None });
        };
        self.rig.set_status(Slot::Cab, Status::Loading);
        match ir::load(path, f64::from(key.rate)) {
            Ok((cab, info)) => {
                self.rig.set_status(Slot::Cab, Status::Cab(Box::new(info)));
                Box::new(CabLoad { cab: Some(cab) })
            }
            Err(e) => {
                self.rig.set_status(Slot::Cab, Status::Failed(e));
                Box::new(CabLoad { cab: None })
            }
        }
    }

    /// Builds every slot now (initialize). Returns what to install.
    pub fn build_all(&self) -> ([Option<Box<ModelLoad>>; 2], Option<Box<CabLoad>>) {
        let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let mut models: [Option<Box<ModelLoad>>; 2] = [None, None];
        for slot in [Slot::Pedal, Slot::Amp] {
            let key = self.wanted(slot);
            let (m, sized) = self.build_model(slot, &key);
            models[slot.index()] = Some(m);
            sent.1[slot.index()] = sized;
            sent.0[slot.index()] = Some(key);
        }
        let key = self.wanted(Slot::Cab);
        let cab = self.build_cab(&key);
        sent.0[Slot::Cab.index()] = Some(key);
        (models, Some(cab))
    }

    /// Builds and sends what changed. Returns whether anything was.
    pub fn step(&self) -> bool {
        self.shared.models[0].collect();
        self.shared.models[1].collect();
        self.shared.cab.collect();
        if self.rate.load() <= 0.0 {
            return false;
        }
        let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let mut any = false;
        for slot in Slot::ALL {
            let key = self.wanted(slot);
            let sized = slot.is_model() && sent.1[slot.index()];
            if sent.0[slot.index()].as_ref().is_some_and(|k| Self::same(slot, k, &key, sized)) {
                continue;
            }
            if slot.is_model() {
                let (m, sized) = self.build_model(slot, &key);
                self.shared.models[slot.index()].send(m);
                sent.1[slot.index()] = sized;
            } else {
                self.shared.cab.send(self.build_cab(&key));
            }
            sent.0[slot.index()] = Some(key);
            any = true;
        }
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
            .name("mite-amp-loader".into())
            .spawn(move || {
                while !s.load(Ordering::Acquire) {
                    let seen = c.rig.files_version();
                    c.step();
                    // A size change only shows in the parameter: look again now and then.
                    c.rig.wait(seen, Duration::from_millis(100));
                }
            })
            .ok();
        Self { core, stop, thread }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.core.rig.poke();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
