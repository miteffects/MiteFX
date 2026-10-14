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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_exact_bits() {
        let a = AtomicF32::new(0.0);
        for v in [-150.0, -0.0, 1.5e-30, f32::MAX] {
            a.store(v);
            assert_eq!(a.load().to_bits(), v.to_bits());
        }
    }
}
