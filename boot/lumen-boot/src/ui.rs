//! The graphical boot menu.
//!
//! A static layer (background, title, card with its shadow) is rendered once.
//! Each frame copies the dynamic region from it, draws the moving parts
//! (selection highlight, labels, countdown, editor, error dialog) and blits only
//! that region, unless a full-screen fade or dialog transition is running.
//! All motion is computed from elapsed time.

use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use uefi::boot::{self, EventType, ScopedProtocol, TimerTrigger, Tpl};
use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput};

use crate::clock::Clock;
use crate::font::{Style, strike};
use crate::gfx::{Canvas, Color, CornerCache, Rect};
use crate::input::{self, UiKey};

pub mod theme {
    use crate::gfx::Color;
    pub const BG_TOP: Color = Color::hex(0x16161c);
    pub const BG_BOTTOM: Color = Color::hex(0x0b0b0f);
    pub const ACCENT: Color = Color::hex(0x7aa2f7);
    pub const ACCENT_SOFT: Color = Color::hex(0xb4cbff);
    pub const CARD: Color = Color::hex(0x1d1d25);
    pub const BORDER: Color = Color::hex(0xffffff);
    pub const TEXT: Color = Color::hex(0xe6e6ee);
    pub const TEXT_STRONG: Color = Color::hex(0xffffff);
    pub const TEXT_DIM: Color = Color::hex(0x8b8b99);
    pub const TRACK: Color = Color::hex(0x2a2a34);
    pub const ERROR: Color = Color::hex(0xff7b72);
    pub const SHADOW: Color = Color::hex(0x000000);
    pub const FIELD: Color = Color::hex(0x121218);
}

/// One row in the menu.
pub struct Item {
    pub title: String,
    pub editable_cmdline: Option<String>,
}

pub enum Choice {
    /// Run item `index`; `cmdline` is set when the user edited it.
    Run { index: usize, cmdline: Option<String> },
}

enum Mode {
    Menu,
    Edit { text: Vec<char>, cursor: usize, started: u64 },
    Error { title: String, lines: Vec<String>, opened: u64 },
}

/// Critically damped spring, integrated in fixed sub-steps.
struct Spring {
    pos: f32,
    vel: f32,
    target: f32,
}

impl Spring {
    const STIFFNESS: f32 = 700.0;

    fn step(&mut self, dt_s: f32) {
        let damping = 2.0 * 26.457_513; // 2 * sqrt(STIFFNESS)
        let mut left = dt_s.min(0.1);
        while left > 0.0 {
            let h = left.min(0.004);
            let acc = -Self::STIFFNESS * (self.pos - self.target) - damping * self.vel;
            self.vel += acc * h;
            self.pos += self.vel * h;
            left -= h;
        }
        if (self.pos - self.target).abs() < 0.05 && self.vel.abs() < 0.5 {
            self.pos = self.target;
            self.vel = 0.0;
        }
    }

