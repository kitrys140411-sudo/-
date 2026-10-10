//! Monotonic time from the TSC, calibrated against the firmware's Stall().
//! Animations are driven by elapsed time, never by frame counts.

use core::time::Duration;
use uefi::boot;

pub struct Clock {
    start: u64,
    ticks_per_ms: u64,
}

fn rdtsc() -> u64 {
    // SAFETY: RDTSC has no side effects and is available on every x86_64 CPU.
    unsafe { core::arch::x86_64::_rdtsc() }
}

impl Clock {
    pub fn calibrate() -> Self {
        let t0 = rdtsc();
        boot::stall(Duration::from_millis(10));
        let t1 = rdtsc();
        Clock {
            start: t0,
            ticks_per_ms: ((t1 - t0) / 10).max(1),
        }
    }

    /// Microseconds since calibration.
    pub fn now_us(&self) -> u64 {
        (rdtsc() - self.start) * 1000 / self.ticks_per_ms
    }
}
