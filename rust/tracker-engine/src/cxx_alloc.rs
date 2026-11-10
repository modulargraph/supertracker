//! Test builds only (feature `cxx-alloc`): route Rust heap allocations through C++
//! `operator new` / `operator delete`.
//!
//! The C++ test programs replace `operator new` to count allocations made while a render is in
//! progress ("zero render-thread allocations"). With this allocator those counters also see the
//! engine's Rust allocations, so the same assertions hold without changing the tests.

use std::alloc::{GlobalAlloc, Layout};
use std::ffi::c_void;

extern "C" {
    /// `operator new(std::size_t, const std::nothrow_t&)`: forwards to the (replaced) throwing
    /// `operator new` and returns null instead of letting `std::bad_alloc` unwind into Rust.
    #[link_name = "_ZnwmRKSt9nothrow_t"]
    fn cxx_new_nothrow(size: usize, tag: *const c_void) -> *mut c_void;
    /// `std::nothrow`
    #[link_name = "_ZSt7nothrow"]
    static CXX_NOTHROW: u8;
    /// `operator delete(void*)`
    #[link_name = "_ZdlPv"]
    fn cxx_delete(pointer: *mut c_void);
    fn posix_memalign(out: *mut *mut c_void, alignment: usize, size: usize) -> i32;
    fn free(pointer: *mut c_void);
}

/// Largest alignment `operator new` guarantees on the supported targets.
const NEW_ALIGNMENT: usize = 16;

/// `operator new(size, std::nothrow)`: null when the allocation fails.
unsafe fn cxx_new(size: usize) -> *mut c_void {
    cxx_new_nothrow(size, std::ptr::addr_of!(CXX_NOTHROW) as *const c_void)
}

struct CxxAllocator;

unsafe impl GlobalAlloc for CxxAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= NEW_ALIGNMENT {
            cxx_new(layout.size().max(1)) as *mut u8
        } else {
            // Over-aligned blocks cannot come from operator new: count them through a one-byte
            // operator new/delete pair, then take the memory from posix_memalign.
            let probe = cxx_new(1);
            if probe.is_null() {
                return std::ptr::null_mut();
            }
            cxx_delete(probe);
            let mut out: *mut c_void = std::ptr::null_mut();
            if posix_memalign(&mut out, layout.align(), layout.size().max(1)) != 0 {
                return std::ptr::null_mut();
            }
            out as *mut u8
        }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if layout.align() <= NEW_ALIGNMENT {
            cxx_delete(pointer as *mut c_void)
        } else {
            free(pointer as *mut c_void)
        }
    }
}

#[global_allocator]
static ALLOCATOR: CxxAllocator = CxxAllocator;
