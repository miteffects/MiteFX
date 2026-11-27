//! A drum kit with known hits for Mite Cut's tests (as `mite-dsp`'s hits test): every drum
//! bleeds into every close mic, with a delay, a duller tone and ±1.5 dB from hit to hit.
#![allow(dead_code)]

pub const FS: f32 = 48_000.0;

pub struct Rng(pub u32);
impl Rng {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }
    pub fn bi(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Drum {
    Kick,
    Snare,
    Tom1,
    Tom2,
    Hat,
}

/// One hit of `d` at velocity `v`, dry.
pub fn voice(d: Drum, v: f32, r: &mut Rng) -> Vec<f32> {
    let (len, f0, f1, drop, decay, noise, ndecay) = match d {
        Drum::Kick => (0.35, 150.0, 52.0, 0.03, 0.15, 0.25, 0.004),
        Drum::Snare => (0.3, 210.0, 185.0, 0.02, 0.07, 0.8, 0.09),
        Drum::Tom1 => (0.6, 150.0, 118.0, 0.06, 0.3, 0.15, 0.01),
        Drum::Tom2 => (0.8, 100.0, 82.0, 0.08, 0.4, 0.15, 0.01),
        Drum::Hat => (0.08, 0.0, 0.0, 1.0, 1.0, 0.6, 0.02),
    };
    let n = (len * FS) as usize;
    let mut ph = 0.0f32;
    let mut hp = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / FS;
            let tone = if f0 > 0.0 {
                ph += std::f32::consts::TAU * (f1 + (f0 - f1) * (-t / drop).exp()) / FS;
                (-t / decay).exp() * ph.sin()
            } else {
                0.0
            };
            let w = r.bi();
            let hpw = w - hp; // a crude high-pass for the hat
            hp = w;
            let nz = if d == Drum::Hat { hpw * 0.5 } else { w };
            // (Faded over its last 50 ms: a voice cut off would click.)
            let tail = ((n - i) as f32 / (0.05 * FS)).min(1.0);
            v * tail * (tone + noise * (-t / ndecay).exp() * nz)
        })
        .collect()
}

/// The pattern: (drum, onset sample, velocity), 40 s at 120 bpm.
pub fn pattern(r: &mut Rng) -> Vec<(Drum, usize, f32)> {
    let eighth = (0.25 * FS) as usize;
    let mut p = Vec::new();
    for k in 0..160 {
        let at = 2000 + k * eighth;
        p.push((Drum::Hat, at, 0.25 + 0.15 * r.next()));
        if k % 4 == 0 || (k % 8 == 5 && r.next() < 0.5) {
            p.push((Drum::Kick, at, 0.7 + 0.3 * r.next()));
        }
        if k % 4 == 2 {
            p.push((Drum::Snare, at, 0.6 + 0.4 * r.next()));
        }
        // Toms: a fill every other bar (sixteenths on the last beat), and scattered hits,
        // velocity anywhere from 0.15 (−16 dB) to 1.
        let bar = k / 8;
        if bar % 2 == 1 && k % 8 >= 6 {
            for s in 0..2 {
                let d = if (k + s) % 2 == 0 {
                    Drum::Tom1
                } else {
                    Drum::Tom2
                };
                p.push((d, at + s * eighth / 2, 0.15 + 0.85 * r.next()));
            }
        } else if k % 8 == 3 && r.next() < 0.4 {
            p.push((
                if r.next() < 0.5 {
                    Drum::Tom1
                } else {
                    Drum::Tom2
                },
                at + eighth / 2,
                0.15 + 0.85 * r.next(),
            ));
        }
    }
    p
}

/// The true hits: (drum, onset sample, velocity).
pub type Truth = Vec<(Drum, usize, f32)>;

