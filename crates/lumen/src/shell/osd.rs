//! On-screen display for the volume keys: a small glass pill with a speaker
//! icon, a level bar and the percentage, shown for a moment after a change.

use std::time::Duration;

use crate::anim::{Spring, SpringParams};

/// How long the OSD stays after the last change.
pub const SHOW_FOR: Duration = Duration::from_millis(1500);

pub struct Osd {
    /// 1.0 = 100 % (may go up to 1.5).
    pub level: f64,
    pub muted: bool,
    /// 0 = hidden, 1 = shown.
    pub anim: Spring,
    /// The bar follows level changes smoothly.
    pub bar: Spring,
    pub until: Duration,
}

impl Osd {
    pub fn new(level: f64, muted: bool, now: Duration, params: SpringParams) -> Osd {
        let mut anim = Spring::new(0.0, params);
        anim.set_target(1.0);
        let mut bar = Spring::new(level, params);
        bar.snap(level);
        Osd { level, muted, anim, bar, until: now + SHOW_FOR }
    }

    pub fn update(&mut self, level: f64, muted: bool, now: Duration) {
        self.level = level;
        self.muted = muted;
        self.bar.set_target(level);
        self.anim.set_target(1.0);
        self.until = now + SHOW_FOR;
    }

    pub fn advance(&mut self, dt: Duration, now: Duration) {
        if now >= self.until {
            self.anim.set_target(0.0);
        }
        self.anim.step(dt);
        self.bar.step(dt);
    }

    pub fn animating(&self) -> bool {
        !self.anim.is_settled() || !self.bar.is_settled()
    }

    pub fn gone(&self) -> bool {
        self.anim.target <= 0.0 && self.anim.is_settled()
    }

    /// Speaker icon for the level.
    pub fn icon(&self) -> &'static str {
        match self.level {
            _ if self.muted || self.level <= 0.0 => "audio-volume-muted",
            l if l < 0.34 => "audio-volume-low",
            l if l < 0.67 => "audio-volume-medium",
            _ => "audio-volume-high",
        }
    }
}
