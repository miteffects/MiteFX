//! Test material for the examples: the drum loop Shape uses, with a bass line and a chord over it
//! (saturation wants something tonal to show its harmonics on).


/// `seconds` of the loop at `sample_rate`, stereo (the hats a little wider).
pub fn drum_loop(seconds: f32, sample_rate: f32) -> [Vec<f32>; 2] {
    let n = (seconds * sample_rate) as usize;
    let step = (sample_rate * 0.125) as usize;
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    let mut seed = 0x2545_F491_u32;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let mut hp = [0.0f32; 2];
    for s in 0..n / step + 1 {
        let k = s % 16;
        let at = s * step;
        let mut hit = |kind: u8, vel: f32| {
            let len = (sample_rate * 0.6) as usize;
            for i in 0..len.min(n.saturating_sub(at)) {
                let t = i as f32 / sample_rate;
                let (a, b) = match kind {
                    // Kick: a falling sine with a click.
                    0 => {
                        let f = 50.0 + 90.0 * (-t / 0.03).exp();
                        let v = (std::f32::consts::TAU * f * t).sin() * (-t / 0.22).exp() + 0.3 * noise() * (-t / 0.004).exp();
                        (v, v)
                    }
                    // Snare: a 190 Hz body and a noise tail.
                    1 => {
                        let v = 0.6 * (std::f32::consts::TAU * 190.0 * t).sin() * (-t / 0.07).exp() + 0.5 * noise() * (-t / 0.15).exp();
                        (v, v * 0.95)
                    }
                    // Hat: high-passed noise, short.
                    _ => {
                        let x = noise() * (-t / 0.045).exp();
                        let y = x - hp[0];
                        hp[0] = x;
                        let z = noise() * (-t / 0.045).exp();
                        let w = z - hp[1];
                        hp[1] = z;
                        (0.5 * y + 0.2 * w, 0.2 * y + 0.5 * w)
                    }
                };
                l[at + i] += a * vel * 0.4;
                r[at + i] += b * vel * 0.4;
            }
        };
        if k == 0 || k == 7 || k == 10 {
            hit(0, if k == 7 { 0.55 } else { 1.0 });
        }
        if k == 4 || k == 12 {
            hit(1, 1.0);
        }
        if k == 14 {
            hit(1, 0.3);
        }
        if k % 2 == 0 {
            hit(2, if k % 4 == 0 { 0.6 } else { 1.0 });
        }
    }
    [l, r]
}

/// The drum loop with a bass line (a sine an octave under the root, A1–D2–E2–A1, a bar each) and
/// a soft triad over it.
#[allow(dead_code)]
pub fn music(seconds: f32, sample_rate: f32) -> [Vec<f32>; 2] {
    let [mut l, mut r] = drum_loop(seconds, sample_rate);
    let bar = 2.0 * sample_rate;
    let roots = [55.0f32, 73.42, 82.41, 55.0];
    let mut phase = [0.0f32; 4];
    for i in 0..l.len() {
        let root = roots[((i as f32 / bar) as usize) % 4];
        let t = (i as f32 % (bar / 4.0)) / sample_rate;
        let env = (1.0 - (-t / 0.004).exp()) * (0.55 + 0.45 * (-t / 0.35).exp());
        let mut v = 0.0;
        for (k, mult) in [1.0f32, 4.0, 5.04, 6.0].into_iter().enumerate() {
            phase[k] = (phase[k] + root * mult / sample_rate).fract();
            let amp = if k == 0 { 0.32 * env } else { 0.05 };
            v += amp * (std::f32::consts::TAU * phase[k]).sin();
        }
        l[i] += v;
        r[i] += v;
    }
    [l, r]
}
