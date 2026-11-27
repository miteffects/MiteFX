//! What the Analyzer's phase page reads behind processors that are not filters (a gate, a
//! compressor, ...): a drum loop through each, measured as the editor does (16k and 64k frames,
//! 1/24 octave, 6 s average), printed per frequency: group delay (ms), coherence, magnitude.
//!
//! `cargo run --profile fasttest -p mite-dsp --example tv_probe`

use mite_dsp::dynamics::{CompSettings, Compressor};
use mite_dsp::gate::{Gate, GateSettings};
use mite_dsp::transfer::{average_weight, blend, Transfer, TransferPoint};

const FS: f64 = 48_000.0;
const POINTS: usize = 600;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A kick-in mic: kicks every 0.5 s (varied velocity, a pitch drop 160 → 50 Hz), snare and hat
/// bleed under them.
fn drums(seconds: f64) -> Vec<f32> {
    let n = (seconds * FS) as usize;
    let mut y = vec![0.0f32; n];
    let mut r = Rng(12345);
    let beat = (0.5 * FS) as usize;
    let mut at = 1000;
    while at < n {
        let vel = 0.55 + 0.45 * (0.5 + 0.5 * r.next());
        let mut ph = 0.0f64;
        for i in 0..(0.45 * FS) as usize {
            if at + i >= n {
                break;
            }
            let t = i as f64 / FS;
            let f = 50.0 + 110.0 * (-t / 0.025).exp();
            ph += std::f64::consts::TAU * f / FS;
            let env = (-t / 0.12).exp();
            let click = if i < 96 { 0.3 * r.next() as f64 * (1.0 - i as f64 / 96.0) } else { 0.0 };
            y[at + i] += (vel as f64 * (0.8 * env * ph.sin() + click)) as f32;
        }
        // Snare bleed on the off beat.
        let s = at + beat / 2;
        for i in 0..(0.2 * FS) as usize {
            if s + i >= n {
                break;
            }
            let t = i as f64 / FS;
            y[s + i] += (0.05 * (-t / 0.05).exp()) as f32 * r.next();
        }
        at += beat;
    }
    // Hats and room under everything.
    for v in &mut y {
        *v += 0.004 * r.next();
    }
    y
}

struct Pair {
    all: Transfer,
    loud: Transfer,
    quiet: Transfer,
    mean_db: Option<f64>,
}

impl Pair {
    fn new(n: usize) -> Self {
        let t = || Transfer::new(n, FS, POINTS, 1.0 / 24.0);
        Self { all: t(), loud: t(), quiet: t(), mean_db: None }
    }
    fn feed(&mut self, r: &[f32], m: &[f32], a: f64) {
        self.all.feed(&[r], &[m], a);
        let e = 10.0 * (r.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / r.len() as f64 + 1e-20).log10();
        let mean = *self.mean_db.get_or_insert(e);
        if e >= mean { self.loud.feed(&[r], &[m], a * 2.0) } else { self.quiet.feed(&[r], &[m], a * 2.0) };
        self.mean_db = Some(mean + (e - mean) * 0.05);
    }
    fn read(&mut self) -> [Vec<TransferPoint>; 3] {
        let mut o = [vec![TransferPoint::default(); POINTS], vec![TransferPoint::default(); POINTS], vec![TransferPoint::default(); POINTS]];
        self.all.read(0.0, &mut o[0]);
        self.loud.read(0.0, &mut o[1]);
        self.quiet.read(0.0, &mut o[2]);
        o
    }
}

/// All, loud frames, quiet frames.
fn measure(x: &[f32], y: &[f32], delay: usize) -> [Vec<TransferPoint>; 3] {
    let mut hi = Pair::new(16_384);
    let mut lo = Pair::new(65_536);
    let a_hi = average_weight(4096.0 / FS, 6.0);
    let a_lo = average_weight(16_384.0 / FS, 6.0);
    let mut end = 65_536 + delay;
    let mut k = 0;
    while end <= y.len() {
        let re = end - delay;
        hi.feed(&x[re - 16_384..re], &y[end - 16_384..end], a_hi);
        if k % 4 == 0 {
            lo.feed(&x[re - 65_536..re], &y[end - 65_536..end], a_lo);
        }
        k += 1;
        end += 4096;
    }
    let (h, l) = (hi.read(), lo.read());
    let mut out: [Vec<TransferPoint>; 3] = Default::default();
    for i in 0..3 {
        out[i] = vec![TransferPoint::default(); POINTS];
        blend(&l[i], &h[i], 180.0, 360.0, &mut out[i]);
    }
    out
}

