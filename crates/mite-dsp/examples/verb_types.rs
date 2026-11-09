//! Each reverb type's impulse response at the default settings (and a long one): its level
//! against the room, its measured T60 (the energy decay's −5…−35 dB slope), how soon it is dense,
//! how periodic and how peaky its tail is.
//!
//!     cargo run --release -p mite-dsp --example verb_types
use mite_dsp::reverb::{Reverb, ReverbKind, ReverbSettings};
use mite_dsp::reverb_quality::{dense_after, highpass, peakiness, periodic_lags, periodicity};

fn ir(s: &ReverbSettings, fs: f32, seconds: f32) -> Vec<f32> {
    let n = (fs * seconds) as usize;
    let mut r = Reverb::new(fs, s);
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    let (mut l, mut rr) = (vec![0.0f32; n], vec![0.0f32; n]);
    for ((a, b), c) in l.chunks_mut(512).zip(rr.chunks_mut(512)).zip(x.chunks(512)) {
        r.process([c, c], [a, b], true, s);
    }
    l
}

fn t60(v: &[f32], fs: f64) -> f64 {
    // Schroeder backward integration.
    let mut e: Vec<f64> = v.iter().map(|x| f64::from(*x).powi(2)).collect();
    for i in (0..e.len() - 1).rev() {
        e[i] += e[i + 1];
    }
    let db: Vec<f64> = e.iter().map(|x| 10.0 * (x / e[0]).max(1e-30).log10()).collect();
    let at = |lvl: f64| db.iter().position(|&d| d < lvl).unwrap_or(db.len()) as f64 / fs;
    2.0 * (at(-35.0) - at(-5.0))
}

fn main() {
    let fs = 48_000.0;
    let mut room_e = 0.0;
    for decay in [2.0f32, 6.0] {
        for k in 0..5 {
            let kind = ReverbKind::from_index(k);
            let s = ReverbSettings { kind, decay_s: decay, early: 0.0, ..ReverbSettings::default() };
            let l = ir(&s, fs, decay * 2.0 + 1.0);
            let h = highpass(&l, 150.0, f64::from(fs));
            let e: f64 = l.iter().map(|x| f64::from(*x).powi(2)).sum();
            if k == 0 {
                room_e = e;
            }
            println!(
                "{:8} decay {decay}: level {:+5.1} dB  T60 {:5.2} s  dense after {:4.0} ms  periodicity {:.3}  peaky {:.3} %",
                ReverbKind::NAMES[k],
                10.0 * (e / room_e).log10(),
                t60(&l, f64::from(fs)),
                dense_after(&h, f64::from(fs)) * 1000.0,
                periodicity(&h, f64::from(fs), 0.15, 0.65),
                peakiness(&h, f64::from(fs), 0.1, 0.9) * 100.0
            );
            if std::env::var_os("LAGS").is_some() && decay == 2.0 {
                println!("    lags {:?}", periodic_lags(&h, f64::from(fs), 0.15, 0.65, 4));
            }
        }
    }
}
