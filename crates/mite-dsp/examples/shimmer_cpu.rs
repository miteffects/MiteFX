//! How much of one core the reverb takes per type, and with shimmer (48 kHz stereo, 256 blocks).
//!
//!     cargo run --release -p mite-dsp --example shimmer_cpu
use mite_dsp::reverb::{Reverb, ReverbKind, ReverbSettings};
fn main() {
    let fs = 48_000.0;
    for (kind, amt) in [(0usize, 0.0f32), (1, 0.0), (2, 0.0), (3, 0.0), (4, 0.0), (5, 0.5)] {
        let s = ReverbSettings { kind: ReverbKind::from_index(kind), shimmer: amt, ..ReverbSettings::default() };
        let mut r = Reverb::new(fs, &s);
        let x: Vec<f32> = (0..256).map(|i| ((i * 7919) % 256) as f32 / 256.0 - 0.5).collect();
        let (mut l, mut rr) = (vec![0.0; 256], vec![0.0; 256]);
        let secs = 10.0;
        let t = std::time::Instant::now();
        for _ in 0..(secs * fs / 256.0) as usize {
            r.process([&x, &x], [&mut l, &mut rr], true, &s);
        }
        println!("{:8} shimmer {amt}: {:.1} % of a core", ReverbKind::NAMES[kind], t.elapsed().as_secs_f64() / f64::from(secs) * 100.0);
    }
}
