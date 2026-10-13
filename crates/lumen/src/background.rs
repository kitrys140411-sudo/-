//! Work that must not block the compositor loop (running wpctl, encoding
//! screenshots) runs on a thread; its result comes back through a calloop
//! channel and is handled on the main loop.

use std::path::PathBuf;

use smithay::reexports::calloop::channel::{self, Event, Sender};
use smithay::reexports::calloop::LoopHandle;

use crate::state::Lumen;
use crate::volume;

pub enum Done {
    Volume(Result<volume::Level, volume::Error>),
    Screenshot(Result<PathBuf, String>),
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
    pub fn on_background_done(&mut self, done: Done) {
        match done {
            Done::Volume(Ok(level)) => self.shell_show_volume(level.volume, level.muted),
            Done::Volume(Err(volume::Error::Missing)) => {
                // Without PipeWire there is nothing to show; say so once.
                if !std::mem::replace(&mut self.wpctl_missing, true) {
                    tracing::warn!("volume keys: wpctl (PipeWire) is not installed");
                }
            }
            Done::Volume(Err(volume::Error::Failed(e))) => tracing::warn!("volume: {e}"),
            Done::Screenshot(Ok(path)) => {
                tracing::info!("screenshot saved to {}", path.display());
                self.notify("Screenshot saved", &display_path(&path));
            }
            Done::Screenshot(Err(e)) => {
                tracing::warn!("screenshot failed: {e}");
                self.notify("Screenshot failed", &e);
            }
        }
        self.queue_redraw_all();
    }
}
