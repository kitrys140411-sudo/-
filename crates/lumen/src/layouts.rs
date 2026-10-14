//! Keyboard layouts as the shell sees them: the panel label of each
//! configured layout, the active one, and (optionally) the layout each
//! window last used.
//!
//! xkb keeps the actual state (the keymap's effective layout); this mirrors
//! it after every key event and layout change, so the panel can show it and
//! focus changes can restore a window's own layout.

use std::collections::HashMap;

use smithay::input::keyboard::{Layout, XkbConfig};

use crate::inputcfg::{InputConfig, KeyboardConfig};
use crate::state::Lumen;

#[derive(Debug, Clone, PartialEq)]
pub struct Layouts {
    /// Panel label per layout ("EN", "RU").
    pub labels: Vec<String>,
    /// xkb layout codes, in keymap order.
    pub names: Vec<String>,
    pub active: u32,
    pub per_window: bool,
    /// Last layout of each window (per-window mode).
    memory: HashMap<u32, u32>,
}

impl Layouts {
    pub fn new(cfg: &KeyboardConfig) -> Layouts {
        Layouts { labels: cfg.labels(), names: cfg.layouts.0.clone(), active: 0, per_window: cfg.per_window, memory: HashMap::new() }
    }

    pub fn count(&self) -> u32 {
        self.names.len().max(1) as u32
    }

    /// The panel label, when there is more than one layout to switch between.
    pub fn indicator(&self) -> Option<&str> {
        (self.labels.len() > 1).then(|| self.labels.get(self.active as usize).map(String::as_str).unwrap_or("?"))
    }

    /// The next (or previous) layout index from the active one.
    pub fn cycle(&self, backward: bool) -> u32 {
        let n = self.count();
        if backward { (self.active + n - 1) % n } else { (self.active + 1) % n }
    }

    /// The active layout changed (`window` has the keyboard): returns true if
    /// it is a change.
    pub fn set_active(&mut self, active: u32, window: Option<u32>) -> bool {
        if self.per_window {
            if let Some(w) = window {
                self.memory.insert(w, active);
            }
        }
        std::mem::replace(&mut self.active, active) != active
    }

    /// The layout to switch to when `window` gets the keyboard (per-window
    /// mode only): its own last one, the first layout for a new window.
    pub fn for_window(&self, window: u32) -> Option<u32> {
        self.per_window.then(|| self.memory.get(&window).copied().unwrap_or(0).min(self.count() - 1))
    }

    pub fn forget(&mut self, window: u32) {
        self.memory.remove(&window);
    }
}

/// The xkb names for a keyboard configuration (owned, for [`XkbConfig`]).
pub struct XkbNames {
    layout: String,
    variant: String,
    model: String,
    options: Option<String>,
}

impl XkbNames {
    pub fn new(k: &KeyboardConfig) -> XkbNames {
        XkbNames { layout: k.xkb_layout(), variant: k.xkb_variant(), model: k.model.clone(), options: k.xkb_options() }
    }

    pub fn config(&self) -> XkbConfig<'_> {
        XkbConfig { layout: &self.layout, variant: &self.variant, model: &self.model, options: self.options.clone(), ..Default::default() }
    }

    pub fn describe(&self) -> String {
        let mut s = format!("layouts '{}'", self.layout);
        if !self.variant.is_empty() {
            s += &format!(", variants '{}'", self.variant);
        }
        if let Some(o) = &self.options {
            s += &format!(", options '{o}'");
        }
        s
    }
}

impl Lumen {
    /// The layout xkb has active now.
    pub fn xkb_active_layout(&mut self) -> u32 {
        let Some(kb) = self.seat.get_keyboard() else { return 0 };
        kb.with_xkb_state(self, |ctx| ctx.xkb().lock().unwrap().active_layout().0)
    }

    /// Make layout `index` active; the focused client learns it with the
    /// next modifiers event (sent right away).
    pub fn set_layout(&mut self, index: u32) {
        let Some(kb) = self.seat.get_keyboard() else { return };
        let index = index.min(self.layouts.count() - 1);
        kb.with_xkb_state(self, |mut ctx| ctx.set_layout(Layout(index)));
        self.sync_layout();
    }

