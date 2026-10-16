//! An effect's output stage: the latency-aligned dry path (for mix, delta and bypass), output
//! balance, band solo (listening to one band's region) and loudness-based gain compensation.

use crate::eq::{analog_response_db, BandSettings, BandType, Placement};

/// The dry signal, delayed by the effect's latency so it lines up with the wet signal.
/// Sized once; writing and reading never allocate.
pub struct DryDelay {
    rings: [Vec<f32>; 2],
    mask: usize,
    write: usize,
    delay: usize,
}

impl DryDelay {
    /// A delay that can hold `max_delay` samples behind blocks of up to `max_block`. Allocates.
    pub fn new(max_delay: usize, max_block: usize) -> Self {
        let len = (max_delay + max_block + 1).next_power_of_two();
        Self { rings: [vec![0.0; len], vec![0.0; len]], mask: len - 1, write: 0, delay: 0 }
    }

    pub fn reset(&mut self) {
        for r in &mut self.rings {
            r.fill(0.0);
        }
        self.write = 0;
    }

    /// The delay, samples (clamped to what it was sized for).
    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay.min(self.mask);
    }

    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Stores the next block's input (one or two channels; a missing right copies the left).
    /// Call [`Self::read`] for this block's samples, then [`Self::advance`] by its length.
    pub fn write(&mut self, left: &[f32], right: Option<&[f32]>) {
        for (ch, input) in [left, right.unwrap_or(left)].into_iter().enumerate() {
            for (i, &x) in input.iter().enumerate() {
                self.rings[ch][(self.write + i) & self.mask] = x;
            }
        }
    }

    /// Sample `i` of the block last written, delayed, on channel `ch`.
    #[inline]
    pub fn read(&self, ch: usize, i: usize) -> f32 {
        self.rings[ch & 1][(self.write + i).wrapping_sub(self.delay) & self.mask]
    }

    pub fn advance(&mut self, frames: usize) {
        self.write = (self.write + frames) & self.mask;
    }
}

/// Output balance, `pan` −1 (left) … 1 (right): the centre leaves both channels at unity, and
/// the far side fades along a quarter sine (−3 dB halfway is reached at about ±0.4).
pub fn balance(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    if pan == 0.0 {
        return (1.0, 1.0);
    }
    // The far side: √2·sin of the remaining quarter turn, which is 1 at the centre.
    let far = (std::f32::consts::SQRT_2 * ((1.0 - pan.abs()) * std::f32::consts::FRAC_PI_4).sin()).min(1.0);
    if pan > 0.0 { (far, 1.0) } else { (1.0, far) }
}

/// What soloing `s` listens to: a filter over the band's region (a band pass at its frequency
/// and Q for bells, notches and band passes; below or above a shelf's corner; what a cut takes
/// away), or `None` where the band has no region (a tilt reaches everything).
pub fn solo_filter(s: &BandSettings) -> Option<BandSettings> {
    let (kind, slope) = match s.kind {
        BandType::Bell | BandType::Notch | BandType::BandPass => (BandType::BandPass, s.slope),
        BandType::LowShelf => (BandType::HighCut, 1),
        BandType::HighShelf => (BandType::LowCut, 1),
        BandType::LowCut => (BandType::HighCut, s.slope),
        BandType::HighCut => (BandType::LowCut, s.slope),
        BandType::TiltShelf => return None,
    };
    let q = if kind == BandType::BandPass { s.q } else { std::f64::consts::FRAC_1_SQRT_2 };
    Some(BandSettings { enabled: true, kind, slope, q, gain_db: 0.0, placement: Placement::Stereo, dynamics: Default::default(), ..*s })
}

/// Keeps only the part of a stereo pair that a band on `placement` processes (the left channel,
/// the mid, …), so a solo plays just what the band hears.
#[inline]
pub fn isolate(placement: Placement, l: f32, r: f32) -> (f32, f32) {
    match placement {
        Placement::Stereo => (l, r),
        Placement::Left => (l, 0.0),
        Placement::Right => (0.0, r),
        Placement::Mid => {
            let m = 0.5 * (l + r);
            (m, m)
        }
        Placement::Side => {
            let s = 0.5 * (l - r);
            (s, -s)
        }
    }
}

/// Points auto gain weighs, log-spaced over 20 Hz–20 kHz.
const LOUDNESS_POINTS: usize = 48;

