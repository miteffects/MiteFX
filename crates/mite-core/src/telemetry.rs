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
    fn stores_exact_bits() {
        let a = AtomicF32::new(0.0);
        for v in [-150.0, -0.0, 1.5e-30, f32::MAX] {
            a.store(v);
            assert_eq!(a.load().to_bits(), v.to_bits());
        }
    }
}