    pub fn switch_layout(&mut self, backward: bool) {
        let next = self.layouts.cycle(backward);
        self.set_layout(next);
    }

    /// Mirror xkb's active layout into the panel (and the focused window's
    /// memory).
    pub fn sync_layout(&mut self) {
        let active = self.xkb_active_layout();
        let window = if self.shell.wants_keyboard() { None } else { self.wm.focused };
        if self.layouts.set_active(active, window) {
            tracing::debug!(layout = active, "keyboard layout");
            self.shell.layout_label = self.layouts.indicator().map(String::from);
            self.queue_redraw_all();
        }
    }

    /// Per-window layouts: `window` just got the keyboard.
    pub fn restore_window_layout(&mut self, window: u32) {
        if let Some(index) = self.layouts.for_window(window) {
            if index != self.layouts.active {
                self.set_layout(index);
            }
        }
    }

    /// Use a new keyboard configuration (keymap, repeat). On a keymap
    /// error the previous keymap stays and the error is returned.
    pub fn apply_keyboard_config(&mut self, k: &KeyboardConfig) -> Result<(), String> {
        let Some(kb) = self.seat.get_keyboard() else { return Err("no keyboard".into()) };
        let names = XkbNames::new(k);
        kb.set_xkb_config(self, names.config()).map_err(|_| format!("xkb cannot build a keymap for {}", names.describe()))?;
        kb.change_repeat_info(k.repeat_rate, k.repeat_delay);
        self.layouts = Layouts::new(k);
        self.shell.layout_label = self.layouts.indicator().map(String::from);
        self.sync_layout();
        self.queue_redraw_all();
        Ok(())
    }

    /// Re-read input.toml and apply it (keyboard and pointer devices). On
    /// any error nothing changes and the problem is shown as a notification.
    pub fn reload_input(&mut self) -> Result<String, String> {
        let result = InputConfig::reload().and_then(|cfg| {
            if cfg.keyboard != self.input_cfg.keyboard {
                self.apply_keyboard_config(&cfg.keyboard)?;
            }
            self.input_cfg = cfg;
            let n = self.backend.configure_input_devices(&self.input_cfg);
            Ok(format!("{} layouts, {n} pointer devices", self.layouts.count()))
        });
        match &result {
            Ok(summary) => tracing::info!("input settings reloaded: {summary}"),
            Err(e) => {
                tracing::error!("input settings: {e}");
                self.notify("Input settings not updated", &format!("{e}\nThe previous settings stay in effect."));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputcfg::List;

    fn layouts(names: &[&str], per_window: bool) -> Layouts {
        let cfg = KeyboardConfig { layouts: List(names.iter().map(|s| s.to_string()).collect()), per_window, ..KeyboardConfig::default() };
        Layouts::new(&cfg)
    }

    #[test]
    fn indicator_and_cycling() {
        let mut l = layouts(&["us", "ru", "ua"], false);
        assert_eq!(l.indicator(), Some("EN"));
        assert_eq!((l.cycle(false), l.cycle(true)), (1, 2));
        assert!(l.set_active(2, Some(7)));
        assert!(!l.set_active(2, Some(7)));
        assert_eq!(l.indicator(), Some("UK"));
        assert_eq!(l.cycle(false), 0);
        assert_eq!(l.for_window(7), None, "global mode restores nothing");
        assert_eq!(layouts(&["us"], false).indicator(), None);
    }

    #[test]
    fn per_window_memory() {
        let mut l = layouts(&["us", "ru"], true);
        l.set_active(1, Some(1));
        assert_eq!(l.for_window(1), Some(1));
        assert_eq!(l.for_window(2), Some(0), "a new window starts with the first layout");
        l.set_active(0, Some(2));
        assert_eq!(l.for_window(2), Some(0));
        l.forget(1);
        assert_eq!(l.for_window(1), Some(0));
        // The shell having the keyboard (no window) remembers nothing.
        l.set_active(1, None);
        assert_eq!(l.for_window(2), Some(0));
    }
}