    fn settled(&self) -> bool {
        self.pos == self.target && self.vel == 0.0
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

struct Fade {
    start: u64,
    dur: u64,
    from: f32,
    to: f32,
    ease_in: bool,
}

impl Fade {
    fn value(&self, now: u64) -> f32 {
        let t = (now.saturating_sub(self.start)) as f32 / self.dur.max(1) as f32;
        let e = if self.ease_in { ease_in_cubic(t) } else { ease_out_cubic(t) };
        self.from + (self.to - self.from) * e
    }
    fn done(&self, now: u64) -> bool {
        now >= self.start + self.dur
    }
}

struct Layout {
    s: i32,
    card: Rect,
    row_h: i32,
    pad: i32,
    title_baseline: i32,
    logo_center: (i32, i32),
    footer: Rect,
    hints_baseline: i32,
    banner: Rect,
}

pub struct Menu<'c> {
    gop: Option<ScopedProtocol<GraphicsOutput>>,
    clock: &'c Clock,
    w: i32,
    h: i32,
    stat: Canvas,
    frame: Canvas,
    corners: CornerCache,
    items: Vec<Item>,
    default: usize,
    selected: usize,
    hl: Spring,
    countdown: Option<(u64, u64)>,
    mode: Mode,
    banner: Option<String>,
    layout: Layout,
    fade: Option<Fade>,
    full_dirty: bool,
    last_frame: u64,
    stats: FrameStats,
}

/// Render+blit timing, logged when the menu returns (used by tests/profiling).
#[derive(Default)]
struct FrameStats {
    frames: u64,
    total_us: u64,
    max_us: u64,
}

const DIALOG_US: u64 = 160_000;

impl<'c> Menu<'c> {
    pub fn new(
        gop: ScopedProtocol<GraphicsOutput>,
        clock: &'c Clock,
        items: Vec<Item>,
        default: usize,
        timeout_s: Option<u32>,
        banner: Option<String>,
    ) -> Self {
        let (w, h) = gop.current_mode_info().resolution();
        let (w, h) = (w as i32, h as i32);
        let s = if w >= 2400 || h >= 1500 { 2 } else { 1 };
        let layout = Self::compute_layout(w, h, s, items.len() as i32, banner.is_some());
        let now = clock.now_us();
        let default = default.min(items.len().saturating_sub(1));
        let row_y = (layout.card.y + layout.pad + default as i32 * layout.row_h) as f32;
        let mut m = Menu {
            gop: Some(gop),
            clock,
            w,
            h,
            stat: Canvas::new(w, h),
            frame: Canvas::new(w, h),
            corners: CornerCache::new(),
            items,
            default,
            selected: default,
            hl: Spring { pos: row_y, vel: 0.0, target: row_y },
            countdown: timeout_s.map(|t| (now, t as u64 * 1_000_000)),
            mode: Mode::Menu,
            banner,
            layout,
            fade: None,
            full_dirty: true,
            last_frame: now,
            stats: FrameStats::default(),
        };
        m.render_static();
        m
    }

    fn compute_layout(w: i32, h: i32, s: i32, n: i32, banner: bool) -> Layout {
        let pad = 8 * s;
        let card_w = (560 * s).min(w - 48 * s);
        let banner_h = if banner { 64 * s } else { 0 };
        // Everything except the card's rows; rows shrink on small screens.
        let chrome = 56 * s + 20 * s + 44 * s + 36 * s + banner_h + 28 * s + 40 * s + 2 * pad + 48 * s;
        let row_h = ((h - chrome) / n.max(1)).clamp(32 * s, 52 * s);
        let card_h = pad * 2 + n * row_h;
        let block = 56 * s + 20 * s + 44 * s + 36 * s + banner_h + card_h + 28 * s + 40 * s;
        let top = ((h - block) / 2).max(16 * s);
        let cx = w / 2;
        let logo_center = (cx, top + 28 * s);
        let title_baseline = top + 56 * s + 20 * s + 36 * s;
        let banner_rect = Rect::new(cx - card_w / 2, title_baseline + 36 * s, card_w, banner_h - 12 * s);
        let card = Rect::new(cx - card_w / 2, title_baseline + 36 * s + banner_h, card_w, card_h);
        let footer = Rect::new(card.x, card.bottom() + 16 * s, card_w, 64 * s);
        Layout {
            s,
            card,
            row_h,
            pad,
            title_baseline,
            logo_center,
            footer,
            hints_baseline: h - 28 * s,
            banner: banner_rect,
        }
    }