/// The mics: kick, snare, tom 1, tom 2, hat. `bleed[m][d]`: drum d in mic m (amplitude), the
/// delay in ms.
pub fn kit(seed: u32) -> (Vec<Vec<f32>>, Truth) {
    let drums = [Drum::Kick, Drum::Snare, Drum::Tom1, Drum::Tom2, Drum::Hat];
    let bleed = [
        [1.0, 0.25, 0.15, 0.2, 0.05],
        [0.2, 1.0, 0.3, 0.25, 0.4],
        [0.2, 0.55, 1.0, 0.4, 0.3],
        [0.25, 0.5, 0.4, 1.0, 0.25],
        [0.05, 0.4, 0.15, 0.1, 1.0],
    ];
    let delay_ms = [
        [0.0, 2.0, 2.5, 2.2, 3.0],
        [2.0, 0.0, 1.4, 2.0, 1.2],
        [2.5, 1.4, 0.0, 1.0, 1.8],
        [2.2, 2.0, 1.0, 0.0, 2.6],
        [3.0, 1.2, 1.8, 2.6, 0.0],
    ];
    let mut r = Rng(seed);
    let p = pattern(&mut r);
    let n = (42.0 * FS) as usize;
    let mut mics = vec![vec![0.0f32; n]; 5];
    for &(d, at, v) in &p {
        let di = drums.iter().position(|&x| x == d).unwrap();
        let dry = voice(d, v, &mut r);
        for m in 0..5 {
            let g = bleed[m][di]
                * if m == di {
                    1.0
                } else {
                    10f32.powf(1.5 * r.bi() / 20.0)
                };
            let off = at + (delay_ms[m][di] * 1e-3 * FS) as usize;
            // Bleed is duller: a one-pole low-pass at 2.5 kHz.
            let k = if m == di {
                1.0
            } else {
                1.0 - (-std::f32::consts::TAU * 2500.0 / FS).exp()
            };
            let mut s = 0.0f32;
            for (i, &x) in dry.iter().enumerate() {
                s += k * (x - s);
                if off + i < n {
                    mics[m][off + i] += g * s;
                }
            }
        }
    }
    for m in &mut mics {
        for v in m.iter_mut() {
            *v += 3e-4 * r.bi();
        }
    }
    (mics, p)
}


/// A kit with two mics on the kick (in; sub, hearing only the lows) and two on the snare (top;
/// bottom, its polarity inverted, more of the kick): kick in, kick sub, snare top, snare bottom,
/// tom 1, tom 2, hat.
pub fn kit_two_mics(seed: u32) -> (Vec<Vec<f32>>, Truth) {
    let drums = [Drum::Kick, Drum::Snare, Drum::Tom1, Drum::Tom2, Drum::Hat];
    let bleed: [[f32; 5]; 7] = [
        [1.0, 0.25, 0.15, 0.2, 0.05],
        [1.2, 0.12, 0.15, 0.25, 0.0],
        [0.2, 1.0, 0.3, 0.25, 0.4],
        [0.35, -0.8, 0.25, 0.2, 0.15],
        [0.2, 0.55, 1.0, 0.4, 0.3],
        [0.25, 0.5, 0.4, 1.0, 0.25],
        [0.05, 0.4, 0.15, 0.1, 1.0],
    ];
    let delay_ms = [[0.0, 2.0, 2.5, 2.2, 3.0], [0.8, 2.6, 3.0, 2.6, 3.4], [2.0, 0.0, 1.4, 2.0, 1.2], [1.6, 0.4, 1.6, 2.0, 1.6], [2.5, 1.4, 0.0, 1.0, 1.8], [2.2, 2.0, 1.0, 0.0, 2.6], [3.0, 1.2, 1.8, 2.6, 0.0]];
    // The sub mic: a low-pass at 150 Hz on all it hears.
    let lows = [None, Some(150.0f32), None, None, None, None, None];
    let mut r = Rng(seed);
    let p = pattern(&mut r);
    let n = (42.0 * FS) as usize;
    let mut mics = vec![vec![0.0f32; n]; 7];
    for &(d, at, v) in &p {
        let di = drums.iter().position(|&x| x == d).unwrap();
        let dry = voice(d, v, &mut r);
        for m in 0..7 {
            let own = matches!((m, di), (0 | 1, 0) | (2 | 3, 1) | (4, 2) | (5, 3) | (6, 4));
            let g = bleed[m][di] * if own { 1.0 } else { 10f32.powf(1.5 * r.bi() / 20.0) };
            if g == 0.0 {
                continue;
            }
            let off = at + (delay_ms[m][di] * 1e-3 * FS) as usize;
            let cut = lows[m].unwrap_or(if own { 24_000.0 } else { 2500.0 });
            let k = 1.0 - (-std::f32::consts::TAU * cut / FS).exp();
            let mut s = 0.0f32;
            for (i, &x) in dry.iter().enumerate() {
                s += k * (x - s);
                if off + i < n {
                    mics[m][off + i] += g * s;
                }
            }
        }
    }
    for m in &mut mics {
        for v in m.iter_mut() {
            *v += 3e-4 * r.bi();
        }
    }
    (mics, p)
}

