//! The engine's audio path must not allocate or free (BLUEPRINT §5), whatever the settings do.

use assert_no_alloc::assert_no_alloc as process_wrapper;
use mite_core::{HistoryTap, SpectrumTap};
use mite_dsp::eq::{BandSettings, BandType};
use mite_dsp::reverb::{DecayBand, DecayShape, ReverbSettings};
use mite_verb::dsp::{record, Output, VerbEngine};

#[test]
fn engine_process_never_allocates() {
    let sr = 48_000.0;
    let mut s = ReverbSettings::default();
    let history: HistoryTap<{ record::WIDTH }> = HistoryTap::new(1024);
    let mut engine = VerbEngine::new(sr, &s, &Output::default());
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let taps = [SpectrumTap::new(8192), SpectrumTap::new(8192)];
    for i in 0..1600 {
        s.size = ((i / 40) % 5) as f32 / 4.0;
        s.predelay_ms = [0.0, 30.0, 500.0][(i / 70) % 3];
        s.decay_s = [0.1, 2.0, 30.0][(i / 90) % 3];
        s.freeze = (i / 110) % 3 == 2;
        s.bands[0] = DecayBand { shape: DecayShape::from_index((i / 45) % 6), ..DecayBand::low_shelf(300.0, [0.25, 1.0, 4.0][(i / 50) % 3]) };
        s.bands[1] = DecayBand::high_shelf(1000.0 + (i % 7) as f32 * 2000.0, 0.5);
        s.bands[2].on = (i / 65) % 2 == 0;
        s.eq[0] = BandSettings { enabled: (i / 55) % 3 != 0, kind: BandType::from_index((i / 33) % 10), freq: 200.0 + (i % 9) as f64 * 900.0, gain_db: 6.0, slope: (i / 20) % 9, ..BandSettings::default() };
        s.duck_db = if (i / 130) % 2 == 0 { 0.0 } else { 12.0 };
        s.mod_depth = ((i / 60) % 2) as f32;
        let out = Output { mix: if (i / 80) % 2 == 0 { 1.0 } else { 0.3 }, bypass: (i / 150) % 4 == 3, gain_db: -2.0 };
        for (k, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let x = (((i * 256 + k) as f32) * 0.05).sin() * if (i / 7) % 2 == 0 { 0.5 } else { 0.001 };
            *a = x;
            *b = -x * 0.7;
        }
        let block = if i % 3 == 0 { 256 } else { 100 };
        process_wrapper(|| engine.process(&mut [&mut l[..block], &mut r[..block]], &s, &out, &history, Some([&taps[0], &taps[1]])));
        if i % 50 == 0 {
            process_wrapper(|| engine.process(&mut [&mut l[..block]], &s, &out, &history, None));
        }
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    }
    assert!(history.count() > 0);
}