    fn render_static(&mut self) {
        let l = &self.layout;
        let s = l.s;
        let c = &mut self.stat;
        c.background(
            theme::BG_TOP,
            theme::BG_BOTTOM,
            theme::ACCENT,
            150,
            (self.w / 2, l.logo_center.1 + 40 * s),
            (self.w * 6 / 10, self.h * 5 / 10),
        );
        // Logo: a soft light source above the title.
        let (lx, ly) = l.logo_center;
        let circle = |r: i32| Rect::new(lx - r, ly - r, 2 * r, 2 * r);
        c.shadow(&mut self.corners, circle(18 * s), 18 * s, 42 * s, 0, theme::ACCENT, 190);
        c.rounded_rect(&mut self.corners, circle(16 * s), 16 * s, theme::ACCENT, 110);
        c.rounded_rect(&mut self.corners, circle(12 * s), 12 * s, theme::ACCENT_SOFT, 255);
        let title = strike(Style::Title, s as u32);
        let tw = title.width("Lumen");
        title.draw_px(c, self.w / 2 - tw / 2, l.title_baseline, "Lumen", theme::TEXT_STRONG, 255);
        // Card with a soft shadow and hairline border.
        let radius = 18 * s;
        c.shadow(&mut self.corners, l.card, radius, 36 * s, 10 * s, theme::SHADOW, 150);
        c.rounded_rect(&mut self.corners, l.card, radius, theme::CARD, 235);
        c.rounded_border(&mut self.corners, l.card, radius, s, theme::BORDER, 18);
        if let Some(text) = &self.banner {
            let b = l.banner;
            c.rounded_rect(&mut self.corners, b, 12 * s, theme::ERROR, 34);
            c.rounded_border(&mut self.corners, b, 12 * s, s, theme::ERROR, 90);
            let small = strike(Style::Small, s as u32);
            let body = strike(Style::BodyMedium, s as u32);
            body.draw_px(c, b.x + 16 * s, b.y + 22 * s, "Configuration problem", theme::ERROR, 255);
            let line = ellipsize(small, text, b.w - 32 * s);
            small.draw_px(c, b.x + 16 * s, b.y + 42 * s, &line, theme::TEXT, 255);
        }
        let hints = "↑ ↓  select      ⏎  start      E  edit command line      Esc  stop timer";
        let small = strike(Style::Small, s as u32);
        let hw = small.width(hints);
        small.draw_px(c, self.w / 2 - hw / 2, l.hints_baseline, hints, theme::TEXT_DIM, 200);
        self.full_dirty = true;
    }

    fn dynamic_rect(&self) -> Rect {
        let l = &self.layout;
        l.card.inset(-4 * l.s).union(&l.footer)
    }

    // -- display ownership (the kernel's EFI stub must be able to use GOP) --------------

    pub fn release_display(&mut self) {
        self.gop = None;
    }

    pub fn acquire_display(&mut self) {
        if self.gop.is_none() {
            if let Ok(h) = boot::get_handle_for_protocol::<GraphicsOutput>() {
                self.gop = boot::open_protocol_exclusive::<GraphicsOutput>(h).ok();
            }
        }
        self.full_dirty = true;
    }

    // -- public flow ---------------------------------------------------------------------------

    pub fn show_error(&mut self, title: &str, detail: &str) {
        let now = self.clock.now_us();
        let small = strike(Style::Body, self.layout.s as u32);
        let lines = wrap(small, detail, self.dialog_rect(4).w - 48 * self.layout.s);
        self.mode = Mode::Error { title: title.into(), lines, opened: now };
        self.countdown = None;
        self.full_dirty = true;
    }

    pub fn fade_in(&mut self) {
        let now = self.clock.now_us();
        self.fade = Some(Fade { start: now, dur: 380_000, from: 0.0, to: 1.0, ease_in: false });
    }

    /// Fade the screen to black (blocking, ~220 ms).
    pub fn fade_out(&mut self) {
        let now = self.clock.now_us();
        self.fade = Some(Fade { start: now, dur: 220_000, from: 1.0, to: 0.0, ease_in: true });
        loop {
            let now = self.clock.now_us();
            let done = self.fade.as_ref().map(|f| f.done(now)).unwrap_or(true);
            self.render(now);
            if done {
                break;
            }
            boot::stall(Duration::from_millis(4));
        }
    }

