//! The Rust stand-in for the C++ test programs' replaced `operator new` / `operator delete`.
//!
//! Every test binary that includes this module gets a `#[global_allocator]` that
//!
//! * counts the allocations (and, separately, the deallocations) made by the current thread while
//!   its "render in progress" flag is set (`realtime=true; ...; realtime=false;` in the C++), and
//! * fails exactly one allocation on request (`failAllocation=true;` in the C++), by returning
//!   null from the allocator, which is what a Rust `try_reserve` reports as an error.
//!
//! The C++ programs used a `thread_local` flag and one process-wide counter, and ran their groups
//! one after the other. Here the flag, the counters and the failure request are all per thread,
//! because the test harness runs the groups (one `#[test]` each) on parallel threads; every group
//! asserts its own counters.

use std::cell::Cell;

thread_local! {
    static REALTIME: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static FAIL_NEXT: Cell<bool> = const { Cell::new(false) };
}

// With the crate's `cxx-alloc` feature the library installs its own global allocator (C++
// operator new); a second one cannot be declared, and the C++ programs do the counting then.
#[cfg(not(feature = "cxx-alloc"))]
mod allocator {
    use super::{ALLOCATIONS, DEALLOCATIONS, FAIL_NEXT, REALTIME};
    use std::alloc::{GlobalAlloc, Layout, System};

    pub struct TestAllocator;

    /// Counts one allocation when the thread is "rendering"; true when it must fail.
    fn note_allocation() -> bool {
        let _ = REALTIME.try_with(|realtime| {
            if realtime.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        FAIL_NEXT.try_with(|fail| fail.replace(false)).unwrap_or(false)
    }

    // SAFETY: defers to the system allocator; the bookkeeping uses const-initialised thread
    // locals without destructors, so it never allocates or re-enters the allocator.
    unsafe impl GlobalAlloc for TestAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if note_allocation() {
                return std::ptr::null_mut();
            }
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if note_allocation() {
                return std::ptr::null_mut();
            }
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if note_allocation() {
                return std::ptr::null_mut();
            }
            unsafe { System.realloc(pointer, layout, size) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            let _ = REALTIME.try_with(|realtime| {
                if realtime.get() {
                    let _ = DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: TestAllocator = TestAllocator;
}

/// Restores the previous flag when a `realtime` section ends, also by a failed assertion.
struct Section(bool);

impl Drop for Section {
    fn drop(&mut self) {
        REALTIME.with(|realtime| realtime.set(self.0));
    }
}

/// Runs `f` as the render thread: `realtime=true; f(); realtime=false;`. Allocations and
/// deallocations made by this thread inside `f` are counted.
pub fn realtime<T>(f: impl FnOnce() -> T) -> T {
    let _section = Section(REALTIME.with(|realtime| realtime.replace(true)));
    f()
}

/// Allocations this thread made inside `realtime` sections since the last reset.
pub fn render_allocations() -> usize {
    ALLOCATIONS.with(|count| count.get())
}

/// Deallocations this thread made inside `realtime` sections since the last reset.
pub fn render_deallocations() -> usize {
    DEALLOCATIONS.with(|count| count.get())
}

/// `allocations=0;`
pub fn reset_render_allocations() {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
}

/// `failAllocation=true;`: the next allocation this thread asks for fails once (the allocator
/// returns null). Call it immediately before the engine call under test, with nothing that
/// allocates in between.
pub fn fail_next_allocation() {
    FAIL_NEXT.with(|fail| fail.set(true));
}

/// True while a requested allocation failure has not been consumed yet.
pub fn allocation_failure_pending() -> bool {
    FAIL_NEXT.with(|fail| fail.get())
}

/// Withdraws an unconsumed `fail_next_allocation`.
pub fn cancel_allocation_failure() {
    FAIL_NEXT.with(|fail| fail.set(false));
}
