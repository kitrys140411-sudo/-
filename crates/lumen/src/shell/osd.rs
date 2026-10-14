//! On-screen display for the volume and brightness keys: a small glass pill
//! with an icon, a level bar and the percentage, shown for a moment after a
//! change.

use std::time::Duration;

use crate::anim::{Spring, SpringParams};

/// How long the OSD stays after the last change.
pub const SHOW_FOR: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
}

impl OsdKind {
    pub fn name(self) -> &'static str {
        match self {
            OsdKind::Volume => "volume",
            OsdKind::Brightness => "brightness",
        }
    }
}

pub struct Osd {
    pub kind: OsdKind,
    /// 1.0 = 100 % (volume may go up to 1.5).
    pub level: f64,
    pub muted: bool,
    /// 0 = hidden, 1 = shown.
    pub anim: Spring,
    /// The bar follows level changes smoothly.
    pub bar: Spring,
    pub until: Duration,
}

impl Osd {
    pub fn new(kind: OsdKind, level: f64, muted: bool, now: Duration, params: SpringParams) -> Osd {
        let mut anim = Spring::new(0.0, params);
        anim.set_target(1.0);
        let mut bar = Spring::new(level, params);
        bar.snap(level);
        Osd { kind, level, muted, anim, bar, until: now + SHOW_FOR }
    }

    pub fn update(&mut self, kind: OsdKind, level: f64, muted: bool, now: Duration) {
        if kind != self.kind {
            // A different quantity: no animation from the old one's level.
            self.kind = kind;
            self.bar.snap(level);
        }
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

    /// Icon for the kind and level.
    pub fn icon(&self) -> &'static str {
        match self.kind {
            OsdKind::Brightness => "display-brightness",
            OsdKind::Volume => match self.level {
                _ if self.muted || self.level <= 0.0 => "audio-volume-muted",
                l if l < 0.34 => "audio-volume-low",
                l if l < 0.67 => "audio-volume-medium",
                _ => "audio-volume-high",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_and_kind_changes() {
        let p = SpringParams::new(400.0, 1.0, 0.001);
        let mut o = Osd::new(OsdKind::Volume, 0.5, false, Duration::ZERO, p);
        assert_eq!(o.icon(), "audio-volume-medium");
        o.update(OsdKind::Volume, 0.2, true, Duration::from_millis(100));
        assert_eq!(o.icon(), "audio-volume-muted");
        assert!(!o.bar.is_settled(), "volume changes animate the bar");
        o.update(OsdKind::Brightness, 0.8, false, Duration::from_millis(200));
        assert_eq!((o.icon(), o.bar.value, o.until), ("display-brightness", 0.8, Duration::from_millis(1700)));
        o.advance(Duration::from_millis(16), Duration::from_millis(1800));
        assert_eq!(o.anim.target, 0.0, "hides after a while");
    }
}
