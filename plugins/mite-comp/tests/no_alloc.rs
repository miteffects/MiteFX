//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do:
//! every block runs inside nih-plug's process guard (`assert_no_alloc`, active in debug builds),
//! which aborts on any allocation on that thread.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_comp::dsp::{record, CompEngine, Output};
use mite_core::HistoryTap;
use mite_dsp::dynamics::{CompSettings, Detector, Quadrant, Smart, Stage, STYLES};
use mite_dsp::eq::{BandSettings, BandType};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = CompSettings::default();
    s.stages[0] = Stage { on: true, quadrant: Quadrant::UpCompress, threshold_db: -40.0, ratio: 2.0, detector: Detector::Rms, ..Stage::default() };
    s.stages[1] = Stage { on: true, quadrant: Quadrant::Expand, threshold_db: -50.0, ratio: 4.0, detector: Detector::Peak, ..Stage::default() };
    s.stages[2] = Stage { on: true, quadrant: Quadrant::Compress, threshold_db: -20.0, ratio: 4.0, detector: Detector::Loudness, ..Stage::default() };
    s.stages[3] = Stage { on: true, quadrant: Quadrant::UpExpand, threshold_db: -6.0, ratio: 1.5, range_db: 6.0, ..Stage::default() };
    s.key_bands[0] = BandSettings { enabled: true, kind: BandType::LowCut, freq: 120.0, ..BandSettings::default() };
    let history = HistoryTap::new(1024);
    let mut engine = CompEngine::new(sr, &s);
    let key = vec![0.2f32; 256];
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for i in 0..2400 {
        // Everything switching all along: Characters, Smart, topology, stereo, link, lookahead,
        // the key and its EQ, Listen, solo, and the output stage.
        s.style = STYLES[(i / 40) % STYLES.len()];
        s.smart = Smart::from_index((i / 90) % 4);
        s.feedback = (i / 70) % 2 == 1;
        s.mid_side = (i / 110) % 2 == 1;
        s.link = if (i / 60) % 2 == 0 { 1.0 } else { 0.3 };
        s.lookahead_ms = [0.0, 5.0, 20.0][(i / 130) % 3];
        s.external_key = (i / 150) % 2 == 1;
        s.key_bands[1] = BandSettings { enabled: (i / 50) % 2 == 0, kind: BandType::Bell, freq: 300.0 + (i % 7) as f64 * 100.0, gain_db: -4.0, ..BandSettings::default() };
        s.listen = (i / 170) % 5 == 4;
        s.solo = if (i / 190) % 3 == 2 { Some(2) } else { None };
        s.stages[2].threshold_db = -30.0 + ((i / 20) % 20) as f32;
        s.stages[3].on = (i / 45) % 2 == 0;
        let out = Output { mix: if (i / 80) % 2 == 0 { 1.0 } else { 0.5 }, delta: (i / 100) % 2 == 1, bypass: (i / 210) % 4 == 3, gain_db: -3.0, pan: 0.2 };
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 30) % 2 == 0 { 0.5 } else { 0.01 };
            *a = x;
            *b = -x;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        process_wrapper(|| {
            engine.process(&mut [&mut l[..block], &mut r[..block]], Some(&[&key[..block], &key[..block]]), &s, &out, &history);
        });
        // Mono blocks too.
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], Some(&[&key[..block]]), &s, &out, &history));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
    let _ = record::WIDTH;
}
