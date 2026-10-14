//! Work that must not block the compositor loop (running wpctl, writing the
//! backlight, reading batteries, talking to init, loading icons, encoding
//! screenshots) runs on a thread; its result comes back through a calloop
//! channel and is handled on the main loop.

use std::collections::VecDeque;
use std::path::PathBuf;

use smithay::reexports::calloop::channel::{self, Event, Sender};
use smithay::reexports::calloop::LoopHandle;

use crate::backlight::{self, BrightnessCmd};
use crate::power;
use crate::state::Lumen;
use crate::volume::{self, VolumeCmd};

pub enum Done {
    Volume(Result<volume::Level, volume::Error>),
    Brightness(Result<f64, backlight::Error>),
    Screenshot(Result<PathBuf, String>),
    Battery(Option<power::Battery>),
    Icon { name: String, px: u32, data: Option<Vec<u8>> },
    Init { request: &'static str, result: Result<(), String> },
}

pub struct Background {
    tx: Sender<Done>,
}

impl Background {
    pub fn new(lh: &LoopHandle<'static, Lumen>) -> Background {
        let (tx, rx) = channel::channel::<Done>();
        lh.insert_source(rx, |event, _, state| {
            if let Event::Msg(done) = event {
                state.on_background_done(done);
            }
        })
        .expect("cannot register the background job channel");
        Background { tx }
    }

    /// Run `job` on a new thread and deliver its result to the main loop.
    pub fn run(&self, job: impl FnOnce() -> Done + Send + 'static) {
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new().name("lumen-job".into()).spawn(move || {
            let _ = tx.send(job());
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot start a background job: {e}");
        }
    }
}

/// Commands of one kind (volume, brightness) run one at a time and in
/// order, so each level read back follows its own change. Every key press
/// counts (up to a sanity limit); the compositor's key repeat skips a beat
/// while a command is still running ([`JobQueue::busy`]), so a held key
/// never piles up work.
#[derive(Debug)]
pub struct JobQueue<T> {
    busy: bool,
    waiting: VecDeque<T>,
}

impl<T> Default for JobQueue<T> {
    fn default() -> Self {
        JobQueue { busy: false, waiting: VecDeque::new() }
    }
}

impl<T> JobQueue<T> {
    /// A new command: returned if it should start now.
    pub fn push(&mut self, cmd: T) -> Option<T> {
        if !self.busy {
            self.busy = true;
            return Some(cmd);
        }
        if self.waiting.len() < 32 {
            self.waiting.push_back(cmd);
        }
        None
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// The running command finished: the next one to start, if any.
    pub fn done(&mut self) -> Option<T> {
        let next = self.waiting.pop_front();
        self.busy = next.is_some();
        next
    }
}

/// `/home/me/Pictures/x.png` → `~/Pictures/x.png` (for notifications).
fn display_path(path: &std::path::Path) -> String {
    let p = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && p.starts_with(&format!("{}/", home.trim_end_matches('/'))) => {
            format!("~{}", &p[home.trim_end_matches('/').len()..])
        }
        _ => p,
    }
}

impl Lumen {
    pub fn run_volume(&mut self, cmd: VolumeCmd) {
        if let Some(cmd) = self.volume_jobs.push(cmd) {
            self.bg.run(move || Done::Volume(volume::run(cmd)));
        }
    }

    pub fn run_brightness(&mut self, cmd: BrightnessCmd) {
        if let Some(cmd) = self.brightness_jobs.push(cmd) {
            let sysfs = backlight::sysfs_root();
            self.bg.run(move || Done::Brightness(backlight::run(&sysfs, cmd)));
        }
    }

    /// Send a request (suspend, reboot, poweroff) to lumen-init.
    pub fn init_request(&mut self, request: &'static str) {
        tracing::info!("{request} requested");
        let socket = power::init_socket();
        self.bg.run(move || Done::Init { request, result: power::init_request(&socket, request) });
    }

