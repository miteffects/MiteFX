//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::HistoryTap;
use mite_dsp::modulation::{Mode, ModSettings, Rotor, Shape, Transport, STAGE_COUNTS};
use mite_core::modmatrix::MatrixTap;
use mite_dsp::modmatrix::{LfoShape, MatrixSettings, Slot, Source};
use mite_mod::dsp::{record, ModEngine, Output};
use mite_plugin::matrix::MatrixRunner;

#[test]
fn engine_process_never_allocates() {
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(512);
    let mut s = ModSettings::default();
    let mut engine = ModEngine::new(48_000.0, &s, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let mut runner = MatrixRunner::default();
    runner.prepare(48_000.0, 512);
    let tap = MatrixTap::new();
    for i in 0..1500 {
        // The matrix routed half the time: LFOs (every shape) and the envelope on a few targets.
        let mut ms = MatrixSettings::default();
        if (i / 50) % 2 == 1 {
            ms.lfos[0].shape = LfoShape::from_index((i / 17) % 7);
            ms.lfos[1].sync = true;
            ms.slots[0] = Slot { source: Source::Lfo1, target: 2, amount: 0.5 };
            ms.slots[1] = Slot { source: Source::Lfo2, target: 5, amount: -0.3 };
            ms.slots[2] = Slot { source: Source::Envelope, target: 8, amount: 0.8 };
        }
        s.mode = Mode::from_index((i / 40) % Mode::ALL.len());
        s.rotor = Rotor::from_index((i / 23) % 3);
        s.balance = [-1.0, 0.3, 1.0][(i / 31) % 3];
        s.detune_cents = [0.0, 9.0, 50.0][(i / 29) % 3];
        s.thru_zero = (i / 90) % 2 == 0;
        s.shape = Shape::from_index((i / 33) % 6);
        s.voices = 1 + (i / 25) % 4;
        s.stages = STAGE_COUNTS[(i / 27) % STAGE_COUNTS.len()];
        s.sync = (i / 70) % 2 == 0;
        s.feedback = [-0.9, 0.0, 0.9][(i / 45) % 3];
        s.low_cut_hz = [20.0, 500.0][(i / 60) % 2];
        let out = Output { bypass: (i / 150) % 4 == 3, gain_db: -1.0 };
        let t = Transport { tempo: Some(120.0), beats: Some(i as f64 * 0.02), playing: i % 3 != 0 };
        for (k, v) in l.iter_mut().enumerate() {
            *v = ((i * 512 + k) as f32 * 0.03).sin() * 0.5;
        }
        let n = if i % 3 == 0 { 512 } else { 100 };
        let stereo = i % 2 == 0;
        process_wrapper(|| {
            runner.run(&ms, &t, &mut [&mut l[..n], &mut r[..n]], None, &tap, 16, |sub, _, offsets| {
                let s = if offsets.iter().any(|o| *o != 0.0) { ModSettings { depth: (s.depth + offsets[2]).clamp(0.0, 1.0), ..s } } else { s };
                engine.process(sub, stereo, &s, &out, &t, &history);
            })
        });
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