fn report(name: &str, pts: &[Vec<TransferPoint>; 3]) {
    println!("== {name}");
    for f in [30.0, 40.0, 50.0, 60.0, 80.0, 100.0, 150.0, 250.0, 500.0, 1000.0, 3000.0, 8000.0] {
        let i = (0..POINTS).min_by(|&a, &b| (pts[0][a].freq / f).ln().abs().total_cmp(&(pts[0][b].freq / f).ln().abs())).unwrap();
        let (p, l, q) = (pts[0][i], pts[1][i], pts[2][i]);
        let h = |p: &TransferPoint| realfft::num_complex::Complex::from_polar(10f64.powf(p.magnitude_db / 20.0), p.phase);
        let spread = (h(&l) - h(&q)).norm() / h(&p).norm().max(1e-9);
        println!("{:>6.0} Hz  gd {:>7.2} ms  γ² {:.3}  {:>6.1} dB  spread {:.3} (own {:.3})  its noise {:.3}", p.freq, p.group_delay * 1e3, p.coherence, p.magnitude_db, spread, p.level_spread, p.level_noise);
    }
}

fn main() {
    let x = drums(60.0);
    let block = 256;

    let mut y = x.clone();
    let mut g = Gate::new(FS as f32);
    let gs = GateSettings { threshold_db: -30.0, release_ms: 80.0, hold_ms: 20.0, ..GateSettings::default() };
    g.reset(&gs);
    for c in y.chunks_mut(block) {
        g.process(&mut [c], None, &gs);
    }
    report("gate −30 dB, 80 ms release", &measure(&x, &y, Gate::latency_for(&gs, FS as f32)));

    let mut y = x.clone();
    let mut c = Compressor::new(FS as f32);
    let mut cs = CompSettings::default();
    cs.stages[0].on = true;
    cs.stages[0].threshold_db = -24.0;
    cs.stages[0].ratio = 6.0;
    cs.stages[0].attack_ms = 1.0;
    cs.stages[0].release_ms = 60.0;
    c.prepare(FS as f32, &cs);
    for b in y.chunks_mut(block) {
        c.process(&mut [b], None, &cs);
    }
    report("comp −24 dB 6:1, 1/60 ms", &measure(&x, &y, cs.latency(FS as f32)));

    let mut y = x.clone();
    let mut g = Gate::new(FS as f32);
    let gs = GateSettings { threshold_db: -18.0, release_ms: 15.0, hold_ms: 0.0, ..GateSettings::default() };
    g.reset(&gs);
    for c in y.chunks_mut(block) {
        g.process(&mut [c], None, &gs);
    }
    report("gate −18 dB, 15 ms release (cuts the tail)", &measure(&x, &y, Gate::latency_for(&gs, FS as f32)));

    // A static low shelf-ish filter: a one-pole low-pass mixed in (LTI).
    let mut y = x.clone();
    let k = 1.0 - (-std::f32::consts::TAU * 80.0 / FS as f32).exp();
    let mut s1 = 0.0f32;
    for v in &mut y {
        s1 += k * (*v - s1);
        *v += 1.0 * s1;
    }
    report("LTI: +6 dB under 80 Hz (one pole)", &measure(&x, &y, 0));

    let y: Vec<f32> = x.iter().map(|v| (2.0 * v).tanh() * 0.5).collect();
    report("tanh saturation", &measure(&x, &y, 0));

    let mut r = Rng(999);
    let y: Vec<f32> = x.iter().map(|v| v + 0.02 * r.next()).collect();
    report("LTI + noise (−34 dB)", &measure(&x, &y, 0));
    let mut r = Rng(998);
    let y: Vec<f32> = x.iter().map(|v| v + 0.1 * r.next()).collect();
    report("LTI + noise (−20 dB)", &measure(&x, &y, 0));
    // A chorus: half wet, a delay of 7 ms swinging ±3 ms at 0.6 Hz (time-variant, not by level).
    let mut y = x.clone();
    for i in 0..x.len() {
        let d = (7.0 + 3.0 * (std::f64::consts::TAU * 0.6 * i as f64 / FS).sin()) * 0.001 * FS;
        let p = i as f64 - d;
        if p >= 1.0 {
            let (k, f) = (p.floor() as usize, (p - p.floor()) as f32);
            y[i] = 0.5 * x[i] + 0.5 * (x[k] + (x[k + 1] - x[k]) * f);
        }
    }
    report("chorus, 50 % wet", &measure(&x, &y, 0));
    // A brickwall limiter 6 dB into it (1.5 ms lookahead, 50 ms release).
    let mut lim = mite_dsp::limit::Limiter::new(FS as f32, 480, 1);
    let ls = mite_dsp::limit::Settings { release_ms: 50.0, ..mite_dsp::limit::Settings::default() };
    lim.configure(72, false, ls);
    let y: Vec<f32> = x.iter().map(|v| lim.process([v * 2.0 * 1.4, 0.0], false).0[0] * 0.5).collect();
    report("limiter, 9 dB into it", &measure(&x, &y, lim.latency()));
    report("nothing (delay 0)", &measure(&x, &x, 0));
}