    /// Run the menu until the user (or the timer) picks an entry.
    pub fn run(&mut self) -> Choice {
        let key_event = input::key_event();
        // SAFETY: plain timer event without a notification function.
        let timer = unsafe { boot::create_event(EventType::TIMER, Tpl::APPLICATION, None, None) }.ok();
        loop {
            let now = self.clock.now_us();
            while let Some(k) = input::poll() {
                if let Some(choice) = self.on_key(k, now) {
                    if let Some(t) = timer {
                        let _ = boot::close_event(t);
                    }
                    self.log_stats();
                    return choice;
                }
            }
            if let Some((start, dur)) = self.countdown {
                if now >= start + dur && matches!(self.mode, Mode::Menu) {
                    crate::log!("timeout, starting entry {}", self.selected);
                    if let Some(t) = timer {
                        let _ = boot::close_event(t);
                    }
                    self.log_stats();
                    return Choice::Run { index: self.selected, cmdline: None };
                }
            }
            let animating = self.update(now);
            self.render(now);
            // Sleep until the next frame (animating), the next countdown/caret
            // update, or a key press.
            let wait_us: u64 = if animating {
                16_000u64.saturating_sub(self.clock.now_us() - now).max(1_000)
            } else if self.countdown.is_some() {
                33_000
            } else if matches!(self.mode, Mode::Edit { .. }) {
                100_000
            } else {
                500_000
            };
            match (&timer, &key_event) {
                (Some(t), Some(k)) => {
                    let _ = boot::set_timer(t, TimerTrigger::Relative(Duration::from_micros(wait_us)));
                    // SAFETY: the events stay valid for the duration of the wait.
                    let events = unsafe { [t.unsafe_clone(), k.unsafe_clone()] };
                    let _ = boot::wait_for_event(&events);
                }
                _ => boot::stall(Duration::from_micros(wait_us)),
            }
        }
    }

    fn select(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = index;
            let l = &self.layout;
            self.hl.target = (l.card.y + l.pad + index as i32 * l.row_h) as f32;
        }
    }

    fn on_key(&mut self, key: UiKey, now: u64) -> Option<Choice> {
        crate::log!("key {:?}", key);
        if self.countdown.take().is_some() {
            crate::log!("timer stopped");
        }
        match &mut self.mode {
            Mode::Error { .. } => {
                self.mode = Mode::Menu;
                self.full_dirty = true;
                None
            }
            Mode::Edit { text, cursor, started } => {
                *started = now;
                match key {
                    UiKey::Escape => self.mode = Mode::Menu,
                    UiKey::Enter => {
                        let cmdline: String = text.iter().collect();
                        return Some(Choice::Run { index: self.selected, cmdline: Some(cmdline) });
                    }
                    UiKey::Left => *cursor = cursor.saturating_sub(1),
                    UiKey::Right => *cursor = (*cursor + 1).min(text.len()),
                    UiKey::Home => *cursor = 0,
                    UiKey::End => *cursor = text.len(),
                    UiKey::Backspace if *cursor > 0 => {
                        *cursor -= 1;
                        text.remove(*cursor);
                    }
                    UiKey::Delete if *cursor < text.len() => {
                        text.remove(*cursor);
                    }
                    UiKey::Char(c) if (' '..='~').contains(&c) && text.len() < 1024 => {
                        text.insert(*cursor, c);
                        *cursor += 1;
                    }
                    _ => {}
                }
                None
            }
            Mode::Menu => match key {
                UiKey::Up => {
                    self.select(self.selected.saturating_sub(1));
                    None
                }
                UiKey::Down => {
                    self.select((self.selected + 1).min(self.items.len() - 1));
                    None
                }
                UiKey::Home => {
                    self.select(0);
                    None
                }
                UiKey::End => {
                    self.select(self.items.len() - 1);
                    None
                }
                UiKey::Enter => Some(Choice::Run { index: self.selected, cmdline: None }),
                UiKey::Char(c @ '1'..='9') => {
                    let i = c as usize - '1' as usize;
                    if i < self.items.len() {
                        self.select(i);
                        return Some(Choice::Run { index: i, cmdline: None });
                    }
                    None
                }
                UiKey::Char('e' | 'E') => {
                    if let Some(cmd) = &self.items[self.selected].editable_cmdline {
                        let text: Vec<char> = cmd.chars().collect();
                        let cursor = text.len();
                        self.mode = Mode::Edit { text, cursor, started: now };
                    }
                    None
                }
                _ => None,
            },
        }
    }

