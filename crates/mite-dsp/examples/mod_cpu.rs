//! Mite Mod's cost per mode: 10 s of stereo noise at 48 kHz in 512-sample blocks, as a share of
//! one core.
//!
//!     cargo run --release -p mite-dsp --example mod_cpu

use mite_dsp::modulation::{Mod, ModSettings, Mode, Transport};

fn main() {
    let fs = 48_000.0;
    let n = 480_000;
    let mut seed = 1u32;
    let x: Vec<f32> = (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 - 0.5
        })
        .collect();
    let d = ModSettings::default();
    for (name, s) in [
        ("chorus, 2 voices", ModSettings { voices: 2, ..d }),
        ("chorus, 4 voices", ModSettings { voices: 4, ..d }),
        ("ensemble", ModSettings { mode: Mode::Ensemble, ..d }),
        ("flanger", ModSettings { mode: Mode::Flanger, feedback: 0.6, ..d }),
        ("flanger, through zero", ModSettings { mode: Mode::Flanger, thru_zero: true, ..d }),
        ("phaser, 4 stages", ModSettings { mode: Mode::Phaser, ..d }),
        ("phaser, 24 stages", ModSettings { mode: Mode::Phaser, stages: 24, ..d }),
        ("vibrato", ModSettings { mode: Mode::Vibrato, ..d }),
        ("rotary (fast)", ModSettings { mode: Mode::Rotary, rotor: mite_dsp::modulation::Rotor::Fast, depth: 0.8, ..d }),
    ] {
        let mut m = Mod::new(fs, &s);
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let t0 = std::time::Instant::now();
        for c in x.chunks(512) {
            let k = c.len();
            m.process([c, c], [&mut l[..k], &mut r[..k]], true, &s, &Transport::default());
        }
        let used = t0.elapsed().as_secs_f64() / 10.0 * 100.0;
        println!("{name:24} {used:5.2} % of a core");
    }
}
