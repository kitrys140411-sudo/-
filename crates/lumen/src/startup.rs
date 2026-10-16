//! Startup timeline: wall-clock and CPU time of each step up to the first
//! frame, logged as one line when the desktop is on screen ("startup: ...").
//!
//! On software rendering under emulation (QEMU TCG, llvmpipe) every step costs
//! seconds, so the line in /run/log/lumen.log is what tells where the boot
//! time goes. Steps are marked where they end; `wall` is since `main`, `cpu`
//! is the CPU time of the whole process (all threads, llvmpipe's included)
//! spent in that step.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Timeline {
    start: Instant,
    last_cpu: Duration,
    steps: Vec<(&'static str, Duration, Duration)>,
    done: bool,
}

static TIMELINE: Mutex<Option<Timeline>> = Mutex::new(None);
/// Recording (until the first frame): marks in per-frame code cost one load after that.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn process_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime only writes the timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Start the timeline (first thing in `main`).
pub fn begin() {
    let mut t = TIMELINE.lock().unwrap_or_else(|e| e.into_inner());
    *t = Some(Timeline { start: Instant::now(), last_cpu: process_cpu(), steps: Vec::new(), done: false });
    ACTIVE.store(true, Ordering::Relaxed);
}

/// Whether the first frame is still being waited for.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// `step` just finished.
pub fn mark(step: &'static str) {
    if !active() {
        return;
    }
    let mut t = TIMELINE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(t) = t.as_mut().filter(|t| !t.done) else { return };
    let cpu = process_cpu();
    let wall = t.start.elapsed();
    tracing::debug!("startup: {step} done at {} ms ({} ms CPU)", wall.as_millis(), cpu.saturating_sub(t.last_cpu).as_millis());
    t.steps.push((step, wall, cpu.saturating_sub(t.last_cpu)));
    t.last_cpu = cpu;
}

/// The first frame is on screen: log the timeline once and stop recording.
pub fn finish() {
    mark("first frame");
    let mut t = TIMELINE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(t) = t.as_mut().filter(|t| !t.done) else { return };
    t.done = true;
    ACTIVE.store(false, Ordering::Relaxed);
    tracing::info!("startup: {}", summary(&t.steps));
}

/// The recorded steps: (name, ms since start when it ended, CPU ms it took).
pub fn steps() -> Vec<(&'static str, u128, u128)> {
    let t = TIMELINE.lock().unwrap_or_else(|e| e.into_inner());
    t.as_ref().map(|t| t.steps.iter().map(|(n, w, c)| (*n, w.as_millis(), c.as_millis())).collect()).unwrap_or_default()
}

/// "step +wall ms (cpu ms CPU), ...; total wall ms, cpu ms CPU".
fn summary(steps: &[(&'static str, Duration, Duration)]) -> String {
    let mut prev = Duration::ZERO;
    let parts: Vec<String> = steps
        .iter()
        .map(|(name, wall, cpu)| {
            let s = format!("{name} +{} ms ({} ms CPU)", wall.saturating_sub(prev).as_millis(), cpu.as_millis());
            prev = *wall;
            s
        })
        .collect();
    let cpu: Duration = steps.iter().map(|s| s.2).sum();
    format!("{}; total {} ms, {} ms CPU", parts.join(", "), prev.as_millis(), cpu.as_millis())
}

#[cfg(test)]
mod tests {
    use super::summary;
    use std::time::Duration;

    #[test]
    fn summary_lists_step_durations() {
        let ms = Duration::from_millis;
        let s = summary(&[("egl", ms(100), ms(90)), ("shaders", ms(250), ms(400))]);
        assert_eq!(s, "egl +100 ms (90 ms CPU), shaders +150 ms (400 ms CPU); total 250 ms, 490 ms CPU");
    }
}
