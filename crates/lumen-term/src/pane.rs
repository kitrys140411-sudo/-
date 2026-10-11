//! A pane: one terminal with its shell, its scroll position, selection,
//! search and animated cursor.

use std::time::{Duration, Instant};

use lumen_term::layout::PaneId;
use lumen_term::pty::Pty;
use lumen_term::select::{self, Pos, Selection};
use lumen_term::term::Terminal;

/// Exponential approach of `v` to `target` with time constant `tau`.
pub fn approach(v: f64, target: f64, dt: f64, tau: f64) -> f64 {
    if tau <= 0.0 {
        return target;
    }
    let k = 1.0 - (-dt / tau).exp();
    let n = v + (target - v) * k;
    if (n - target).abs() < 1e-3 { target } else { n }
}

#[derive(Default)]
pub struct Scroll {
    /// Lines scrolled back from the live view (fractional while animating).
    pub offset: f64,
    pub target: f64,
    /// Kinetic scrolling (touchpad flick), lines per second.
    pub velocity: f64,
    /// `term.scrolled` when last looked at (to keep a scrolled-back view
    /// anchored while output continues).
    pub seen: u64,
    /// When the view last moved (the scrollbar fades out after).
    pub moved: Option<Instant>,
}

impl Scroll {
    pub fn animating(&self) -> bool {
        (self.offset - self.target).abs() > 1e-3 || self.velocity.abs() > 0.05
    }

    pub fn step(&mut self, dt: f64, max: f64, smooth: bool) {
        if self.velocity.abs() > 0.05 {
            self.target = (self.target + self.velocity * dt).clamp(0.0, max);
            self.velocity *= (-dt / 0.325).exp();
            if self.target <= 0.0 || self.target >= max {
                self.velocity = 0.0;
            }
            self.offset = self.target;
        } else {
            self.velocity = 0.0;
        }
        let before = self.offset;
        self.offset = if smooth { approach(self.offset, self.target, dt, 0.045) } else { self.target };
        self.offset = self.offset.clamp(0.0, max);
        self.target = self.target.clamp(0.0, max);
        if (self.offset - before).abs() > 1e-6 {
            self.moved = Some(Instant::now());
        }
    }
}

pub struct Search {
    pub query: String,
    pub matches: Vec<(Pos, Pos)>,
    /// Index into matches of the current one.
    pub current: Option<usize>,
    /// The query changed or new output arrived: recompute on the next frame.
    pub stale: bool,
}

pub struct Pane {
    pub id: PaneId,
    pub term: Terminal,
    pub pty: Pty,
    pub exited: bool,
    pub scroll: Scroll,
    pub selection: Option<Selection>,
    /// A drag selection is in progress.
    pub selecting: bool,
    pub search: Option<Search>,
    /// Animated cursor position in cells (fractional while gliding).
    pub cursor_x: f64,
    pub cursor_y: f64,
    /// When the cursor last moved or a key was typed (blink restarts).
    pub cursor_moved: Instant,
    pub last_cursor: (usize, usize),
    /// Bytes waiting for the PTY to accept them.
    pub write_queue: Vec<u8>,
    /// Size in cells.
    pub cols: usize,
    pub rows: usize,
    /// Synchronized output started (rendering is held back).
    pub sync_since: Option<Instant>,
    pub bell: Option<Instant>,
}