/// How much louder (dB) the bands' static curves make pink noise, weighted as loudness is
/// heard (ITU-R BS.1770's K weighting: a high pass near 38 Hz and a +4 dB shelf above about
/// 1.5 kHz). Auto gain applies the negative of this. It reads the curves, not the signal, so it
/// never pumps. Bands on one channel or on mid or side count for the half of the power they
/// reach (exact for uncorrelated channels); with `mono` every band reaches everything.
pub fn loudness_change_db(bands: &[BandSettings], mono: bool) -> f64 {
    let (mut total, mut weights) = (0.0, 0.0);
    for i in 0..LOUDNESS_POINTS {
        let f = 20.0 * 1000f64.powf(i as f64 / (LOUDNESS_POINTS - 1) as f64);
        let hp = (f / 38.0).powi(4);
        let k = hp / (1.0 + hp) * (1.0 + (10f64.powf(0.4) - 1.0) * f * f / (f * f + 1500.0 * 1500.0));
        let (mut left, mut right, mut mid, mut side, mut all) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for s in bands.iter().filter(|s| s.enabled) {
            let db = analog_response_db(s, f);
            match (mono, s.placement) {
                (true, _) | (_, Placement::Stereo) => all += db,
                (_, Placement::Left) => left += db,
                (_, Placement::Right) => right += db,
                (_, Placement::Mid) => mid += db,
                (_, Placement::Side) => side += db,
            }
        }
        let p = |db: f64| 10f64.powf(0.1 * db.max(-200.0));
        let power = p(all) * 0.5 * (p(left) + p(right)) * 0.5 * (p(mid) + p(side));
        total += k * power;
        weights += k;
    }
    10.0 * (total / weights).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dry_path_lines_up_with_the_latency() {
        let mut d = DryDelay::new(100, 64);
        d.set_delay(37);
        let input: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let mut out = Vec::new();
        for block in input.chunks(64) {
            d.write(block, None);
            out.extend((0..block.len()).map(|i| d.read(0, i)));
            d.advance(block.len());
        }
        assert!(out[..37].iter().all(|&v| v == 0.0));
        assert_eq!(&out[37..], &input[..1000 - 37]);
    }

    #[test]
    fn balance_is_unity_in_the_centre_and_closes_the_far_side() {
        assert_eq!(balance(0.0), (1.0, 1.0));
        let (l, r) = balance(1.0);
        assert!(l.abs() < 1e-6 && r == 1.0);
        let (l, r) = balance(-1.0);
        assert!(l == 1.0 && r.abs() < 1e-6);
        // Monotone towards the side it closes.
        let mut last = 1.0;
        for i in 0..=20 {
            let (l, _) = balance(i as f32 / 20.0);
            assert!(l <= last + 1e-6);
            last = l;
        }
    }

    #[test]
    fn auto_gain_reads_the_curve_not_the_signal() {
        let flat = [BandSettings::default(); 4];
        assert!(loudness_change_db(&flat, false).abs() < 1e-9);
        let boost = BandSettings { enabled: true, kind: BandType::Bell, freq: 2000.0, gain_db: 6.0, q: 1.0, ..Default::default() };
        let up = loudness_change_db(&[boost], false);
        assert!(up > 0.5 && up < 6.0, "{up}");
        // A mirror-image cut takes away about as much.
        let down = loudness_change_db(&[BandSettings { gain_db: -6.0, ..boost }], false);
        assert!(down < -0.5 && (up + down).abs() < 0.6 * up, "{up} {down}");
        // A boost on one channel counts for less than one on both.
        let left = loudness_change_db(&[BandSettings { placement: Placement::Left, ..boost }], false);
        assert!(left > 0.0 && left < up, "{left} {up}");
        // A sub-sonic boost barely counts (K weighting).
        let sub = loudness_change_db(&[BandSettings { freq: 20.0, ..boost }], false);
        assert!(sub < 0.25 * up, "{sub}");
    }

    #[test]
    fn a_solo_listens_to_the_band_region() {
        let bell = BandSettings { enabled: true, kind: BandType::Bell, freq: 500.0, gain_db: -6.0, q: 4.0, ..Default::default() };
        let f = solo_filter(&bell).expect("bells have a region");
        assert_eq!((f.kind, f.freq, f.q, f.gain_db), (BandType::BandPass, 500.0, 4.0, 0.0));
        let shelf = solo_filter(&BandSettings { kind: BandType::HighShelf, ..bell }).expect("region");
        assert_eq!(shelf.kind, BandType::LowCut);
        assert!(solo_filter(&BandSettings { kind: BandType::TiltShelf, ..bell }).is_none());
        assert_eq!(isolate(Placement::Side, 1.0, 0.0), (0.5, -0.5));
        assert_eq!(isolate(Placement::Right, 1.0, 2.0), (0.0, 2.0));
    }
}
