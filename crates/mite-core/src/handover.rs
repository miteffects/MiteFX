//! Moving data between a worker thread and the audio thread without locks or audio-thread
//! allocation (BLUEPRINT §5: expensive work runs on a worker and arrives via handover).
//!
//! - [`Handover`]: the worker sends heavy boxed values (FIR kernels, IRs, models); the audio thread
//!   takes the newest and retires the ones it is done with, so it never frees memory itself.
//! - [`triple_buffer`]: the audio thread publishes a small `Copy` snapshot (settings) that the
//!   worker reads whenever it likes; only the latest one matters.

use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};
use std::sync::Arc;

/// Slots for values the audio thread has retired and the worker has not freed yet.
const RETIRE_SLOTS: usize = 8;

/// Worker → audio thread hand-over of boxed values. Share it in an `Arc`.
///
/// The worker calls [`send`](Self::send) and, now and then, [`collect`](Self::collect); the audio
/// thread calls [`take`](Self::take) and [`retire`](Self::retire). A value sent while an older one
/// is still waiting replaces it (the worker frees the older one).
pub struct Handover<T> {
    next: AtomicPtr<T>,
    retired: [AtomicPtr<T>; RETIRE_SLOTS],
}

// SAFETY: values move between threads only as owned boxes through atomic pointer swaps; each
// pointer is owned by exactly one side at a time.
unsafe impl<T: Send> Sync for Handover<T> {}
unsafe impl<T: Send> Send for Handover<T> {}

impl<T> Default for Handover<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Handover<T> {
    pub fn new() -> Self {
        Self { next: AtomicPtr::new(ptr::null_mut()), retired: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())) }
    }

    /// Worker: offers `value`, replacing (and freeing) one the audio thread has not taken yet.
    pub fn send(&self, value: Box<T>) {
        let old = self.next.swap(Box::into_raw(value), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: the pointer came from `Box::into_raw` and nobody else holds it any more.
            drop(unsafe { Box::from_raw(old) });
        }
        self.collect();
    }

    /// Worker: frees values the audio thread has retired.
    pub fn collect(&self) {
        for slot in &self.retired {
            let p = slot.swap(ptr::null_mut(), Ordering::AcqRel);
            if !p.is_null() {
                // SAFETY: as in `send`.
                drop(unsafe { Box::from_raw(p) });
            }
        }
    }

    /// Audio thread: the newest value sent since the last call, if any. Lock- and allocation-free.
    #[inline]
    pub fn take(&self) -> Option<Box<T>> {
        let p = self.next.swap(ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: as in `send`; the swap made this thread the only owner.
        (!p.is_null()).then(|| unsafe { Box::from_raw(p) })
    }

    /// Audio thread: gives a value back for the worker to free. Lock- and allocation-free. If every
    /// slot is still full (the worker has fallen behind), the value comes back: keep it and retry.
    #[inline]
    pub fn retire(&self, value: Box<T>) -> Result<(), Box<T>> {
        let p = Box::into_raw(value);
        for slot in &self.retired {
            if slot.compare_exchange(ptr::null_mut(), p, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return Ok(());
            }
        }
        // SAFETY: the pointer was never published.
        Err(unsafe { Box::from_raw(p) })
    }
}

impl<T> Drop for Handover<T> {
    fn drop(&mut self) {
        drop(self.take());
        self.collect();
    }
}

/// Fresh-data flag in the triple buffer's state byte (the low two bits are the middle slot).
const FRESH: u8 = 4;

struct TripleShared<T> {
    slots: [UnsafeCell<T>; 3],
    /// The middle slot's index, plus [`FRESH`] when the writer has published since the last read.
    state: AtomicU8,
}

// SAFETY: the writer only touches its back slot and the reader only its front slot; the middle
// slot changes hands through the atomic swap of `state`.
unsafe impl<T: Send> Sync for TripleShared<T> {}

