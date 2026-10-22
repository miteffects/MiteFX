//! Lock-free, allocation-free values written by the audio thread and read by the UI.

use std::sync::atomic::{AtomicU32, Ordering};

/// An `f32` stored in an `AtomicU32`. Relaxed ordering: telemetry is a stream of independent
/// samples, never a synchronisation point.
#[derive(Debug)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub const fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    #[inline]
    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Output peak levels in dBFS, one per channel. Mono plugins write the same value to both.
#[derive(Debug)]
pub struct StereoMeter {
    pub left_db: AtomicF32,
    pub right_db: AtomicF32,
}

impl StereoMeter {
    pub const fn new(floor_db: f32) -> Self {
        Self { left_db: AtomicF32::new(floor_db), right_db: AtomicF32::new(floor_db) }
    }

    #[inline]
    pub fn store(&self, left_db: f32, right_db: f32) {
        self.left_db.store(left_db);
        self.right_db.store(right_db);
    }

    #[inline]
    pub fn load(&self) -> (f32, f32) {
        (self.left_db.load(), self.right_db.load())
    }
}

/// Lock-free ring of the most recent audio samples (mono), written by the audio thread and read
/// by the analyzer on the UI thread. A torn read across the write position only smears one
/// analysis frame, which the analyzer's ballistics hide.
pub struct SpectrumTap {
    ring: Box<[AtomicF32]>,
    write: std::sync::atomic::AtomicUsize,
}

impl SpectrumTap {
    /// `capacity` is rounded up to a power of two. Allocates; call off the audio thread.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        Self { ring: (0..cap).map(|_| AtomicF32::new(0.0)).collect(), write: std::sync::atomic::AtomicUsize::new(0) }
    }

    pub fn capacity(&self) -> usize {
        self.ring.len()
    }

    /// Audio thread: appends samples. Allocation- and lock-free.
    #[inline]
    pub fn push(&self, samples: impl Iterator<Item = f32>) {
        let mask = self.ring.len() - 1;
        let mut w = self.write.load(Ordering::Relaxed);
        for s in samples {
            self.ring[w & mask].store(s);
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// UI thread: copies the newest `out.len()` samples, oldest first.
    pub fn read_latest(&self, out: &mut [f32]) {
        let mask = self.ring.len() - 1;
        let end = self.write.load(Ordering::Acquire);
        let n = out.len().min(self.ring.len());
        let start = end.wrapping_sub(n);
        for (i, o) in out.iter_mut().take(n).enumerate() {
            *o = self.ring[start.wrapping_add(i) & mask].load();
        }
    }
}

/// Lock-free ring of fixed-width records (one per time slice: levels, gain reduction…), written
/// by the audio thread and read by the editor's history. The UI asks for what arrived since it
/// last looked; if it fell more than the ring behind, it gets the newest ring's worth.
pub struct HistoryTap<const W: usize> {
    ring: Box<[[AtomicF32; W]]>,
    write: std::sync::atomic::AtomicUsize,
}

impl<const W: usize> HistoryTap<W> {
    /// `capacity` records, rounded up to a power of two. Allocates; call off the audio thread.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        Self {
            ring: (0..cap).map(|_| std::array::from_fn(|_| AtomicF32::new(0.0))).collect(),
            write: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.ring.len()
    }

    /// Audio thread: appends one record. Allocation- and lock-free.
    #[inline]
    pub fn push(&self, record: &[f32; W]) {
        let w = self.write.load(Ordering::Relaxed);
        let slot = &self.ring[w & (self.ring.len() - 1)];
        for (a, &v) in slot.iter().zip(record) {
            a.store(v);
        }
        self.write.store(w.wrapping_add(1), Ordering::Release);
    }

    /// Records pushed so far (wrapping).
    pub fn count(&self) -> usize {
        self.write.load(Ordering::Acquire)
    }

    /// UI thread: calls `f` with every record pushed since `*seen` (oldest first, at most the
    /// ring's capacity) and moves `*seen` to now.
    pub fn read_since(&self, seen: &mut usize, mut f: impl FnMut(&[f32; W])) {
        let end = self.count();
        let n = end.wrapping_sub(*seen).min(self.ring.len());
        let mask = self.ring.len() - 1;
        let mut record = [0.0; W];
        for i in end.wrapping_sub(n)..end {
            for (v, a) in record.iter_mut().zip(&self.ring[i & mask]) {
                *v = a.load();
            }
            f(&record);
        }
        *seen = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_returns_newest_samples_in_order() {
        let tap = SpectrumTap::new(8);
        tap.push((0..13).map(|i| i as f32));
        let mut out = [0.0; 4];
        tap.read_latest(&mut out);
        assert_eq!(out, [9.0, 10.0, 11.0, 12.0]);
    }

    #[test]
    fn history_returns_what_arrived_since_last_time() {
        let tap: HistoryTap<2> = HistoryTap::new(4);
        let mut seen = 0;
        for i in 0..3 {
            tap.push(&[i as f32, -(i as f32)]);
        }
        let mut got = Vec::new();
        tap.read_since(&mut seen, |r| got.push(r[0]));
        assert_eq!(got, [0.0, 1.0, 2.0]);
        // Fallen behind by more than the ring: the newest four.
        for i in 3..10 {
            tap.push(&[i as f32, 0.0]);
        }
        got.clear();
        tap.read_since(&mut seen, |r| got.push(r[0]));
        assert_eq!(got, [6.0, 7.0, 8.0, 9.0]);
        tap.read_since(&mut seen, |_| panic!("nothing new"));
    }

    #[test]
    fn stores_exact_bits() {
        let a = AtomicF32::new(0.0);
        for v in [-150.0, -0.0, 1.5e-30, f32::MAX] {
            a.store(v);
            assert_eq!(a.load().to_bits(), v.to_bits());
        }
    }
}
