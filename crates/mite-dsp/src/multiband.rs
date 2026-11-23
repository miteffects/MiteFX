//! The multiband dynamics behind Mite Multiband (docs/design/multiband-brief.md §1): the shared
//! LR4 crossover (`crate::crossover`, bands summing to an allpass) into up to six bands, each with
//! one of Comp's stages (`crate::dynamics::Compressor`: any quadrant, threshold on the band's
//! input), a band gain, bypass, mute and solo; detected on the band itself or on the external
//! sidechain, EQ'd and split by the same crossovers.
//!
//! - **Mix and Delta** are against the dry signal through the same crossover (the allpass sum of
//!   the bands, delayed by the lookahead), so a partial mix never combs (BLUEPRINT §4.2).
//! - **Bypass** plays the input itself, delayed by the latency.
//! - **Band Delta** (any band's): only those bands are heard, each as what its dynamics changed
//!   (the band after them minus the band before, delayed alike).
//! - **Listen** plays the key as the external bands' detectors hear it: the sidechain after the
//!   sidechain EQ (split and summed, so an allpass of it).
//! - A structure change (a crossover on or off) restructures the split (`Split::restructure`
//!   carries the filter states, so the sum stays continuous); a new band's dynamics start from
//!   rest. Crossover frequencies glide in log terms.
//!
//! Nothing allocates outside [`Multiband::new`].

use crate::crossover::{Cross, FREQ_MAX, FREQ_MIN, Layout, Split};
use crate::dynamics::{CompSettings, Compressor, Stage, Style};
use crate::eq::{BandSettings, MAX_SECTIONS, Sections, design};
use crate::filter::Tdf2;
use crate::output::DryDelay;
use crate::{LinearSmoother, MINUS_INFINITY_DB, db_to_gain, gain_to_db};

pub const BANDS: usize = 6;
pub const CROSSOVERS: usize = BANDS - 1;
pub const DEFAULT_CROSSOVERS: [f32; CROSSOVERS] = [120.0, 500.0, 2000.0, 6000.0, 12_000.0];
/// The sidechain EQ's bands (Gate's).
pub const KEY_BANDS: usize = 6;
/// Longest chunk processed at once.
const CHUNK: usize = 256;
/// Crossover glide per sample (log frequency), output glides and switch fades.
const GLIDE: f64 = 0.002;
const GLIDE_MS: f32 = 20.0;
const SWITCH_MS: f32 = 10.0;

/// One band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MbBand {
    /// The band's dynamics (one of Comp's stages; `on` false: the band passes untouched).
    pub stage: Stage,
    pub gain_db: f32,
    pub bypass: bool,
    pub mute: bool,
    pub solo: bool,
    /// Detect on the external sidechain (EQ'd and split like the audio).
    pub external: bool,
    /// Hear only what this band's dynamics change.
    pub delta: bool,
}