impl Pane {
    pub fn new(id: PaneId, term: Terminal, pty: Pty, cols: usize, rows: usize) -> Pane {
        Pane {
            id,
            term,
            pty,
            exited: false,
            scroll: Scroll::default(),
            selection: None,
            selecting: false,
            search: None,
            cursor_x: 0.0,
            cursor_y: 0.0,
            cursor_moved: Instant::now(),
            last_cursor: (0, 0),
            write_queue: Vec::new(),
            cols,
            rows,
            sync_since: None,
            bell: None,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        self.write_queue.extend_from_slice(data);
        self.flush();
    }

    /// Write as much of the queue as the PTY takes now.
    pub fn flush(&mut self) {
        while !self.write_queue.is_empty() {
            match self.pty.write(&self.write_queue) {
                Ok(0) => break,
                Ok(n) => {
                    self.write_queue.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.write_queue.clear();
                    break;
                }
            }
        }
    }

    pub fn history(&self) -> usize {
        self.term.term.grid().history()
    }

    /// Follow output: a scrolled-back view stays on the same content.
    pub fn track_output(&mut self) {
        let s = self.term.term.scrolled;
        let d = s.saturating_sub(self.scroll.seen) as f64;
        self.scroll.seen = s;
        if self.term.term.alt_active {
            self.scroll.offset = 0.0;
            self.scroll.target = 0.0;
            return;
        }
        if self.scroll.target > 0.0 && d > 0.0 {
            let max = self.history() as f64;
            self.scroll.offset = (self.scroll.offset + d).min(max);
            self.scroll.target = (self.scroll.target + d).min(max);
        }
    }

    pub fn scroll_by(&mut self, lines: f64) {
        let max = if self.term.term.alt_active { 0.0 } else { self.history() as f64 };
        self.scroll.velocity = 0.0;
        self.scroll.target = (self.scroll.target + lines).clamp(0.0, max);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll.velocity = 0.0;
        self.scroll.target = 0.0;
        self.scroll.offset = 0.0;
    }

    /// Absolute line number of screen row `y` of the live view.
    pub fn abs_line_of_screen_row(&self, y: usize) -> u64 {
        let g = self.term.term.grid();
        g.dropped + (g.history() + y) as u64
    }

    /// Animate the cursor toward the terminal cursor: glide over short
    /// distances (typing), jump across the screen.
    pub fn step_cursor(&mut self, dt: f64, smooth: bool) -> bool {
        let t = &self.term.term;
        let (tx, ty) = (t.cursor.x as f64, t.cursor.y as f64);
        if (t.cursor.x, t.cursor.y) != self.last_cursor {
            self.last_cursor = (t.cursor.x, t.cursor.y);
            self.cursor_moved = Instant::now();
            let far = (tx - self.cursor_x).abs() > 8.0 || (ty - self.cursor_y).abs() > 1.5;
            if !smooth || far {
                self.cursor_x = tx;
                self.cursor_y = ty;
            }
        }
        self.cursor_x = approach(self.cursor_x, tx, dt, 0.028);
        self.cursor_y = approach(self.cursor_y, ty, dt, 0.028);
        (self.cursor_x - tx).abs() > 1e-3 || (self.cursor_y - ty).abs() > 1e-3
    }

    /// Cursor opacity for blinking (smooth fades), and when it next changes.
    pub fn cursor_alpha(&self, now: Instant, blink: bool) -> (f32, Option<Duration>) {
        let since = now.saturating_duration_since(self.cursor_moved).as_secs_f64();
        // Blink only for a while after activity, like GNOME.
        if !blink || since > 10.0 {
            return (1.0, None);
        }
        const PERIOD: f64 = 1.2;
        let p = since % PERIOD;
        let (a, next) = if p < 0.5 {
            (1.0, 0.5 - p)
        } else if p < 0.65 {
            (1.0 - (p - 0.5) / 0.15, 0.0)
        } else if p < 1.05 {
            (0.0, 1.05 - p)
        } else {
            ((p - 1.05) / 0.15, 0.0)
        };
        // Ease.
        let a = a * a * (3.0 - 2.0 * a);
        let next = if since + next > 10.0 { 10.0 - since } else { next };
        (a as f32, Some(Duration::from_secs_f64(next.max(0.0))))
    }

    pub fn update_search(&mut self) {
        let Some(s) = &mut self.search else { return };
        if !s.stale {
            return;
        }
        s.stale = false;
        let prev = s.current.and_then(|i| s.matches.get(i).copied());
        s.matches = select::search(self.term.term.grid(), &s.query);
        s.current = match prev {
            Some(p) => s.matches.iter().position(|m| *m == p).or_else(|| s.matches.len().checked_sub(1)),
            None => s.matches.len().checked_sub(1),
        };
    }

    /// Move to the next (older when `up`) match and scroll it into view.
    pub fn search_step(&mut self, up: bool) {
        self.update_search();
        let rows = self.rows;
        let Some(s) = &mut self.search else { return };
        if s.matches.is_empty() {
            return;
        }
        let n = s.matches.len();
        let i = match s.current {
            Some(i) if up => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
            None => n - 1,
        };
        s.current = Some(i);
        let line = s.matches[i].0.line;
        // Scroll so the match is on screen (a third from the top).
        let g = self.term.term.grid();
        let live_top = g.dropped + g.history() as u64;
        let top_wanted = line.saturating_sub((rows / 3) as u64);
        let offset = live_top.saturating_sub(top_wanted.min(live_top)) as f64;
        let in_view = {
            let top = live_top as f64 - self.scroll.target;
            (line as f64) >= top && (line as f64) < top + rows as f64
        };
        if !in_view {
            self.scroll.velocity = 0.0;
            self.scroll.target = offset.min(g.history() as f64);
        }
    }
}
