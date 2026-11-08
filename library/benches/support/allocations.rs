use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
struct CountedAllocator;
fn record(bytes: usize) {
    let _ = COUNTS.try_with(|counts| {
        let (enabled, calls, total) = counts.get();
        if enabled {
            counts.set((true, calls + 1, total + bytes));
        }
    });
}
unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

/// Counts allocation/reallocation requests on the calling thread, excluding setup and reporting.
pub fn measure<T>(label: &str, work: impl FnOnce() -> T) -> T {
    COUNTS.with(|counts| counts.set((true, 0, 0)));
    let output = work();
    let (_, calls, bytes) = COUNTS.with(|counts| {
        let result = counts.get();
        counts.set((false, 0, 0));
        result
    });
    eprintln!("ALLOCATIONS {label}: calls={calls} requested_bytes={bytes}");
    output
}