impl Default for MbBand {
    fn default() -> Self {
        Self {
            stage: Stage::default(),
            gain_db: 0.0,
            bypass: false,
            mute: false,
            solo: false,
            external: false,
            delta: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MbSettings {
    /// Each crossover slot's frequency, Hz, or `None` while it is off.
    pub crossovers: [Option<f32>; CROSSOVERS],
    pub bands: [MbBand; BANDS],
    pub lookahead_ms: f32,
    /// Stereo link of every band's detector, 0…1.
    pub link: f32,
    /// The sidechain EQ (applied to the external key before it is split).
    pub key_eq: [BandSettings; KEY_BANDS],
    /// 0…1.
    pub mix: f32,
    pub out_db: f32,
    pub delta: bool,
    pub bypass: bool,
    /// Hear the EQ'd sidechain instead (monitoring).
    pub listen: bool,
    /// Each band's auto makeup (Comp's: its loudness out matched to its loudness in), so the
    /// bands, and their sum, stay as loud as they came in.
    pub auto_gain: bool,
}

impl Default for MbSettings {
    fn default() -> Self {
        Self {
            crossovers: [None; CROSSOVERS],
            bands: [MbBand::default(); BANDS],
            lookahead_ms: 0.0,
            link: 1.0,
            key_eq: [BandSettings::default(); KEY_BANDS],
            mix: 1.0,
            out_db: 0.0,
            delta: false,
            bypass: false,
            listen: false,
            auto_gain: false,
        }
    }
}

/// What a block did, per band slot and overall.
#[derive(Debug, Clone, Copy)]
pub struct MbTelemetry {
    pub in_peak_db: [f32; 2],
    pub out_peak_db: [f32; 2],
    /// Each band's peak going in (the louder channel), dBFS.
    pub band_in_db: [f32; BANDS],
    /// Each band's gain change at its most (signed, dB: cut ≤ 0 or lift ≥ 0, whichever is larger).
    pub band_gain_db: [f32; BANDS],
    /// Each band's detector level at its highest (the detector its stage uses), dBFS: what its
    /// threshold is compared with.
    pub band_det_db: [f32; BANDS],
    /// Each band's auto makeup at the end of the block, dB (0 while auto gain is off).
    pub band_auto_db: [f32; BANDS],
}

impl Default for MbTelemetry {
    fn default() -> Self {
        Self {
            in_peak_db: [MINUS_INFINITY_DB; 2],
            out_peak_db: [MINUS_INFINITY_DB; 2],
            band_in_db: [MINUS_INFINITY_DB; BANDS],
            band_gain_db: [0.0; BANDS],
            band_det_db: [MINUS_INFINITY_DB; BANDS],
            band_auto_db: [0.0; BANDS],
        }
    }
}

/// Which bands are heard: the soloed ones if any, else all but the muted.
pub fn audible(s: &MbSettings, layout: &Layout<BANDS>) -> [bool; BANDS] {
    let live = |b: usize| layout.position(b).is_some();
    let any_solo = (0..BANDS).any(|b| live(b) && s.bands[b].solo);
    std::array::from_fn(|b| {
        live(b)
            && if any_solo {
                s.bands[b].solo
            } else {
                !s.bands[b].mute
            }
    })
}

/// The band's compressor settings: its stage (off while bypassed), its gain as makeup.
fn comp_settings(s: &MbSettings, b: usize) -> CompSettings {
    comp_settings_at(s, b, s.lookahead_ms)
}

fn comp_settings_at(s: &MbSettings, b: usize, lookahead_ms: f32) -> CompSettings {
    let band = &s.bands[b];
    let mut stages = [Stage::default(); crate::dynamics::STAGES];
    stages[0] = Stage {
        on: band.stage.on && !band.bypass,
        ..band.stage
    };
    CompSettings {
        stages,
        style: Style::default(),
        link: s.link,
        lookahead_ms,
        makeup_db: if band.bypass { 0.0 } else { band.gain_db },
        auto_makeup: s.auto_gain && !band.bypass,
        external_key: band.external,
        ..CompSettings::default()
    }
}

pub struct Multiband {
    fs: f64,
    layout: Layout<BANDS>,
    split: Split<BANDS>,
    key_split: Split<BANDS>,
    cross: [Cross; CROSSOVERS],
    log_freq: [f64; CROSSOVERS],
    comps: Vec<Compressor>,
    key_sections: [Sections; KEY_BANDS],
    key_designed: [BandSettings; KEY_BANDS],
    key_filters: [[[Tdf2; MAX_SECTIONS]; KEY_BANDS]; 2],
    /// Band audio and band key per slot and channel, one chunk.
    band: Vec<[Vec<f32>; 2]>,
    key: Vec<[Vec<f32>; 2]>,
    /// The allpass sum before the dynamics (for mix and Delta) and the input (for bypass).
    dry_ap: DryDelay,
    dry_in: DryDelay,
    /// Each band before its dynamics, delayed alike (band Delta), and the EQ'd key's sum (Listen).
    dry_band: Vec<DryDelay>,
    key_sum: [Vec<f32>; 2],
    sum_scratch: [Vec<f32>; 2],
    mix: LinearSmoother,
    out: LinearSmoother,
    delta: LinearSmoother,
    bypass: LinearSmoother,
    /// A lookahead change: fade out, reset every band at the new lookahead, fade back in.
    dip: LinearSmoother,
    lookahead_ms: f32,
}

impl Multiband {
    /// Allocates.
    pub fn new(sample_rate: f32) -> Self {
        let fs = f64::from(sample_rate);
        let max = (crate::dynamics::MAX_LOOKAHEAD_MS * 0.001 * sample_rate).ceil() as usize + 2;
        let mut m = Self {
            fs,
            layout: Layout::new(&[None; CROSSOVERS]),
            split: Split::default(),
            key_split: Split::default(),
            cross: DEFAULT_CROSSOVERS.map(|f| Cross::new(f64::from(f), fs)),
            log_freq: DEFAULT_CROSSOVERS.map(|f| f64::from(f).ln()),
            comps: (0..BANDS).map(|_| Compressor::new(sample_rate)).collect(),
            key_sections: [Sections::default(); KEY_BANDS],
            key_designed: [BandSettings::default(); KEY_BANDS],
            key_filters: [[[Tdf2::default(); MAX_SECTIONS]; KEY_BANDS]; 2],
            band: (0..BANDS)
                .map(|_| [vec![0.0; CHUNK], vec![0.0; CHUNK]])
                .collect(),
            key: (0..BANDS)
                .map(|_| [vec![0.0; CHUNK], vec![0.0; CHUNK]])
                .collect(),
            dry_ap: DryDelay::new(max, CHUNK),
            dry_in: DryDelay::new(max, CHUNK),
            dry_band: (0..BANDS).map(|_| DryDelay::new(max, CHUNK)).collect(),
            key_sum: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            sum_scratch: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            mix: LinearSmoother::new(1.0),
            out: LinearSmoother::new(1.0),
            delta: LinearSmoother::new(0.0),
            bypass: LinearSmoother::new(0.0),
            dip: LinearSmoother::new_s_curve(1.0),
            lookahead_ms: 0.0,
        };
        for sm in [&mut m.mix, &mut m.out] {
            sm.set_ramp(sample_rate, GLIDE_MS);
        }
        for sm in [&mut m.delta, &mut m.bypass, &mut m.dip] {
            sm.set_ramp(sample_rate, SWITCH_MS);
        }
        // The only allocation: each band's delay lines.
        let s = MbSettings::default();
        for (b, c) in m.comps.iter_mut().enumerate() {
            c.prepare(sample_rate, &comp_settings(&s, b));
        }
        m.reset(&s);
        m
    }

    /// Clears the state and snaps every glide to `s`. No allocation.
    #[allow(clippy::needless_range_loop)] // crossover slots index three arrays
    pub fn reset(&mut self, s: &MbSettings) {
        self.layout = Layout::new(&s.crossovers);
        self.split.reset();
        self.key_split.reset();
        for k in 0..CROSSOVERS {
            let f = s.crossovers[k].unwrap_or(DEFAULT_CROSSOVERS[k]);
            self.log_freq[k] = f64::from(f).ln();
            self.cross[k] = Cross::new(f64::from(f), self.fs);
        }
        self.lookahead_ms = s.lookahead_ms;
        for (b, c) in self.comps.iter_mut().enumerate() {
            c.reset(&comp_settings(s, b));
        }
        self.dip.reset(1.0);
        self.key_filters = [[[Tdf2::default(); MAX_SECTIONS]; KEY_BANDS]; 2];
        self.design_key(s, true);
        self.dry_ap.reset();
        self.dry_in.reset();
        for d in &mut self.dry_band {
            d.reset();
        }
        self.set_delays();
        self.mix.reset(s.mix);
        self.out.reset(db_to_gain(s.out_db));
        self.delta.reset(if s.delta { 1.0 } else { 0.0 });
        self.bypass.reset(if s.bypass { 1.0 } else { 0.0 });
    }

    fn design_key(&mut self, s: &MbSettings, force: bool) {
        for (i, band) in s.key_eq.iter().enumerate() {
            if force || *band != self.key_designed[i] {
                self.key_sections[i] = design(band, self.fs);
                self.key_designed[i] = *band;
            }
        }
    }

    fn set_delays(&mut self) {
        let l = self.latency();
        self.dry_ap.set_delay(l);
        self.dry_in.set_delay(l);
        for d in &mut self.dry_band {
            d.set_delay(l);
        }
    }

    /// The latency (the lookahead), samples.
    pub fn latency(&self) -> usize {
        self.comps[0].latency()
    }

    /// The band layout in force.
    pub fn layout(&self) -> Layout<BANDS> {
        self.layout
    }

    /// Processes `channels` (one or two) in place; `key` is the sidechain (silent without it).
    #[allow(clippy::needless_range_loop)] // channels, slots and positions indexed together
    pub fn process(
        &mut self,
        channels: &mut [&mut [f32]],
        key: Option<&[&[f32]]>,
        s: &MbSettings,
    ) -> MbTelemetry {
        let mut t = MbTelemetry::default();
        let n_ch = channels.len().min(2);
        if n_ch == 0 {
            return t;
        }
        let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
        self.design_key(s, false);
        // A structure change: carry the split's states over, start new bands from rest.
        let layout = Layout::new(&s.crossovers);
        if layout != self.layout {
            for k in 0..CROSSOVERS {
                let was_on = self.layout.crossovers[..self.layout.n].contains(&k);
                if !was_on && let Some(f) = s.crossovers[k] {
                    self.log_freq[k] = f64::from(f).ln();
                    self.cross[k] = Cross::new(f64::from(f), self.fs);
                }
            }
            // A merged band hands what it delayed to the band that takes its region (its lower
            // crossover's frequency, still held in `log_freq`), so nothing goes missing for the
            // lookahead's length.
            for slot in 1..BANDS {
                if self.layout.position(slot).is_some() && layout.position(slot).is_none() {
                    let to = layout.band_at(
                        &s.crossovers,
                        (self.log_freq[slot - 1].exp() * 1.001) as f32,
                    );
                    if to != slot {
                        let (lo, hi) = self.comps.split_at_mut(slot.max(to));
                        let (dst, src) = if to < slot {
                            (&mut lo[to], &hi[0])
                        } else {
                            (&mut hi[0], &lo[slot])
                        };
                        dst.absorb_delayed(src);
                        let (lo, hi) = self.dry_band.split_at_mut(slot.max(to));
                        let (dst, src) = if to < slot {
                            (&mut lo[to], &hi[0])
                        } else {
                            (&mut hi[0], &lo[slot])
                        };
                        dst.absorb(src);
                    }
                }
            }
            let parents = self.split.restructure(&self.layout, &layout, &s.crossovers);
            self.key_split
                .restructure(&self.layout, &layout, &s.crossovers);
            // A new band starts from rest, its delays empty (the band it came from plays out the
            // whole region it had delayed).
            for (slot, parent) in parents.iter().enumerate() {
                if parent.is_some() {
                    self.comps[slot].reset(&comp_settings(s, slot));
                    self.dry_band[slot].reset();
                }
            }
            self.layout = layout;
        }
        let heard = audible(s, &layout);
        let any_external =
            s.listen || (0..BANDS).any(|b| layout.position(b).is_some() && s.bands[b].external);
        let any_delta = (0..BANDS).any(|b| heard[b] && s.bands[b].delta);
        let key_len = key.map_or(0, |k| k.iter().map(|c| c.len()).min().unwrap_or(0));
        self.mix.set_target(s.mix);
        self.out.set_target(db_to_gain(s.out_db));
        self.delta.set_target(if s.delta { 1.0 } else { 0.0 });
        self.bypass.set_target(if s.bypass { 1.0 } else { 0.0 });
        let mut peak_in = [0.0f32; 2];
        let mut peak_out = [0.0f32; 2];
        let mut band_in = [0.0f32; BANDS];
        let mut start = 0;
        while start < len {
            // A lookahead change: fade out, switch, fade back in. The bands keep their state:
            // their delay lines are always written, so the new lookahead reads real audio (a
            // reset emptied them, and the sound jumped back in a lookahead after the switch,
            // half way up the fade: the §10 ramp test).
            if s.lookahead_ms != self.lookahead_ms {
                self.dip.set_target(0.0);
                if self.dip.current() == 0.0 {
                    self.lookahead_ms = s.lookahead_ms;
                    for (b, c) in self.comps.iter_mut().enumerate() {
                        c.follow_lookahead(&comp_settings(s, b));
                    }
                    self.set_delays();
                    self.dip.set_target(1.0);
                }
            }
            let n = (len - start).min(CHUNK);
            let end = start + n;
            // 1. Glide the crossovers; split the audio (and the key) into the band slots.
            for i in 0..n {
                for k in 0..CROSSOVERS {
                    if let Some(f) = s.crossovers[k] {
                        let target = f64::from(f.clamp(FREQ_MIN, FREQ_MAX)).ln();
                        let d = target - self.log_freq[k];
                        if d.abs() > 1e-6 {
                            self.log_freq[k] = if d.abs() < 1e-4 {
                                target
                            } else {
                                self.log_freq[k] + GLIDE * d
                            };
                            self.cross[k] = Cross::new(self.log_freq[k].exp(), self.fs);
                        }
                    }
                }
                let mut sig = [0.0f64; BANDS];
                for c in 0..n_ch {
                    let x = channels[c][start + i];
                    peak_in[c] = peak_in[c].max(x.abs());
                    self.split
                        .process(c, f64::from(x), &layout, &self.cross, &mut sig);
                    let mut sum = 0.0f64;
                    for p in 0..=layout.n {
                        let slot = layout.owners[p];
                        self.band[slot][c][i] = sig[p] as f32;
                        band_in[slot] = band_in[slot].max(sig[p].abs() as f32);
                        sum += sig[p];
                    }
                    self.sum_scratch[c][i] = sum as f32;
                    if any_external {
                        let kx = match key {
                            Some(k) if !k.is_empty() && start + i < key_len => {
                                k[c.min(k.len() - 1)][start + i]
                            }
                            _ => 0.0,
                        };
                        let mut y = f64::from(kx);
                        for (b, sections) in self.key_sections.iter().enumerate() {
                            for (st, coeffs) in
                                self.key_filters[c][b].iter_mut().zip(sections.as_slice())
                            {
                                y = st.process(coeffs, y);
                            }
                        }
                        self.key_split.process(c, y, &layout, &self.cross, &mut sig);
                        let mut ks = 0.0f64;
                        for p in 0..=layout.n {
                            self.key[layout.owners[p]][c][i] = sig[p] as f32;
                            ks += sig[p];
                        }
                        self.key_sum[c][i] = ks as f32;
                    }
                }
            }
            self.dry_ap.write(
                &self.sum_scratch[0][..n],
                (n_ch == 2).then(|| &self.sum_scratch[1][..n]),
            );
            self.dry_in.write(
                &channels[0][start..end],
                (n_ch == 2).then(|| &channels[1][start..end]),
            );
            // Every live band's dry path stays current (a Delta switched on, or a merge, reads it).
            for p in 0..=layout.n {
                let slot = layout.owners[p];
                let [a, b] = &self.band[slot];
                self.dry_band[slot].write(&a[..n], (n_ch == 2).then(|| &b[..n]));
            }
            // 2. Each band's dynamics, in place on its slot.
            for p in 0..=layout.n {
                let slot = layout.owners[p];
                let cs = comp_settings_at(s, slot, self.lookahead_ms);
                let [a, b] = &mut self.band[slot];
                let mut parts: [&mut [f32]; 2] = [&mut a[..n], &mut b[..n]];
                let [ka, kb] = &self.key[slot];
                let keys: [&[f32]; 2] = [&ka[..n], &kb[..n]];
                let tel = self.comps[slot].process(
                    &mut parts[..n_ch],
                    cs.external_key.then_some(&keys[..n_ch]),
                    &cs,
                );
                t.band_det_db[slot] = t.band_det_db[slot]
                    .max(tel.detector_levels_db[s.bands[slot].stage.detector as usize]);
                t.band_auto_db[slot] = self.comps[slot].auto_db();
                for c in 0..n_ch {
                    let (cut, lift) = (tel.cut_db[c], tel.lift_db[c]);
                    let g = if -cut >= lift { cut } else { lift };
                    if g.abs() > t.band_gain_db[slot].abs() {
                        t.band_gain_db[slot] = g;
                    }
                }
            }
            // 3. The sum of what is heard, then mix, Delta and bypass against the dry paths.
            for i in 0..n {
                let (mix, delta, bypass) = (
                    self.mix.next_value(),
                    self.delta.next_value(),
                    self.bypass.next_value(),
                );
                let out = self.out.next_value() * self.dip.next_value();
                for c in 0..n_ch {
                    let mut wet = 0.0f32;
                    for p in 0..=layout.n {
                        let slot = layout.owners[p];
                        if !heard[slot] {
                            continue;
                        }
                        if any_delta {
                            if s.bands[slot].delta {
                                wet += self.band[slot][c][i] - self.dry_band[slot].read(c, i);
                            }
                        } else {
                            wet += self.band[slot][c][i];
                        }
                    }
                    if s.listen {
                        channels[c][start + i] = self.key_sum[c][i];
                        peak_out[c] = peak_out[c].max(self.key_sum[c][i].abs());
                        continue;
                    }
                    let dry = self.dry_ap.read(c, i);
                    let mixed = dry * (1.0 - mix) + wet * mix;
                    let y = (mixed * (1.0 - delta) + (wet - dry) * delta) * out;
                    let y = y * (1.0 - bypass) + self.dry_in.read(c, i) * bypass;
                    channels[c][start + i] = y;
                    peak_out[c] = peak_out[c].max(y.abs());
                }
            }
            self.dry_ap.advance(n);
            self.dry_in.advance(n);
            for p in 0..=layout.n {
                self.dry_band[layout.owners[p]].advance(n);
            }
            start = end;
        }
        for c in 0..2 {
            let src = c.min(n_ch - 1);
            t.in_peak_db[c] = gain_to_db(peak_in[src]);
            t.out_peak_db[c] = gain_to_db(peak_out[src]);
        }
        for b in 0..BANDS {
            t.band_in_db[b] = gain_to_db(band_in[b]);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamics::Quadrant;

    const FS: f32 = 48000.0;

    fn noise(n: usize, db: f32) -> Vec<f32> {
        let a = db_to_gain(db);
        let mut seed = 0x9E37_79B9u32;
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                a * ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    fn run(m: &mut Multiband, s: &MbSettings, x: &[f32], block: usize) -> Vec<f32> {
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            m.process(&mut [a, b], None, s);
        }
        l
    }

    fn three_bands() -> MbSettings {
        let mut s = MbSettings::default();
        s.crossovers[0] = Some(200.0);
        s.crossovers[1] = Some(3000.0);
        s
    }

    #[test]
    fn one_band_and_neutral_is_the_input_delayed_bit_for_bit() {
        let s = MbSettings {
            lookahead_ms: 2.0,
            ..MbSettings::default()
        };
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let x = noise(9600, -12.0);
        let y = run(&mut m, &s, &x, 333);
        let d = m.latency();
        assert_eq!(d, 96);
        for i in d..x.len() {
            assert_eq!(y[i], x[i - d], "sample {i}");
        }
    }

    #[test]
    fn inert_bands_sum_flat_and_mix_never_combs() {
        // Neutral bands: the allpass sum. Its spectrum is flat, so its power equals the input's.
        for mix in [1.0, 0.5, 0.0] {
            let s = MbSettings {
                mix,
                ..three_bands()
            };
            let mut m = Multiband::new(FS);
            m.reset(&s);
            let x = noise(48000, -12.0);
            let y = run(&mut m, &s, &x, 256);
            let p = |v: &[f32]| v[9600..].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            let ratio_db = 10.0 * (p(&y) / p(&x[..x.len()])).log10() - 10.0 * (1.0f64).log10();
            assert!(ratio_db.abs() < 0.1, "mix {mix}: {ratio_db} dB");
        }
    }

    #[test]
    fn a_band_compresses_only_its_region() {
        // Compress the middle band hard (slot 1: crossover slot 0 owns band slot 1); a low tone
        // (band slot 0) passes, a 1 kHz tone drops.
        let mut s = three_bands();
        s.bands[1].stage = Stage {
            on: true,
            quadrant: Quadrant::Compress,
            threshold_db: -40.0,
            ratio: 20.0,
            knee_db: 0.0,
            range_db: 60.0,
            ..Stage::default()
        };
        let tone = |f: f32| -> Vec<f32> {
            (0..48000)
                .map(|i| 0.3 * (std::f32::consts::TAU * f * i as f32 / FS).sin())
                .collect()
        };
        let level = |v: &[f32]| {
            10.0 * (v[24000..]
                .iter()
                .map(|s| f64::from(*s).powi(2))
                .sum::<f64>()
                / 24000.0)
                .log10()
        };
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let low = tone(60.0);
        let y = run(&mut m, &s, &low, 256);
        assert!(
            (level(&y) - level(&low)).abs() < 0.3,
            "the low band is untouched: {} vs {}",
            level(&y),
            level(&low)
        );
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let mid = tone(1000.0);
        let y = run(&mut m, &s, &mid, 256);
        assert!(
            level(&y) < level(&mid) - 10.0,
            "the middle band is compressed: {} vs {}",
            level(&y),
            level(&mid)
        );
    }

    #[test]
    fn solo_and_mute_pick_what_is_heard() {
        let mut s = three_bands();
        s.bands[0].solo = true;
        let layout = Layout::new(&s.crossovers);
        assert_eq!(
            audible(&s, &layout),
            [true, false, false, false, false, false]
        );
        s.bands[0].solo = false;
        s.bands[1].mute = true;
        assert_eq!(
            audible(&s, &layout),
            [true, false, true, false, false, false]
        );
    }

    #[test]
    fn bypass_is_the_input_and_delta_is_what_changed() {
        let mut s = three_bands();
        s.bands[0].stage = Stage {
            on: true,
            quadrant: Quadrant::Compress,
            threshold_db: -30.0,
            ratio: 4.0,
            ..Stage::default()
        };
        let x = noise(24000, -6.0);
        let mut m = Multiband::new(FS);
        m.reset(&MbSettings { bypass: true, ..s });
        let y = run(&mut m, &MbSettings { bypass: true, ..s }, &x, 256);
        assert_eq!(y, x);
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let wet = run(&mut m, &s, &x, 256);
        let mut m = Multiband::new(FS);
        m.reset(&MbSettings { delta: true, ..s });
        let d = run(&mut m, &MbSettings { delta: true, ..s }, &x, 256);
        // Delta + the allpass dry = the wet signal.
        let mut m = Multiband::new(FS);
        let neutral = MbSettings {
            bands: [MbBand::default(); BANDS],
            ..s
        };
        m.reset(&neutral);
        let dry = run(&mut m, &neutral, &x, 256);
        for i in 0..x.len() {
            assert!((d[i] + dry[i] - wet[i]).abs() < 1e-5, "sample {i}");
        }
    }

    #[test]
    fn band_delta_is_what_that_band_changed() {
        // Band 1 compressed; its Delta is its output minus its input, delayed alike: with the
        // other bands' Delta off they are not heard.
        let mut s = three_bands();
        s.lookahead_ms = 1.0;
        s.bands[0].stage = Stage {
            on: true,
            quadrant: Quadrant::Compress,
            threshold_db: -40.0,
            ratio: 8.0,
            ..Stage::default()
        };
        let x = noise(24000, -6.0);
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let mut solo = s;
        solo.bands[0].solo = true;
        let mut m2 = Multiband::new(FS);
        m2.reset(&solo);
        let wet = run(&mut m2, &solo, &x, 256);
        let mut bypassed = solo;
        bypassed.bands[0].bypass = true;
        let mut m3 = Multiband::new(FS);
        m3.reset(&bypassed);
        let dry = run(&mut m3, &bypassed, &x, 256);
        let mut delta = s;
        delta.bands[0].delta = true;
        m.reset(&delta);
        let d = run(&mut m, &delta, &x, 256);
        for i in 0..x.len() {
            assert!(
                (d[i] - (wet[i] - dry[i])).abs() < 1e-5,
                "sample {i}: {} vs {}",
                d[i],
                wet[i] - dry[i]
            );
        }
        assert!(
            d.iter().any(|v| v.abs() > 1e-3),
            "the band does change something"
        );
    }

    #[test]
    fn listen_plays_the_eqd_key() {
        let s = MbSettings {
            listen: true,
            ..three_bands()
        };
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        let key = noise(4800, -12.0);
        let k: [&[f32]; 2] = [&key, &key];
        m.process(&mut [&mut l, &mut r], Some(&k), &s);
        let p = |v: &[f32]| v[2400..].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
        assert!(
            (10.0 * (p(&l) / p(&key)).log10()).abs() < 0.3,
            "the key's level, through the allpass"
        );
    }

    #[test]
    fn a_band_can_appear_and_merge_while_playing() {
        let mut s = MbSettings::default();
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let x = noise(48000, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        for (k, (a, b)) in l.chunks_mut(512).zip(r.chunks_mut(512)).enumerate() {
            s.crossovers[0] = (k % 20 >= 10).then_some(300.0);
            s.crossovers[3] = (k % 14 >= 7).then_some(5000.0);
            m.process(&mut [a, b], None, &s);
        }
        assert!(l.iter().all(|v| v.is_finite() && v.abs() < 2.0));
    }

    #[test]
    fn auto_gain_brings_each_band_back_to_its_loudness() {
        // Every band compressed hard: without auto gain the sum drops by many dB; with it the
        // sum, and a tone alone in one band, come out as loud as they went in.
        let mut s = three_bands();
        for b in &mut s.bands {
            b.stage = Stage {
                on: true,
                quadrant: Quadrant::Compress,
                threshold_db: -36.0,
                ratio: 6.0,
                ..Stage::default()
            };
        }
        let level = |v: &[f32]| {
            10.0 * (v[v.len() / 2..]
                .iter()
                .map(|s| f64::from(*s).powi(2))
                .sum::<f64>()
                / (v.len() / 2) as f64)
                .log10()
        };
        let x = noise(6 * 48000, -12.0);
        let tone: Vec<f32> = (0..6 * 48000)
            .map(|i| 0.3 * (std::f32::consts::TAU * 80.0 * i as f32 / FS).sin())
            .collect();
        for (name, sig) in [("noise", &x), ("80 Hz tone", &tone)] {
            let mut m = Multiband::new(FS);
            m.reset(&s);
            let off = run(&mut m, &s, sig, 256);
            let on_s = MbSettings {
                auto_gain: true,
                ..s
            };
            let mut m = Multiband::new(FS);
            m.reset(&on_s);
            let on = run(&mut m, &on_s, sig, 256);
            let (l_in, l_off, l_on) = (level(sig), level(&off), level(&on));
            println!("{name}: in {l_in:.2} off {l_off:.2} auto {l_on:.2} dB");
            assert!(l_off < l_in - 6.0, "{name}: compressed without auto gain");
            assert!((l_on - l_in).abs() < 1.0, "{name}: {l_on} vs {l_in}");
        }
    }

    #[test]
    fn auto_gain_leaves_a_bypassed_band_alone_and_reports_the_makeup() {
        let mut s = MbSettings {
            auto_gain: true,
            ..three_bands()
        };
        s.bands[0].stage = Stage {
            on: true,
            quadrant: Quadrant::Compress,
            threshold_db: -60.0,
            ratio: 6.0,
            ..Stage::default()
        };
        s.bands[1].bypass = true;
        let mut m = Multiband::new(FS);
        m.reset(&s);
        let x = noise(48000, -12.0);
        let (mut l, mut r) = (x.clone(), x.clone());
        let mut t = MbTelemetry::default();
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            t = m.process(&mut [a, b], None, &s);
        }
        assert!(
            t.band_auto_db[0] > 3.0,
            "band 1 made up: {}",
            t.band_auto_db[0]
        );
        assert_eq!(t.band_auto_db[1], 0.0, "bypassed");
    }

    /// The lowest and highest level of the 1 ms windows of `y` after `from`, against a tone of
    /// amplitude `a`.
    fn window_levels(y: &[f32], from: usize, a: f32) -> (f32, f32) {
        let w = 48;
        let (mut lo, mut hi) = (f32::MAX, 0.0f32);
        for chunk in y[from..].chunks_exact(w) {
            let rms = (chunk.iter().map(|v| v * v).sum::<f32>() / w as f32).sqrt();
            let rel = rms / (a * std::f32::consts::FRAC_1_SQRT_2);
            lo = lo.min(rel);
            hi = hi.max(rel);
        }
        (lo, hi)
    }

    #[test]
    fn lookahead_carries_a_band_made_or_merged() {
        // A tone in the region a crossover splits off and gives back: with lookahead, the band
        // that held it plays out what it delayed (a split), and a merged band's delay is handed
        // to the band that takes its region (no gap for the lookahead's length).
        for look in [5.0, 20.0] {
            for delta in [false, true] {
                let mut s = MbSettings {
                    lookahead_ms: look,
                    ..MbSettings::default()
                };
                if delta {
                    // Every band's Delta with a fixed gain: the tone times (g − 1), through the
                    // bands' dry paths.
                    for b in &mut s.bands {
                        b.delta = true;
                        b.gain_db = 6.0;
                    }
                }
                let mut m = Multiband::new(FS);
                m.reset(&s);
                let a = 0.25f32;
                let x: Vec<f32> = (0..48000)
                    .map(|i| a * (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / FS).sin())
                    .collect();
                let (mut l, mut r) = (x.clone(), x.clone());
                for (k, (bl, br)) in l.chunks_mut(256).zip(r.chunks_mut(256)).enumerate() {
                    // Split at 300 Hz from block 40 to 100, then merged back.
                    s.crossovers[0] = (40..100).contains(&k).then_some(300.0);
                    m.process(&mut [bl, br], None, &s);
                }
                let gain = if delta { db_to_gain(6.0) - 1.0 } else { 1.0 };
                let (lo, hi) = window_levels(&l, 9600, a * gain);
                assert!(
                    lo > 0.94 && hi < 1.06,
                    "look {look} delta {delta}: windows {lo}…{hi}"
                );
            }
        }
    }
}
