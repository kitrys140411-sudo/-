//! Volume keys: PipeWire's `wpctl` changes the default sink's volume and
//! reports the new level for the on-screen display.
//!
//! wpctl runs on a background thread (it talks to PipeWire and may stall);
//! jobs are serialized by a lock, so each level read back follows its own
//! change and the last result to arrive is the current state.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// Volume up stops at 150 % (like GNOME's "over-amplification").
const LIMIT: &str = "1.5";
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeCmd {
    Up,
    Down,
    ToggleMute,
}

/// The sink's state as reported by `wpctl get-volume`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// 1.0 = 100 %.
    pub volume: f64,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// wpctl is not installed.
    Missing,
    Failed(String),
}

static LOCK: Mutex<()> = Mutex::new(());

/// The wpctl invocations for a command (run in order).
pub fn commands(cmd: VolumeCmd) -> Vec<Vec<&'static str>> {
    match cmd {
        // Raising the volume of a muted sink unmutes it.
        VolumeCmd::Up => vec![vec!["set-mute", SINK, "0"], vec!["set-volume", "-l", LIMIT, SINK, "5%+"]],
        VolumeCmd::Down => vec![vec!["set-volume", SINK, "5%-"]],
        VolumeCmd::ToggleMute => vec![vec!["set-mute", SINK, "toggle"]],
    }
}

/// Parse "Volume: 0.45" / "Volume: 0.45 [MUTED]".
pub fn parse_level(out: &str) -> Option<Level> {
    let rest = out.trim().strip_prefix("Volume:")?.trim();
    let volume = rest.split_whitespace().next()?.parse::<f64>().ok()?;
    Some(Level { volume, muted: rest.contains("[MUTED]") })
}

/// Run wpctl with `args`, killing it if it hangs; returns its stdout.
fn wpctl(args: &[&str]) -> Result<String, Error> {
    let mut child = Command::new("wpctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Error::Missing } else { Error::Failed(format!("wpctl: {e}")) })?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Failed(format!("wpctl {} timed out", args.join(" "))));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(Error::Failed(format!("wpctl: {e}"))),
        }
    };
    let (mut out, mut err) = (String::new(), String::new());
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    if !status.success() {
        return Err(Error::Failed(format!("wpctl {}: {}", args.join(" "), err.trim())));
    }
    Ok(out)
}

/// Apply `cmd` and read the new level back (blocking: call off the main loop).
pub fn run(cmd: VolumeCmd) -> Result<Level, Error> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for args in commands(cmd) {
        wpctl(&args)?;
    }
    let out = wpctl(&["get-volume", SINK])?;
    parse_level(&out).ok_or_else(|| Error::Failed(format!("unexpected wpctl output: {}", out.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_output() {
        assert_eq!(parse_level("Volume: 0.45\n"), Some(Level { volume: 0.45, muted: false }));
        assert_eq!(parse_level("Volume: 1.20 [MUTED]\n"), Some(Level { volume: 1.2, muted: true }));
        assert_eq!(parse_level("error"), None);
    }

    #[test]
    fn volume_up_is_limited_and_unmutes() {
        let c = commands(VolumeCmd::Up);
        assert_eq!(c[0], vec!["set-mute", SINK, "0"]);
        assert_eq!(c[1], vec!["set-volume", "-l", "1.5", SINK, "5%+"]);
        assert_eq!(commands(VolumeCmd::ToggleMute), vec![vec!["set-mute", SINK, "toggle"]]);
    }
}
