//! Real-time thread setup: scheduling class and denormal handling.

/// Best-effort promotion of the calling thread to a real-time scheduling class. Returns
/// whether it took effect.
///
/// * Linux: `SCHED_FIFO` at a low real-time priority. Needs `CAP_SYS_NICE` or an
///   `RLIMIT_RTPRIO` allowance; without it the thread keeps its normal priority.
/// * macOS: a time-constraint policy sized for `period_ns` (the nominal block duration),
///   which is what CoreAudio gives its own I/O threads. **TYPE-CHECKED ONLY.** Joining the
///   host's audio workgroup (`os_workgroup_join`) would be better still and is an open
///   point: the workgroup has to be sent across the process boundary as a Mach port.
pub fn promote_current_thread_to_realtime(period_ns: u64) -> bool {
    imp::promote(period_ns)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    pub fn promote(_period_ns: u64) -> bool {
        let param = libc::sched_param { sched_priority: 10 };
        // SAFETY: valid parameter block; 0 addresses the calling thread.
        unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK, &param) == 0 }
    }
}

// ---- macOS: TYPE-CHECKED ONLY ----
#[cfg(target_os = "macos")]
mod imp {
    /// `struct mach_timebase_info` from <mach/mach_time.h> (libc's own binding is
    /// deprecated in favour of another crate; two integers do not justify a dependency).
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
    }

    pub fn promote(period_ns: u64) -> bool {
        let mut timebase = Timebase { numer: 0, denom: 0 };
        // SAFETY: valid out-parameter.
        unsafe { mach_timebase_info(&mut timebase) };
        if timebase.numer == 0 || timebase.denom == 0 {
            return false;
        }
        let to_ticks = |ns: u64| (ns as u128 * timebase.denom as u128 / timebase.numer as u128) as u32;
        let period = period_ns.clamp(500_000, 50_000_000);
        let mut policy = libc::thread_time_constraint_policy {
            period: to_ticks(period),
            computation: to_ticks(period / 2),
            constraint: to_ticks(period),
            preemptible: 1,
        };
        // SAFETY: the policy struct matches THREAD_TIME_CONSTRAINT_POLICY and its count;
        // pthread_mach_thread_np does not add a port reference that would need releasing.
        unsafe {
            let thread = libc::pthread_mach_thread_np(libc::pthread_self());
            libc::thread_policy_set(
                thread,
                libc::THREAD_TIME_CONSTRAINT_POLICY as _,
                (&mut policy as *mut libc::thread_time_constraint_policy).cast(),
                libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
            ) == 0
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
mod imp {
    pub fn promote(_period_ns: u64) -> bool {
        false
    }
}

/// Makes the calling thread treat denormal floats as zero (x86-64: FTZ and DAZ in MXCSR;
/// AArch64: FZ in FPCR), as audio hosts do on their render threads. The host's own setting
/// does not cross the process boundary, so the runtime applies it to every real-time
/// thread. The AArch64 variant is TYPE-CHECKED ONLY.
pub fn enable_denormal_flushing() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: reads and writes the calling thread's MXCSR through a stack slot.
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{0}]", in(reg) &mut csr, options(nostack));
        csr |= 0x8040; // FTZ | DAZ
        std::arch::asm!("ldmxcsr [{0}]", in(reg) &csr, options(nostack));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: reads and writes the calling thread's FPCR.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {0}, fpcr", out(reg) fpcr, options(nomem, nostack));
        fpcr |= 1 << 24; // FZ
        std::arch::asm!("msr fpcr, {0}", in(reg) fpcr, options(nomem, nostack));
    }
}
