//! The audio-thread paths must not allocate or free (BLUEPRINT §5). A counting global allocator
//! watches them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mite_dsp::conv::{Convolver, KernelLayout, KernelSet};

struct Counting;

static WATCHING: AtomicBool = AtomicBool::new(false);
static EVENTS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if WATCHING.load(Ordering::Relaxed) {
            EVENTS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if WATCHING.load(Ordering::Relaxed) {
            EVENTS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and returns how many allocations or frees it made.
fn watched(f: impl FnOnce()) -> usize {
    EVENTS.store(0, Ordering::Relaxed);
    WATCHING.store(true, Ordering::Relaxed);
    f();
    WATCHING.store(false, Ordering::Relaxed);
    EVENTS.load(Ordering::Relaxed)
}

#[test]
fn convolver_processes_and_swaps_kernels_without_allocating() {
    let block = 128;
    let kernel = |g: f64| {
        Box::new(KernelSet::new(
            block,
            KernelLayout::Matrix,
            &[
                vec![g; 1000],
                vec![0.1; 1000],
                vec![0.1; 1000],
                vec![g; 1000],
            ],
            500,
        ))
    };
    let mut conv = Convolver::new(block, 4096, 512);
    conv.replace(kernel(1.0));
    let (first, second) = (kernel(0.5), kernel(0.25));
    let (mut l, mut r) = (vec![0.1f32; 4096], vec![-0.1f32; 4096]);
    let mut retired = Vec::with_capacity(4);
    let events = watched(|| {
        for (a, b) in l.chunks_mut(100).zip(r.chunks_mut(100)) {
            conv.process(a, b);
        }
        assert!(conv.offer(first).is_ok());
        for (a, b) in l.chunks_mut(100).zip(r.chunks_mut(100)) {
            conv.process(a, b);
            if let Some(old) = conv.retired() {
                retired.push(old);
            }
        }
        assert!(conv.offer(second).is_ok());
        conv.reset();
    });
    assert_eq!(events, 0, "the audio path allocated or freed");
    assert_eq!(retired.len(), 1);
}
