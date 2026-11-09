//! The song's bars and beats: a tempo map (song seconds ↔ quarter notes) and bar signatures, from
//! the host's ARA musical context or, without ARA, from its transport (one tempo, one signature).
//! The editor draws its ruler from it and snaps timing to it.

/// Tempo map and bar signatures.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    /// (song s, quarter) points, in order; between and beyond them the tempo is constant.
    tempo: Vec<(f64, f64)>,
    /// (numerator, denominator, quarter where it starts), in order.
    bars: Vec<(i32, i32, f64)>,
}

/// A line of the ruler: where (song s), and what it marks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub s: f64,
    /// The bar (1 = the first), and the beat in it (1 = the downbeat), or 0 for a line between
    /// beats.
    pub bar: i64,
    pub beat: i64,
}

impl Grid {
    /// From tempo points (s, quarter) and signatures (numerator, denominator, quarter). `None`
    /// when they cannot make a grid (no tempo, or quarters not rising).
    pub fn new(mut tempo: Vec<(f64, f64)>, mut bars: Vec<(i32, i32, f64)>) -> Option<Self> {
        tempo.retain(|(s, q)| s.is_finite() && q.is_finite());
        tempo.sort_by(|a, b| a.0.total_cmp(&b.0));
        tempo.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
        if tempo.len() == 1 {
            // One point: 120 bpm until told otherwise.
            let (s, q) = tempo[0];
            tempo.push((s + 0.5, q + 1.0));
        }
        if tempo.len() < 2 || tempo.windows(2).any(|w| w[1].1 <= w[0].1) {
            return None;
        }
        bars.retain(|b| b.0 > 0 && b.1 > 0 && b.2.is_finite());
        bars.sort_by(|a, b| a.2.total_cmp(&b.2));
        if bars.is_empty() {
            bars.push((4, 4, 0.0));
        }
        Some(Self { tempo, bars })
    }

    /// One tempo (bpm) and signature, with quarter `q_at` at song second `s_at` and a bar
    /// starting at quarter `bar_q` (a host's transport).
    pub fn constant(bpm: f64, (num, den): (i32, i32), s_at: f64, q_at: f64, bar_q: f64) -> Option<Self> {
        if !(bpm.is_finite() && bpm > 1.0) {
            return None;
        }
        let spq = 60.0 / bpm;
        Self::new(vec![(s_at, q_at), (s_at + spq, q_at + 1.0)], vec![(num, den, bar_q)])
    }

    /// The segment of the tempo map around song second `s` (or quarter `q`).
    fn segment_s(&self, s: f64) -> ((f64, f64), (f64, f64)) {
        let k = self.tempo.partition_point(|p| p.0 <= s).clamp(1, self.tempo.len() - 1);
        (self.tempo[k - 1], self.tempo[k])
    }

    fn segment_q(&self, q: f64) -> ((f64, f64), (f64, f64)) {
        let k = self.tempo.partition_point(|p| p.1 <= q).clamp(1, self.tempo.len() - 1);
        (self.tempo[k - 1], self.tempo[k])
    }

    /// The quarter at song second `s`.
    pub fn quarter_at(&self, s: f64) -> f64 {
        let (a, b) = self.segment_s(s);
        a.1 + (s - a.0) * (b.1 - a.1) / (b.0 - a.0)
    }

    /// The song second of quarter `q`.
    pub fn seconds_at(&self, q: f64) -> f64 {
        let (a, b) = self.segment_q(q);
        a.0 + (q - a.1) * (b.0 - a.0) / (b.1 - a.1)
    }

    /// The signature in force at quarter `q`, and the quarter it starts at (or the first one, before it).
    fn signature_at(&self, q: f64) -> (i32, i32, f64) {
        let k = self.bars.partition_point(|b| b.2 <= q);
        self.bars[k.saturating_sub(1)]
    }

    /// The bars' starts (quarter, bar number from 1, beats per bar, quarters per beat), for bars
    /// that touch quarters `q0..q1` (at most `limit`).
    fn bars_between(&self, q0: f64, q1: f64, limit: usize) -> Vec<(f64, i64, i32, f64)> {
        let mut out = Vec::new();
        // Bar 1 starts at the first signature; bars before it count down from 0.
        let mut number: i64 = 1;
        for (k, &(num, den, start)) in self.bars.iter().enumerate() {
            let len = f64::from(num) * 4.0 / f64::from(den);
            let per_beat = 4.0 / f64::from(den);
            let end = self.bars.get(k + 1).map_or(f64::INFINITY, |b| b.2);
            let count = if end.is_finite() { ((end - start) / len).ceil() as i64 } else { i64::MAX };
            let from = ((q0 - start) / len).floor() as i64;
            let mut j = if k == 0 { from } else { from.max(0) };
            while j < count {
                let q = start + j as f64 * len;
                if q > q1 || out.len() >= limit {
                    return out;
                }
                if q + len > q0 {
                    out.push((q, number + j, num, per_beat));
                }
                j += 1;
            }
            number = number.saturating_add(count);
        }
        out
    }