    /// Advance animations; returns whether anything is still moving.
    fn update(&mut self, now: u64) -> bool {
        let dt = (now - self.last_frame) as f32 / 1_000_000.0;
        self.last_frame = now;
        self.hl.step(dt);
        let mut animating = !self.hl.settled();
        if let Some(f) = &self.fade {
            if f.done(now) && f.to >= 1.0 {
                self.fade = None;
                self.full_dirty = true;
            } else {
                animating = true;
            }
        }
        if let Mode::Error { opened, .. } = &self.mode {
            if now < opened + DIALOG_US + 20_000 {
                animating = true;
                self.full_dirty = true;
            }
        }
        animating
    }

    fn dialog_rect(&self, lines: usize) -> Rect {
        let s = self.layout.s;
        let w = (520 * s).min(self.w - 48 * s);
        let h = 112 * s + lines as i32 * 22 * s;
        Rect::new(self.w / 2 - w / 2, self.h / 2 - h / 2, w, h)
    }

    fn render(&mut self, now: u64) {
        self.render_frame(now);
        let took = self.clock.now_us() - now;
        self.stats.frames += 1;
        self.stats.total_us += took;
        self.stats.max_us = self.stats.max_us.max(took);
    }

    fn log_stats(&mut self) {
        let st = &self.stats;
        if st.frames > 0 {
            crate::log!("frames {} avg {} us max {} us", st.frames, st.total_us / st.frames, st.max_us);
        }
        self.stats = FrameStats::default();
    }

    fn render_frame(&mut self, now: u64) {
        let fade = self.fade.as_ref().map(|f| f.value(now));
        let full = self.full_dirty || fade.is_some() || matches!(self.mode, Mode::Error { .. });
        let region = if full { Rect::new(0, 0, self.w, self.h) } else { self.dynamic_rect() };
        self.frame.copy_from(&self.stat, region);
        self.draw_dynamic(now);
        let dialog = match &self.mode {
            Mode::Error { title, lines, opened } => Some((title.clone(), lines.clone(), *opened)),
            _ => None,
        };
        if let Some((title, lines, opened)) = dialog {
            let t = ((now - opened) as f32 / DIALOG_US as f32).clamp(0.0, 1.0);
            self.draw_dialog(title, lines, ease_out_cubic(t));
        }
        if let Some(f) = fade {
            let k = (f.clamp(0.0, 1.0) * 256.0) as u32;
            self.frame.fade(region, k);
        }
        self.blit(region);
        self.full_dirty = false;
    }