/// The publishing half of a [`triple_buffer`]. Lock- and allocation-free.
pub struct TripleWriter<T> {
    shared: Arc<TripleShared<T>>,
    back: u8,
}

/// The reading half of a [`triple_buffer`].
pub struct TripleReader<T> {
    shared: Arc<TripleShared<T>>,
    front: u8,
}

/// A single-writer, single-reader "latest value" buffer for `Copy` snapshots. Allocates here,
/// never in [`TripleWriter::write`] or [`TripleReader::read`].
pub fn triple_buffer<T: Copy + Send>(initial: T) -> (TripleWriter<T>, TripleReader<T>) {
    let shared = Arc::new(TripleShared {
        slots: [UnsafeCell::new(initial), UnsafeCell::new(initial), UnsafeCell::new(initial)],
        state: AtomicU8::new(1),
    });
    (TripleWriter { shared: shared.clone(), back: 0 }, TripleReader { shared, front: 2 })
}

impl<T: Copy> TripleWriter<T> {
    /// Publishes `value`; the reader sees it (or a later one) on its next read.
    #[inline]
    pub fn write(&mut self, value: T) {
        // SAFETY: the back slot belongs to the writer alone.
        unsafe { *self.shared.slots[usize::from(self.back)].get() = value };
        let prev = self.shared.state.swap(self.back | FRESH, Ordering::AcqRel);
        self.back = prev & 3;
    }
}

impl<T: Copy> TripleReader<T> {
    /// The latest published value, if any arrived since the last read.
    pub fn read(&mut self) -> Option<T> {
        if self.shared.state.load(Ordering::Acquire) & FRESH == 0 {
            return None;
        }
        let prev = self.shared.state.swap(self.front, Ordering::AcqRel);
        self.front = prev & 3;
        // SAFETY: the front slot belongs to the reader alone.
        Some(unsafe { *self.shared.slots[usize::from(self.front)].get() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct Counted(#[allow(dead_code)] u32);
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn handover_passes_the_newest_value_and_frees_everything() {
        let before = DROPS.load(Ordering::Relaxed);
        {
            let h = Handover::new();
            h.send(Box::new(Counted(1)));
            h.send(Box::new(Counted(2))); // replaces 1, which the worker frees
            let got = h.take().expect("a value");
            assert_eq!(got.0, 2);
            assert!(h.take().is_none());
            assert!(h.retire(got).is_ok());
            h.collect();
            h.send(Box::new(Counted(3))); // still waiting when the handover drops
        }
        assert_eq!(DROPS.load(Ordering::Relaxed) - before, 3);
    }

    #[test]
    fn retire_gives_the_value_back_when_the_worker_falls_behind() {
        let h = Handover::new();
        for _ in 0..RETIRE_SLOTS {
            assert!(h.retire(Box::new(0u8)).is_ok());
        }
        assert!(h.retire(Box::new(0u8)).is_err());
        h.collect();
        assert!(h.retire(Box::new(0u8)).is_ok());
    }

    #[test]
    fn triple_buffer_reads_only_the_latest_and_only_once() {
        let (mut w, mut r) = triple_buffer(0u32);
        assert_eq!(r.read(), None);
        w.write(1);
        w.write(2);
        assert_eq!(r.read(), Some(2));
        assert_eq!(r.read(), None);
        w.write(3);
        assert_eq!(r.read(), Some(3));
    }

    #[test]
    fn triple_buffer_survives_concurrent_use() {
        let (mut w, mut r) = triple_buffer([0u64; 8]);
        let writer = std::thread::spawn(move || {
            for i in 1..=200_000u64 {
                w.write([i; 8]);
            }
        });
        let mut last = 0;
        while last < 200_000 {
            if let Some(v) = r.read() {
                // Never torn, never older than what was already seen.
                assert!(v.iter().all(|&x| x == v[0]), "torn read {v:?}");
                assert!(v[0] >= last);
                last = v[0];
            }
        }
        writer.join().unwrap();
    }
}