    /// Start the themed icon lookups the last frame asked for.
    pub fn load_requested_icons(&mut self) {
        for (name, px) in self.render.icons.take_requests() {
            self.bg.run(move || {
                let data = crate::icontheme::load(&name, px);
                Done::Icon { name, px, data }
            });
        }
    }

    pub fn on_background_done(&mut self, done: Done) {
        match done {
            Done::Volume(result) => {
                match result {
                    Ok(level) => self.shell_show_osd(crate::shell::osd::OsdKind::Volume, level.volume, level.muted),
                    Err(volume::Error::Missing) => {
                        // Without PipeWire there is nothing to show; say so once.
                        if !std::mem::replace(&mut self.wpctl_missing, true) {
                            tracing::warn!("volume keys: wpctl (PipeWire) is not installed");
                        }
                    }
                    Err(volume::Error::Failed(e)) => tracing::warn!("volume: {e}"),
                }
                if let Some(next) = self.volume_jobs.done() {
                    self.run_volume_now(next);
                }
            }
            Done::Brightness(result) => {
                match result {
                    Ok(level) => self.shell_show_osd(crate::shell::osd::OsdKind::Brightness, level, false),
                    // A desktop: the monitor has its own buttons.
                    Err(backlight::Error::NoDevice) => {
                        if !std::mem::replace(&mut self.backlight_reported, true) {
                            tracing::info!("brightness keys: no backlight device");
                        }
                    }
                    Err(backlight::Error::Permission(e)) => {
                        tracing::warn!("brightness: {e}");
                        if !std::mem::replace(&mut self.backlight_reported, true) {
                            self.notify("Cannot change the brightness", "The backlight is not writable for the video group.");
                        }
                    }
                    Err(backlight::Error::Failed(e)) => tracing::warn!("brightness: {e}"),
                }
                if let Some(next) = self.brightness_jobs.done() {
                    let sysfs = backlight::sysfs_root();
                    self.bg.run(move || Done::Brightness(backlight::run(&sysfs, next)));
                }
            }
            Done::Screenshot(Ok(path)) => {
                tracing::info!("screenshot saved to {}", path.display());
                self.notify("Screenshot saved", &display_path(&path));
            }
            Done::Screenshot(Err(e)) => {
                tracing::warn!("screenshot failed: {e}");
                self.notify("Screenshot failed", &e);
            }
            Done::Battery(b) => self.on_battery(b),
            Done::Icon { name, px, data } => self.render.icons.arrived(name, px, data),
            Done::Init { request, result: Err(e) } => {
                tracing::warn!("{request}: {e}");
                let what = match request {
                    "suspend" => "Cannot suspend",
                    "reboot" => "Cannot restart",
                    _ => "Cannot power off",
                };
                self.notify(what, &e);
            }
            Done::Init { result: Ok(()), .. } => {}
        }
        self.queue_redraw_all();
    }

    fn run_volume_now(&mut self, cmd: VolumeCmd) {
        self.bg.run(move || Done::Volume(volume::run(cmd)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_queue_runs_one_at_a_time_in_order() {
        let mut q = JobQueue::default();
        assert!(!q.busy());
        assert_eq!(q.push(1), Some(1));
        assert!(q.busy());
        assert_eq!(q.push(2), None);
        assert_eq!(q.push(3), None);
        assert_eq!(q.done(), Some(2));
        assert_eq!(q.push(4), None);
        assert_eq!(q.done(), Some(3));
        assert_eq!(q.done(), Some(4));
        assert!(q.busy(), "4 is still running");
        assert_eq!(q.done(), None);
        assert!(!q.busy());
        assert_eq!(q.push(5), Some(5), "idle again");
        for i in 0..100 {
            q.push(i);
        }
        assert_eq!(std::iter::from_fn(|| q.done()).count(), 32, "a flood is capped");
    }
}