    fn draw_dynamic(&mut self, now: u64) {
        let l = &self.layout;
        let s = l.s;
        let body = strike(Style::Body, s as u32);
        let medium = strike(Style::BodyMedium, s as u32);
        let small = strike(Style::Small, s as u32);
        // Selection highlight.
        let hl = Rect::new(l.card.x + l.pad, self.hl.pos as i32, l.card.w - 2 * l.pad, l.row_h);
        self.frame.rounded_rect(&mut self.corners, hl, 12 * s, theme::ACCENT, 38);
        self.frame.rounded_border(&mut self.corners, hl, 12 * s, s, theme::ACCENT, 70);
        // Rows.
        for (i, item) in self.items.iter().enumerate() {
            let y = l.card.y + l.pad + i as i32 * l.row_h;
            let baseline = y + l.row_h / 2 + (body.ascent + body.descent) / 2;
            let x = l.card.x + l.pad + 18 * s;
            let (st, color) = if i == self.selected { (medium, theme::TEXT_STRONG) } else { (body, theme::TEXT) };
            let num = [b'1' + i as u8];
            let num = core::str::from_utf8(&num[..]).unwrap_or("");
            if i < 9 {
                small.draw_px(&mut self.frame, x, baseline, num, theme::TEXT_DIM, 200);
            }
            st.draw_px(&mut self.frame, x + 22 * s, baseline, &item.title, color, 255);
            if i == self.default {
                let tag = "default";
                let tw = small.width(tag);
                small.draw_px(&mut self.frame, l.card.right() - l.pad - 18 * s - tw, baseline, tag, theme::TEXT_DIM, 220);
            }
        }
        // Footer: countdown, editor, or idle hint.
        let f = l.footer;
        match &self.mode {
            Mode::Edit { text, cursor, started } => {
                small.draw_px(&mut self.frame, f.x + 4 * s, f.y + 12 * s, "Kernel command line  (Enter to start, Esc to cancel)", theme::TEXT_DIM, 255);
                let field = Rect::new(f.x, f.y + 22 * s, f.w, 36 * s);
                self.frame.rounded_rect(&mut self.corners, field, 10 * s, theme::FIELD, 255);
                self.frame.rounded_border(&mut self.corners, field, 10 * s, s, theme::ACCENT, 120);
                let inner_w = field.w - 28 * s;
                // Scroll horizontally so the cursor stays visible.
                let mut start = 0;
                loop {
                    let before: String = text[start..*cursor].iter().collect();
                    if body.width(&before) <= inner_w - 8 * s || start >= *cursor {
                        break;
                    }
                    start += 1;
                }
                let visible: String = text[start..].iter().collect();
                let visible = clip_to_width(body, &visible, inner_w);
                let baseline = field.y + field.h / 2 + (body.ascent + body.descent) / 2;
                let tx = field.x + 14 * s;
                body.draw_px(&mut self.frame, tx, baseline, &visible, theme::TEXT, 255);
                let before: String = text[start..*cursor].iter().collect();
                let cx = tx + body.width(&before);
                // Caret blinks with a 1.06 s period, solid right after typing.
                if (now - started) % 1_060_000 < 600_000 {
                    self.frame.fill(Rect::new(cx, field.y + 9 * s, s.max(2), field.h - 18 * s), theme::ACCENT_SOFT);
                }
            }
            _ => {
                if let Some((start, dur)) = self.countdown {
                    let left = (start + dur).saturating_sub(now);
                    let secs = left.div_ceil(1_000_000);
                    let mut msg = String::from("Starting ");
                    msg.push_str(&self.items[self.selected].title);
                    msg.push_str(" in ");
                    push_num(&mut msg, secs);
                    msg.push_str(" s");
                    let tw = small.width(&msg);
                    small.draw_px(&mut self.frame, f.x + f.w / 2 - tw / 2, f.y + 14 * s, &msg, theme::TEXT_DIM, 255);
                    let track = Rect::new(f.x + f.w / 4, f.y + 28 * s, f.w / 2, 3 * s);
                    self.frame.rounded_rect(&mut self.corners, track, 2 * s, theme::TRACK, 255);
                    let frac = (left as i64 * 4096 / dur.max(1) as i64) as i32;
                    let bar = Rect::new(track.x, track.y, track.w * frac / 4096, track.h);
                    if bar.w > 0 {
                        self.frame.rounded_rect(&mut self.corners, bar, 2 * s, theme::ACCENT, 255);
                    }
                }
            }
        }
    }

