//! Turning panes and window chrome into GPU instances, with damage tracking:
//! every region (row, header, overlay) is hashed; regions whose hash or
//! place changed are redrawn, everything else stays in the canvas.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use lumen_term::config::{Config, CursorStyle};
use lumen_term::layout::Rect;
use lumen_term::term::{attr, Cell, Color, CursorShape, Term, Underline};

use lumen_ui::gfx::atlas::{Atlas, Entry};
use lumen_ui::gfx::glyphs::{CellMetrics, Fonts, GlyphKey, BOLD, ITALIC, PHASES, TOFU};
use lumen_ui::gfx::{boxdraw, color, GlyphInst, RectInst, Scene, KIND_CURL, KIND_DASHED, KIND_DOTTED, KIND_FILL, KIND_OUTLINE};
use crate::pane::Pane;

pub type Rgb = (u8, u8, u8);

/// Integer rectangle in buffer pixels (top-left origin).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub fn of(r: Rect) -> IRect {
        let x0 = r.x.floor() as i32;
        let y0 = r.y.floor() as i32;
        IRect { x: x0, y: y0, w: (r.x + r.w).ceil() as i32 - x0, h: (r.y + r.h).ceil() as i32 - y0 }
    }

    pub fn union(self, o: IRect) -> IRect {
        if self.w <= 0 || self.h <= 0 {
            return o;
        }
        if o.w <= 0 || o.h <= 0 {
            return self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    pub fn clamp_to(self, w: i32, h: i32) -> Option<IRect> {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x + self.w).min(w);
        let y1 = (self.y + self.h).min(h);
        (x1 > x0 && y1 > y0).then_some(IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
}

/// Regions drawn last frame: their rectangle and content hash.
#[derive(Default)]
pub struct Damage {
    prev: HashMap<u64, (IRect, u64)>,
    cur: HashMap<u64, (IRect, u64)>,
    pub dirty: Option<IRect>,
}

impl Damage {
    pub fn begin(&mut self) {
        self.cur.clear();
        self.dirty = None;
    }

    /// Record region `key` at `rect` with content `hash`.
    pub fn region(&mut self, key: u64, rect: IRect, hash: u64) {
        match self.prev.get(&key) {
            Some(&(r, h)) if r == rect && h == hash => {}
            Some(&(r, _)) => self.add(r.union(rect)),
            None => self.add(rect),
        }
        self.cur.insert(key, (rect, hash));
    }

    pub fn add(&mut self, r: IRect) {
        self.dirty = Some(match self.dirty {
            Some(d) => d.union(r),
            None => r,
        });
    }

    /// Regions that disappeared are damaged too.
    pub fn finish(&mut self) {
        let gone: Vec<IRect> = self.prev.iter().filter(|(k, _)| !self.cur.contains_key(k)).map(|(_, (r, _))| *r).collect();
        for r in gone {
            self.add(r);
        }
        std::mem::swap(&mut self.prev, &mut self.cur);
    }

    pub fn reset(&mut self) {
        self.prev.clear();
    }
}

pub fn key_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Colors for one frame.
pub struct Colors {
    pub selection: Rgb,
    pub search: Rgb,
    pub search_current: Rgb,
    pub cursor_text: Rgb,
    pub bold_is_bright: bool,
}

impl Colors {
    pub fn from(c: &Config) -> Colors {
        Colors {
            selection: c.colors.selection.tuple(),
            search: c.colors.search_match.tuple(),
            search_current: c.colors.search_current.tuple(),
            cursor_text: c.colors.cursor_text.tuple(),
            bold_is_bright: c.colors.bold_is_bright,
        }
    }
}

pub fn resolve(c: Color, default: Rgb, term: &Term, bold_bright: bool) -> Rgb {
    match c {
        Color::Default => default,
        Color::Indexed(i) if bold_bright && i < 8 => term.color(i + 8),
        Color::Indexed(i) => term.color(i),
        Color::Rgb(r, g, b) => (r, g, b),
    }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// Foreground and background of a cell after inverse/dim/bold rules.
fn cell_colors(c: &Cell, term: &Term, colors: &Colors) -> (Rgb, Rgb) {
    let (mut dfg, mut dbg) = (term.fg(), term.bg());
    if term.modes.reverse_video {
        std::mem::swap(&mut dfg, &mut dbg);
    }
    let bright = colors.bold_is_bright && c.has(attr::BOLD);
    let mut fg = resolve(c.fg, dfg, term, bright);
    let mut bg = resolve(c.bg, dbg, term, false);
    if c.has(attr::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if c.has(attr::DIM) {
        fg = mix(fg, bg, 0.4);
    }
    if c.has(attr::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

/// Clip a glyph quad vertically to [y0, y1), adjusting its uv.
fn clip_glyph(mut g: GlyphInst, y0: f32, y1: f32) -> Option<GlyphInst> {
    let (top, h) = (g.rect[1], g.rect[3]);
    if top >= y1 || top + h <= y0 || h <= 0.0 {
        return None;
    }
    let (a, b) = (top.max(y0), (top + h).min(y1));
    let (v0, v1) = (g.uv[1], g.uv[3]);
    let t0 = (a - top) / h;
    let t1 = (b - top) / h;
    g.uv[1] = v0 + (v1 - v0) * t0;
    g.uv[3] = v0 + (v1 - v0) * t1;
    g.rect[1] = a;
    g.rect[3] = b - a;
    Some(g)
}

fn clip_rect(mut r: RectInst, y0: f32, y1: f32) -> Option<RectInst> {
    let (top, h) = (r.rect[1], r.rect[3]);
    if top >= y1 || top + h <= y0 {
        return None;
    }
    // Only plain fills can be cut (shaped kinds depend on their size).
    if r.params[1] != KIND_FILL || r.params[0] > 0.0 {
        return Some(r);
    }
    let (a, b) = (top.max(y0), (top + h).min(y1));
    r.rect[1] = a;
    r.rect[3] = b - a;
    Some(r)
}

/// Glyph instance for an atlas entry with the pen at (`x`, baseline `y`).
fn glyph_inst(e: &Entry, x: f32, y: f32, col: [f32; 4]) -> GlyphInst {
    GlyphInst {
        rect: [x + e.left, y - e.top, e.width as f32, e.height as f32],
        uv: e.uv,
        color: col,
        kind: if e.color { 1.0 } else { 0.0 },
    }
}

/// Split a pen x position into a whole pixel and a subpixel phase.
fn phase_of(x: f32) -> (f32, u8) {
    let fx = x.floor();
    let p = ((x - fx) * PHASES).round() as u32;
    if p >= PHASES as u32 { (fx + 1.0, 0) } else { (fx, p as u8) }
}

pub struct PaneView<'a> {
    pub pane: &'a mut Pane,
    /// Content rectangle (cells start here), buffer pixels.
    pub rect: Rect,
    /// Whole pane area including padding (background).
    pub area: Rect,
    pub focused: bool,
    pub window_focused: bool,
    pub cursor_alpha: f32,
    pub hovered_link: u16,
}

/// Build one pane into `scene`, recording its regions in `damage`.
pub fn pane(view: PaneView, cfg: &Config, colors: &Colors, fonts: &mut Fonts, atlas: &mut Atlas<GlyphKey>, scene: &mut Scene, damage: &mut Damage, scale: f32) {
    let cell = fonts.cell;
    let PaneView { pane, rect, area, focused, window_focused, cursor_alpha, hovered_link } = view;
    pane.update_search();
    let term = &pane.term.term;
    let grid = term.grid();
    let (cw, ch) = (cell.w, cell.h as f32);
    let rows = pane.rows;
    let cols = grid.cols.min(pane.cols.max(1));
    let default_bg = if term.modes.reverse_video { term.fg() } else { term.bg() };
    let bg_lin = color(default_bg, 1.0);
    let palette_key = key_of((term.fg(), term.bg(), term.palette.iter().filter_map(|p| *p).collect::<Vec<_>>(), term.modes.reverse_video));

    // Pane background (padding included): one region.
    let area_i = IRect::of(area);
    damage.region(key_of(("pane-bg", pane.id)), area_i, key_of((palette_key, area_i)));
    scene.layers[0].rects.push(RectInst::fill(area.x, area.y, area.w, area.h, bg_lin));

    // Scroll position: top visible line (fractional).
    let offset = if term.alt_active { 0.0 } else { pane.scroll.offset.min(grid.history() as f64) };
    let top = grid.history() as f64 - offset;
    let first = top.floor();
    let frac = (top - first) as f32;
    let first = first as i64;
    let y_clip0 = rect.y;
    let y_clip1 = rect.y + rows as f32 * ch;
    let sel = pane.selection;
    let search = pane.search.as_ref();
    let cur = term.cursor;
    // Cursor screen row in this view.
    let cursor_visible = term.modes.show_cursor && cursor_alpha > 0.0;
    let cursor_row = cur.y as i64 + grid.history() as i64;
    let anim_x = pane.cursor_x as f32;
    let anim_y = pane.cursor_y as f32 + (grid.history() as f64 - top) as f32;
    let cursor_style = match term.cursor_shape {
        CursorShape::Block if cfg.cursor.style != CursorStyle::Block && !term.alt_active => match cfg.cursor.style {
            CursorStyle::Bar => CursorShape::Bar,
            CursorStyle::Underline => CursorShape::Underline,
            CursorStyle::Block => CursorShape::Block,
        },
        s => s,
    };
    let cursor_rgb = term.cursor_color();
    let block_text = cursor_visible && focused && window_focused && cursor_style == CursorShape::Block && cursor_alpha > 0.5;

    let extra_rows = if frac > 0.0 { 1 } else { 0 };
    for r in 0..(rows + extra_rows) {
        let line = first + r as i64;
        let y = rect.y + (r as f32 - frac) * ch;
        let row_rect = IRect::of(Rect::new(rect.x, y.max(y_clip0), cols as f32 * cw + 1.0, (y + ch).min(y_clip1) - y.max(y_clip0)));
        if row_rect.h <= 0 {
            continue;
        }
        let Some(row) = (line >= 0).then(|| grid.line(line as usize)).flatten() else {
            damage.region(key_of(("row", pane.id, r)), row_rect, key_of(("empty", palette_key)));
            continue;
        };
        let abs = grid.dropped + line as u64;
        let sel_cols = sel.and_then(|s| s.cols_on(grid, abs));
        let mut matches: Vec<(usize, usize, bool)> = Vec::new();
        if let Some(s) = search {
            for (i, (a, b)) in s.matches.iter().enumerate() {
                if a.line <= abs && b.line >= abs {
                    let from = if a.line == abs { a.col } else { 0 };
                    let to = if b.line == abs { b.col } else { cols - 1 };
                    matches.push((from, to, Some(i) == s.current));
                }
            }
        }
        let has_cursor = line == cursor_row && cursor_visible;
        let cursor_key = has_cursor.then(|| {
            (
                (anim_x * 64.0) as i32,
                (anim_y * 64.0) as i32,
                (cursor_alpha * 32.0) as i32,
                cursor_style as u8,
                focused && window_focused,
                cur.x,
            )
        });
        // The animated cursor may sit between rows: both rows carry it.
        let near_cursor = cursor_visible && ((anim_y + first as f32 - (line as f32)).abs() < 1.0 || has_cursor);
        let hash = key_of((
            &row.cells[..cols.min(row.cells.len())],
            &row.extras,
            sel_cols,
            &matches,
            cursor_key,
            near_cursor.then(|| ((anim_x * 64.0) as i32, (anim_y * 64.0) as i32, (cursor_alpha * 32.0) as i32)),
            (y * 64.0) as i32,
            palette_key,
            hovered_link,
            (focused, window_focused),
        ));
        damage.region(key_of(("row", pane.id, r)), row_rect, hash);

        // Backgrounds: runs of equal color.
        let x_of = |c: usize| (rect.x + c as f32 * cw).round();
        let mut run: Option<(usize, Rgb)> = None;
        let mut bg_rects: Vec<RectInst> = Vec::new();
        let flush = |run: &mut Option<(usize, Rgb)>, end: usize, out: &mut Vec<RectInst>| {
            if let Some((start, c)) = run.take() {
                if c != default_bg {
                    let x0 = x_of(start);
                    out.push(RectInst::fill(x0, y, x_of(end) - x0, ch, color(c, 1.0)));
                }
            }
        };
        for x in 0..cols {
            let c = &row.cells[x];
            let (_, mut bg) = cell_colors(c, term, colors);
            if let Some(&(_, _, cur_m)) = matches.iter().find(|(a, b, _)| x >= *a && x <= *b) {
                bg = if cur_m { colors.search_current } else { colors.search };
            }
            if sel_cols.is_some_and(|(a, b)| x >= a && x <= b) {
                bg = colors.selection;
            }
            match run {
                Some((_, rc)) if rc == bg => {}
                _ => {
                    flush(&mut run, x, &mut bg_rects);
                    run = Some((x, bg));
                }
            }
        }
        flush(&mut run, cols, &mut bg_rects);
        for b in bg_rects {
            if let Some(b) = clip_rect(b, y_clip0, y_clip1) {
                scene.layers[0].rects.push(b);
            }
        }

        // Glyphs and decorations.
        let baseline = y + cell.baseline;
        let mut x = 0;
        while x < cols {
            let c = row.cells[x];
            if c.has(attr::SPACER) {
                x += 1;
                continue;
            }
            let wide = c.has(attr::WIDE);
            let (mut fg, _) = cell_colors(&c, term, colors);
            let is_cursor_cell = block_text && line == cursor_row && x == cur.x && (anim_x - cur.x as f32).abs() < 0.3 && (pane.cursor_y as f32 - cur.y as f32).abs() < 0.3;
            if is_cursor_cell {
                fg = colors.cursor_text;
            }
            let fgc = color(fg, 1.0);
            let pen = rect.x + x as f32 * cw;
            if c.ch != ' ' && !c.has(attr::HIDDEN) {
                if boxdraw::handles(c.ch) && !c.has(attr::EXTRA) {
                    let x0 = x_of(x);
                    let x1 = x_of(x + if wide { 2 } else { 1 });
                    let (w, h) = ((x1 - x0) as u32, cell.h);
                    let key = GlyphKey::Symbol { c: c.ch, w: w as u16, h: h as u16 };
                    if let Some(e) = atlas.get(&key, || fonts.symbol(c.ch, w, h)) {
                        let g = GlyphInst { rect: [x0, y, e.width as f32, e.height as f32], uv: e.uv, color: fgc, kind: 0.0 };
                        if let Some(g) = clip_glyph(g, y_clip0, y_clip1) {
                            scene.layers[1].glyphs.push(g);
                        }
                    }
                } else {
                    let style = if c.has(attr::BOLD) { BOLD } else { 0 } | if c.has(attr::ITALIC) { ITALIC } else { 0 };
                    let placed = if c.has(attr::EXTRA) {
                        let mut s = String::new();
                        s.push(c.ch);
                        if let Some(e) = row.extra(x) {
                            s.push_str(e);
                        }
                        fonts.lookup(&s, style)
                    } else {
                        match fonts.single(c.ch, style) {
                            Some(p) => std::rc::Rc::from([p].as_slice()),
                            None => std::rc::Rc::from([].as_slice()),
                        }
                    };
                    let cells = if wide { 2 } else { 1 };
                    if placed.is_empty() && !c.ch.is_whitespace() {
                        let x0 = x_of(x);
                        let x1 = x_of(x + cells as usize);
                        let (w, h) = ((x1 - x0) as u32, cell.h);
                        let key = GlyphKey::Symbol { c: TOFU, w: w as u16, h: h as u16 };
                        if let Some(e) = atlas.get(&key, || fonts.symbol(TOFU, w, h)) {
                            let g = GlyphInst { rect: [x0, y, e.width as f32, e.height as f32], uv: e.uv, color: fgc, kind: 0.0 };
                            if let Some(g) = clip_glyph(g, y_clip0, y_clip1) {
                                scene.layers[1].glyphs.push(g);
                            }
                        }
                    }
                    for p in placed.iter() {
                        if p.color {
                            let key = GlyphKey::Font { style: p.style, font: p.font, glyph: p.glyph, phase: cells as u8 * 8 };
                            if let Some(e) = atlas.get(&key, || fonts.raster(p, 0, cells)) {
                                let g = GlyphInst { rect: [x_of(x) + e.left, baseline - e.top, e.width as f32, e.height as f32], uv: e.uv, color: [1.0, 1.0, 1.0, 1.0], kind: 1.0 };
                                if let Some(g) = clip_glyph(g, y_clip0, y_clip1) {
                                    scene.layers[1].glyphs.push(g);
                                }
                            }
                            continue;
                        }
                        let (px, phase) = phase_of(pen + p.dx);
                        let key = GlyphKey::Font { style: p.style, font: p.font, glyph: p.glyph, phase };
                        if let Some(e) = atlas.get(&key, || fonts.raster(p, phase, cells)) {
                            let g = glyph_inst(&e, px, baseline - p.dy, fgc);
                            if let Some(g) = clip_glyph(g, y_clip0, y_clip1) {
                                scene.layers[1].glyphs.push(g);
                            }
                        }
                    }
                }
            }
            // Decorations.
            let w = if wide { 2.0 } else { 1.0 } * cw;
            let thick = cell.underline_thick.max(1.0);
            let link_hover = c.link != 0 && c.link == hovered_link;
            let underline = if link_hover && c.underline == Underline::None { Underline::Single } else { c.underline };
            if underline != Underline::None {
                let ulc = color(resolve(c.ul_color, fg, term, false), 1.0);
                let uy = y + cell.underline_pos;
                let r = match underline {
                    Underline::Double => {
                        let gap = thick.max(1.0);
                        scene.layers[2].rects.push(RectInst::fill(pen.round(), (uy - gap).round() - thick, w.round(), thick, ulc));
                        RectInst::fill(pen.round(), (uy + gap).round() - thick + thick, w.round(), thick, ulc)
                    }
                    Underline::Curly => {
                        let h = (thick * 3.0).max(3.0);
                        RectInst { rect: [pen.round(), uy - h / 2.0, w.round(), h], color: ulc, params: [0.0, KIND_CURL, thick, cw] }
                    }
                    Underline::Dotted => RectInst { rect: [pen.round(), uy, w.round(), thick], color: ulc, params: [0.0, KIND_DOTTED, 0.0, (thick * 2.0).max(2.0)] },
                    Underline::Dashed => RectInst { rect: [pen.round(), uy, w.round(), thick], color: ulc, params: [0.0, KIND_DASHED, 0.0, cw] },
                    _ => RectInst::fill(pen.round(), uy, w.round(), thick, ulc),
                };
                if let Some(r) = clip_rect(r, y_clip0, y_clip1) {
                    scene.layers[2].rects.push(r);
                }
            }
            if c.has(attr::STRIKE) {
                if let Some(r) = clip_rect(RectInst::fill(pen.round(), y + cell.strike_pos, w.round(), thick, fgc), y_clip0, y_clip1) {
                    scene.layers[2].rects.push(r);
                }
            }
            if c.has(attr::OVERLINE) {
                if let Some(r) = clip_rect(RectInst::fill(pen.round(), y, w.round(), thick, fgc), y_clip0, y_clip1) {
                    scene.layers[2].rects.push(r);
                }
            }
            x += if wide { 2 } else { 1 };
        }
    }

    // Cursor.
    if cursor_visible && anim_y > -1.0 && anim_y < rows as f32 {
        let x0 = rect.x + anim_x * cw;
        let y0 = rect.y + anim_y * ch;
        let wide = grid.row(cur.y).cells.get(cur.x).is_some_and(|c| c.has(attr::WIDE));
        let w = if wide { 2.0 } else { 1.0 } * cw;
        let col = color(cursor_rgb, cursor_alpha);
        let active = focused && window_focused;
        let line = (cell.line as f32).max(scale.round()).max(1.0);
        let r = if !active {
            RectInst { rect: [x0.round(), y0.round(), w.round(), ch], color: color(cursor_rgb, 0.9), params: [0.0, KIND_OUTLINE, line, 0.0] }
        } else {
            match cursor_style {
                CursorShape::Block => RectInst::rounded(x0, y0, w, ch, (1.5 * scale).min(3.0), col),
                CursorShape::Bar => RectInst::fill(x0.round(), y0, (2.0 * scale).round().max(1.0), ch, col),
                CursorShape::Underline => RectInst::fill(x0, y0 + ch - (2.0 * scale).round().max(1.0), w, (2.0 * scale).round().max(1.0), col),
            }
        };
        if let Some(r) = clip_rect(r, y_clip0, y_clip1) {
            // A filled block goes under the text, other shapes over it.
            if active && cursor_style == CursorShape::Block {
                scene.layers[0].rects.push(r);
            } else {
                scene.layers[2].rects.push(r);
            }
        }
    }
}

/// Text in the UI font with its baseline at `y`, left edge at `x`; returns
/// the advance. `max_w` ellipsizes (cuts) longer text.
pub fn ui_text(fonts: &mut Fonts, atlas: &mut Atlas<GlyphKey>, scene: &mut Scene, layer: usize, text: &str, x: f32, y: f32, col: [f32; 4], max_w: f32) -> f32 {
    let (glyphs, width) = fonts.ui_line(text);
    let (glyphs, width) = if width > max_w {
        // Cut to fit with an ellipsis.
        let (dots, dw) = fonts.ui_line("…");
        let keep: Vec<_> = glyphs.into_iter().filter(|g| g.2 + dw < max_w - 1.0).collect();
        let end = keep.last().map(|g| g.2).unwrap_or(0.0);
        let mut v = keep;
        // Place the ellipsis after the last kept glyph's advance (approximate).
        let shift = (end + (max_w - dw - end).min(fonts.ui_px * 0.6)).min(max_w - dw);
        v.extend(dots.into_iter().map(|g| (g.0, g.1, g.2 + shift, g.3)));
        (v, max_w)
    } else {
        (glyphs, width)
    };
    for (font, glyph, gx, gy) in glyphs {
        let (px, phase) = phase_of(x + gx);
        let key = GlyphKey::Ui { font, glyph, phase };
        if let Some(e) = atlas.get(&key, || fonts.ui_raster(font, glyph, phase)) {
            let c = if e.color { [1.0, 1.0, 1.0, col[3]] } else { col };
            scene.layers[layer].glyphs.push(glyph_inst(&e, px, y - gy, c));
        }
    }
    width
}

pub fn ui_width(fonts: &mut Fonts, text: &str) -> f32 {
    fonts.ui_line(text).1
}

/// Cell under a point of a pane's content (clamped to the grid).
pub fn cell_at(rect: Rect, cell: CellMetrics, cols: usize, rows: usize, px: f32, py: f32) -> (usize, usize) {
    let x = ((px - rect.x) / cell.w).floor().clamp(0.0, cols.saturating_sub(1) as f32) as usize;
    let y = ((py - rect.y) / cell.h as f32).floor().clamp(0.0, rows.saturating_sub(1) as f32) as usize;
    (x, y)
}

pub fn now() -> Instant {
    Instant::now()
}
