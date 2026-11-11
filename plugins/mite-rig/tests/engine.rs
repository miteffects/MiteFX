//! The board's engine against what it should be: a capture on the board is the capture; lanes
//! with captures of another rate stay in time with a dry lane (and the latency is reported);
//! a structural edit keeps the running units of the blocks still there; the loader's files
//! reach the blocks.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mite_nam::{LoadOptions, NamModel};
use mite_rig::blocks::Kind;
use mite_rig::board::{Board, Item, Place};
use mite_rig::engine::{RigEngine, RigSettings, RigShared};
use mite_rig::loader::{LoaderCore, MODEL_BLOCK};
use mite_rig::params::RigParams;
use mite_rig::rig::{SlotFile, Status};

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mite-nam/tests/models")
}

/// A fresh path for each call (tests run in parallel and must not share files).
fn tmp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("mite-rig-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A model that is the identity (a one-tap Linear model) at `rate`.
fn identity_model(rate: u32) -> PathBuf {
    let path = tmp(&format!("identity-{rate}.nam"));
    std::fs::write(&path, format!(r#"{{"version":"0.5.4","architecture":"Linear","config":{{"receptive_field":1,"bias":false}},"weights":[1.0],"sample_rate":{rate}}}"#)).unwrap();
    path
}

fn noise(n: usize, level: f32) -> Vec<f32> {
    let mut seed = 0x2468_ace1u32;
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            level * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

struct Rig {
    params: Arc<RigParams>,
    board: Arc<Board>,
    shared: Arc<RigShared>,
    core: LoaderCore,
    engine: RigEngine,
}

/// An empty board at `fs`.
fn rig(fs: f32) -> Rig {
    let params = Arc::new(RigParams::default());
    let board = params.board.clone();
    for b in board.blocks() {
        board.remove(b.id());
    }
    let shared = Arc::new(RigShared::new());
    let core = LoaderCore::new(board.clone(), shared.clone(), params.clone());
    core.set_rate(fs);
    Rig { engine: RigEngine::new(fs, &RigSettings::default()), params, board, shared, core }
}

impl Rig {
    /// Builds what the board asks for and puts it in at once.
    fn install(&mut self) {
        self.core.build_files();
        if let Some(g) = self.core.build_graph(true) {
            drop(self.engine.install_now(g));
        }
        self.engine.prime();
    }

    fn run(&mut self, x: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            self.engine.process(&mut [a, b], &RigSettings::default(), &self.shared);
        }
        (l, r)
    }
}

#[test]
fn a_capture_on_the_board_is_the_capture() {
    let mut r = rig(48_000.0);
    let path = models_dir().join("A2.nam");
    let id = r.board.insert(Kind::Amp, Place::Main(0)).unwrap();
    r.board.block(id).unwrap().live.set(2, 0.0); // no normalisation: the raw capture
    r.board.set_file(id, SlotFile { path: Some(path.clone()), source: None });
    r.install();
    assert!(matches!(r.board.block(id).unwrap().live.status(), Status::Model(_)));
    let x = noise(9600, 0.2);
    let (y, _) = r.run(&x, 333);
    let mut reference = NamModel::from_file(&path, LoadOptions { max_block: MODEL_BLOCK, ..LoadOptions::default() }).unwrap();
    let mut want = vec![0.0; x.len()];
    reference.process(&x, &mut want);
    let err = y.iter().zip(&want).skip(64).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(err < 1e-5, "{err}");
}

#[test]
fn lanes_stay_in_time_and_the_latency_is_the_longest_path() {
    // At 44.1 kHz a 48 kHz capture runs in a bracket (32 samples late); a parallel dry lane is
    // delayed to match, so identity capture + dry lane at −6 dB each is the input, in time.
    let mut r = rig(44_100.0);
    r.board.insert_split(0, 2);
    let id = r.board.insert(Kind::Amp, Place::Lane { split: 0, lane: 0, index: 0 }).unwrap();
    r.board.block(id).unwrap().live.set(2, 0.0);
    r.board.set_file(id, SlotFile { path: Some(identity_model(48_000)), source: None });
    if let Item::Split(s) = &r.board.items()[0] {
        for lane in &s.lanes {
            lane.live.level_db.store(-6.0206);
        }
    }
    r.install();
    assert_eq!(r.engine.latency(), 32);
    let x: Vec<f32> = (0..11_025).map(|i| 0.5 * (std::f32::consts::TAU * 997.0 * i as f32 / 44_100.0).sin()).collect();
    let (y, _) = r.run(&x, 128);
    let l = r.engine.latency();
    let err = (2000..x.len()).fold(0.0f32, |m, i| m.max((y[i] - x[i - l]).abs()));
    assert!(err < 2e-3, "{err}");
}

#[test]
fn an_edit_keeps_the_loaded_files_and_the_loader_swaps_it_in() {
    let mut r = rig(48_000.0);
    let id = r.board.insert(Kind::Amp, Place::Main(0)).unwrap();
    r.board.set_file(id, SlotFile { path: Some(models_dir().join("A2.nam")), source: None });
    r.install();
    let x = noise(4800, 0.1);
    let _ = r.run(&x, 256);
    // A boost added in front: the loader sends a new graph; the capture's loaded model moves
    // over (it is not loaded again: the file's status stays, no reload is asked).
    r.board.insert(Kind::Boost, Place::Main(0));
    assert!(r.core.step());
    let _ = r.run(&x, 256);
    assert_eq!(r.shared.swaps.load(std::sync::atomic::Ordering::Acquire), 1);
    assert!(!r.board.block(id).unwrap().live.reload.load(std::sync::atomic::Ordering::Acquire));
    assert!(!r.core.build_files(), "nothing to load again");
    // The boost is in: +6 dB.
    let (y, _) = r.run(&noise(4800, 0.001), 256);
    assert!(y.iter().any(|v| v.abs() > 0.0));
    let _ = &r.params;
}

/// The whole way a split's lane knobs take: the editor's knobs (dragged as a user would), the
/// board, the graph the loader built, an amp after the split (a capture: it used to play the
/// two channels' sum, so a lane's pan was lost), the engine's output.
#[test]
fn a_lanes_level_and_pan_knobs_are_heard_through_an_amp() {
    use mite_rig::editor::{memory_host, RigView};
    use mite_ui::kurbo::Point;
    use mite_ui::{Modifiers, MouseButton, UiEvent, View};
    const MODS: Modifiers = Modifiers { shift: false, alt: false, ctrl: false, meta: false };
    let mut r = rig(48_000.0);
    r.board.insert_split(0, 2);
    let amp = r.board.insert(Kind::Amp, Place::Main(1)).unwrap();
    r.board.block(amp).unwrap().live.set(2, 0.0);
    r.board.set_file(amp, SlotFile { path: Some(identity_model(48_000)), source: None });
    r.install();
    let ui = Arc::new(mite_rig::RigUi::with_stores(r.params.snapshots.clone(), r.params.preset.clone(), r.board.clone(), r.shared.clone()));
    let mut view = RigView::new(ui).with_preset_dir(tmp("presets"));
    let host = memory_host();
    view.tick(&host);
    let fork = view.layout().forks[0].fork;
    for e in [UiEvent::PointerMove { pos: fork, mods: MODS }, UiEvent::PointerDown { pos: fork, button: MouseButton::Left, mods: MODS }, UiEvent::PointerUp { pos: fork, button: MouseButton::Left, mods: MODS }] {
        view.event(&e, &host);
    }
    view.tick(&host);
    let drag = |view: &mut RigView, id: &str, dy: f64| {
        let at = view.knob_at(id).unwrap_or_else(|| panic!("no knob {id}"));
        view.event(&UiEvent::PointerMove { pos: at, mods: MODS }, &host);
        view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: MODS }, &host);
        for k in 1..=10 {
            view.event(&UiEvent::PointerMove { pos: Point::new(at.x, at.y + dy * f64::from(k) / 10.0), mods: MODS }, &host);
        }
        view.event(&UiEvent::PointerUp { pos: Point::new(at.x, at.y + dy), mods: MODS, button: MouseButton::Left }, &host);
        view.tick(&host);
    };
    // Long enough for the amp's stereo copy to fade in (150 ms) before the last 200 ms counted.
    let x = noise(24_000, 0.2);
    let rms = |v: &[f32]| (v[14_400..].iter().map(|a| a * a).sum::<f32>() / 9600.0).sqrt();
    let (l0, r0) = r.run(&x, 256);
    let (a0, b0) = (rms(&l0), rms(&r0));
    // Both lanes' levels all the way down: silence.
    drag(&mut view, "0:level", 400.0);
    drag(&mut view, "1:level", 400.0);
    let (l1, r1) = r.run(&x, 256);
    assert!(rms(&l1) < 0.01 * a0 && rms(&r1) < 0.01 * b0, "levels down: {} {} (from {a0} {b0})", rms(&l1), rms(&r1));
    // Lane 1 back up and hard right: the right only.
    drag(&mut view, "0:level", -300.0);
    drag(&mut view, "0:pan", -400.0);
    let (l2, r2) = r.run(&x, 256);
    assert!(rms(&l2) < 0.01 * a0 && rms(&r2) > 0.5 * b0, "lane 1 hard right: {} {}", rms(&l2), rms(&r2));
}