    fn draw_dialog(&mut self, title: String, lines: Vec<String>, a: f32) {
        let s = self.layout.s;
        let ai = (a * 255.0) as i32;
        self.frame.fill_alpha(Rect::new(0, 0, self.w, self.h), Color::hex(0x000000), (ai * 150) / 255);
        let mut r = self.dialog_rect(lines.len());
        r.y += ((1.0 - a) * 12.0 * s as f32) as i32;
        self.frame.shadow(&mut self.corners, r, 18 * s, 30 * s, 8 * s, theme::SHADOW, (ai * 160) / 255);
        self.frame.rounded_rect(&mut self.corners, r, 18 * s, theme::CARD, ai);
        self.frame.rounded_border(&mut self.corners, r, 18 * s, s, theme::ERROR, (ai * 80) / 255);
        let medium = strike(Style::BodyMedium, s as u32);
        let body = strike(Style::Body, s as u32);
        let small = strike(Style::Small, s as u32);
        // Error badge.
        let badge = Rect::new(r.x + 24 * s, r.y + 24 * s, 28 * s, 28 * s);
        self.frame.rounded_rect(&mut self.corners, badge, 14 * s, theme::ERROR, ai);
        let bw = medium.width("!");
        medium.draw_px(&mut self.frame, badge.x + badge.w / 2 - bw / 2, badge.y + 20 * s, "!", Color::hex(0x1d1d25), ai);
        medium.draw_px(&mut self.frame, r.x + 64 * s, r.y + 44 * s, &title, theme::TEXT_STRONG, ai);
        for (i, line) in lines.iter().enumerate() {
            body.draw_px(&mut self.frame, r.x + 24 * s, r.y + 82 * s + i as i32 * 22 * s, line, theme::TEXT, ai);
        }
        let hint = "Press any key to return to the menu";
        small.draw_px(&mut self.frame, r.x + 24 * s, r.bottom() - 20 * s, hint, theme::TEXT_DIM, ai);
    }

    fn blit(&mut self, r: Rect) {
        let r = r.clip(self.w, self.h);
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        let Some(gop) = self.gop.as_mut() else { return };
        // SAFETY: BltPixel is a 4-byte repr(C) {b, g, r, reserved} struct, which is
        // exactly the little-endian layout of our 0x00RRGGBB pixels.
        let buf: &[BltPixel] =
            unsafe { core::slice::from_raw_parts(self.frame.px.as_ptr().cast(), self.frame.px.len()) };
        let _ = gop.blt(BltOp::BufferToVideo {
            buffer: buf,
            src: BltRegion::SubRectangle {
                coords: (r.x as usize, r.y as usize),
                px_stride: self.w as usize,
            },
            dest: (r.x as usize, r.y as usize),
            dims: (r.w as usize, r.h as usize),
        });
    }
}

fn push_num(s: &mut String, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    s.push_str(core::str::from_utf8(&digits[i..]).unwrap_or("?"));
}

fn clip_to_width(st: &crate::font::Strike, text: &str, max_w: i32) -> String {
    let mut out = String::new();
    for c in text.chars() {
        out.push(c);
        if st.width(&out) > max_w {
            out.pop();
            break;
        }
    }
    out
}

fn ellipsize(st: &crate::font::Strike, text: &str, max_w: i32) -> String {
    if st.width(text) <= max_w {
        return text.into();
    }
    let mut out = clip_to_width(st, text, max_w - st.width("…"));
    out.push('…');
    out
}

/// Greedy word wrap; words longer than a line are broken.
fn wrap(st: &crate::font::Strike, text: &str, max_w: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        let mut candidate = cur.clone();
        if !candidate.is_empty() {
            candidate.push(' ');
        }
        candidate.push_str(word);
        if st.width(&candidate) <= max_w {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            lines.push(core::mem::take(&mut cur));
        }
        let mut rest = String::from(word);
        while st.width(&rest) > max_w {
            let head = clip_to_width(st, &rest, max_w);
            let n = head.chars().count().max(1);
            lines.push(head);
            rest = rest.chars().skip(n).collect();
        }
        cur = rest;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.truncate(8);
    lines
}