    /// The ruler's lines between song seconds `s0` and `s1`, no closer than `min_gap` s: bars
    /// (every 1, 2, 4… bars when crowded), their beats, and halves, quarters… of a beat.
    pub fn ticks(&self, s0: f64, s1: f64, min_gap: f64) -> Vec<Tick> {
        let (q0, q1) = (self.quarter_at(s0), self.quarter_at(s1));
        let bars = self.bars_between(q0, q1, 4096);
        let mut out = Vec::new();
        let Some(&(_, _, num, per_beat)) = bars.first() else { return out };
        let beat_s = self.seconds_at(q0 + per_beat) - self.seconds_at(q0);
        let bar_s = beat_s * f64::from(num);
        if beat_s >= min_gap {
            // Beats, and divisions of them when there is room (halves to sixteenths of a beat).
            let div = [8, 4, 2, 1].into_iter().find(|&d| beat_s / f64::from(d) >= min_gap).unwrap_or(1);
            for &(q, bar, num, per_beat) in &bars {
                for beat in 0..num {
                    for d in 0..div {
                        let s = self.seconds_at(q + (f64::from(beat) + f64::from(d) / f64::from(div)) * per_beat);
                        if s >= s0 && s <= s1 {
                            out.push(Tick { s, bar, beat: if d == 0 { i64::from(beat) + 1 } else { 0 } });
                        }
                    }
                }
            }
        } else {
            // Bars only, every 1, 2, 4… of them.
            let every = (0..12).map(|k| 1i64 << k).find(|&e| bar_s * e as f64 >= min_gap).unwrap_or(4096);
            for &(q, bar, _, _) in &bars {
                if (bar - 1).rem_euclid(every) == 0 {
                    let s = self.seconds_at(q);
                    if s >= s0 && s <= s1 {
                        out.push(Tick { s, bar, beat: 1 });
                    }
                }
            }
        }
        out
    }

    /// Song second `s` onto the nearest line of a grid no finer than `min_gap` s (beats and their
    /// halves, quarters…; bars when beats are closer than that).
    pub fn snap(&self, s: f64, min_gap: f64) -> f64 {
        let ticks = self.ticks(s - 64.0 * min_gap.max(0.01), s + 64.0 * min_gap.max(0.01), min_gap);
        ticks.iter().map(|t| t.s).min_by(|a, b| (a - s).abs().total_cmp(&(b - s).abs())).unwrap_or(s)
    }

    /// The bar and beat (1-based, with the fraction of the beat) at song second `s`.
    pub fn bar_beat(&self, s: f64) -> (i64, f64) {
        let q = self.quarter_at(s);
        let bars = self.bars_between(q, q, 1);
        match bars.first() {
            Some(&(start, bar, _, per_beat)) => (bar, 1.0 + (q - start) / per_beat),
            None => {
                let (num, den, start) = self.signature_at(q);
                let len = f64::from(num) * 4.0 / f64::from(den);
                let k = ((q - start) / len).floor();
                (k as i64 + 1, 1.0 + (q - start - k * len) / (4.0 / f64::from(den)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_constant_tempo_counts_bars_and_beats() {
        // 120 bpm, 4/4 from 0: a beat is 0.5 s, a bar 2 s.
        let g = Grid::constant(120.0, (4, 4), 0.0, 0.0, 0.0).expect("grid");
        assert!((g.quarter_at(3.0) - 6.0).abs() < 1e-12);
        assert!((g.seconds_at(6.0) - 3.0).abs() < 1e-12);
        assert_eq!(g.bar_beat(2.0), (2, 1.0));
        assert_eq!(g.bar_beat(3.25), (2, 3.5));
        let t = g.ticks(0.0, 4.0, 0.4);
        assert_eq!(t.iter().filter(|t| t.beat == 1).map(|t| t.bar).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(t.len(), 9, "{t:?}");
        // Crowded: bars only, every second one.
        let t = g.ticks(0.0, 20.0, 3.0);
        assert_eq!(t.iter().map(|t| t.bar).collect::<Vec<_>>(), vec![1, 3, 5, 7, 9, 11]);
        // Snap to the nearest eighth when they are 0.25 s apart.
        assert!((g.snap(1.13, 0.2) - 1.25).abs() < 1e-9, "{}", g.snap(1.13, 0.2));
        assert!((g.snap(1.13, 0.4) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn tempo_changes_and_signatures_follow_the_map() {
        // 120 bpm for 4 quarters (2 s), then 60 bpm; 4/4, then 3/4 from quarter 8.
        let g = Grid::new(vec![(0.0, 0.0), (2.0, 4.0), (3.0, 5.0)], vec![(4, 4, 0.0), (3, 4, 8.0)]).expect("grid");
        assert!((g.seconds_at(8.0) - 6.0).abs() < 1e-12);
        assert!((g.quarter_at(7.0) - 9.0).abs() < 1e-12);
        // Bar 3 starts at quarter 8 (6 s) in 3/4; bar 4 at quarter 11 (9 s).
        assert_eq!(g.bar_beat(6.0), (3, 1.0));
        assert_eq!(g.bar_beat(9.0), (4, 1.0));
        let downbeats: Vec<i64> = g.ticks(0.0, 10.0, 0.4).iter().filter(|t| t.beat == 1).map(|t| t.bar).collect();
        assert_eq!(downbeats, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_grid_needs_rising_quarters() {
        assert!(Grid::new(vec![], vec![]).is_none());
        assert!(Grid::new(vec![(0.0, 4.0), (1.0, 2.0)], vec![]).is_none());
        assert!(Grid::constant(0.0, (4, 4), 0.0, 0.0, 0.0).is_none());
    }
}
