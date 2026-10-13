//! Symbolic icons: an original line-style set drawn from vector paths on a
//! 24-unit grid (2-unit strokes, round caps and joins), rasterized at the exact
//! pixel size with FreeType's outline renderer and tinted when drawn (like
//! text, they live in the glyph atlas as coverage masks).

use std::f32::consts::PI;
use std::rc::Rc;

use lumen_text::{Library, Path};

macro_rules! icons {
    ($($name:ident => $label:literal,)*) => {
        /// The icon set. Use with [`crate::Ui::icon`], [`crate::widgets::Button::icon`] etc.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum Icon { $($name,)* }

        impl Icon {
            /// Every icon, in declaration order (the gallery shows them all).
            pub const ALL: &'static [Icon] = &[$(Icon::$name,)*];

            /// Kebab-case name ("folder", "chevron-left").
            pub fn name(self) -> &'static str {
                match self { $(Icon::$name => $label,)* }
            }

            pub fn from_name(name: &str) -> Option<Icon> {
                match name { $($label => Some(Icon::$name),)* _ => None }
            }
        }
    };
}

icons! {
    Folder => "folder",
    FolderOpen => "folder-open",
    File => "file",
    FileText => "file-text",
    FileCode => "file-code",
    Image => "image",
    Video => "video",
    Audio => "audio",
    Back => "back",
    Forward => "forward",
    Up => "up",
    Down => "down",
    Home => "home",
    Search => "search",
    Close => "close",
    Plus => "plus",
    Minus => "minus",
    Trash => "trash",
    Edit => "edit",
    Copy => "copy",
    Paste => "paste",
    Cut => "cut",
    Save => "save",
    Open => "open",
    Settings => "settings",
    Refresh => "refresh",
    Check => "check",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronUp => "chevron-up",
    ChevronDown => "chevron-down",
    Menu => "menu",
    MoreVertical => "more-vertical",
    MoreHorizontal => "more-horizontal",
    Grid => "grid",
    List => "list",
    Star => "star",
    StarFilled => "star-filled",
    Info => "info",
    Warning => "warning",
    Error => "error",
    Question => "question",
    Keyboard => "keyboard",
    Network => "network",
    Wifi => "wifi",
    Volume => "volume",
    VolumeLow => "volume-low",
    VolumeMute => "volume-mute",
    Display => "display",
    Power => "power",
    Play => "play",
    Pause => "pause",
    Stop => "stop",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
    Fullscreen => "fullscreen",
    FullscreenExit => "fullscreen-exit",
    RotateLeft => "rotate-left",
    RotateRight => "rotate-right",
    Sun => "sun",
    Moon => "moon",
    Lock => "lock",
    User => "user",
    Clock => "clock",
    Calendar => "calendar",
    Download => "download",
    Upload => "upload",
    ExternalLink => "external-link",
    Eye => "eye",
    EyeOff => "eye-off",
    Filter => "filter",
    Sort => "sort",
    Bookmark => "bookmark",
    Tag => "tag",
    Terminal => "terminal",
    Calculator => "calculator",
    Cpu => "cpu",
    Activity => "activity",
    HardDrive => "hard-drive",
    Undo => "undo",
    Redo => "redo",
    Sidebar => "sidebar",
    Bell => "bell",
    Battery => "battery",
    Palette => "palette",
    Backspace => "backspace",
    Maximize => "maximize",
    Restore => "restore",
}

/// Path commands in grid units (24 × 24, y down). Angles are degrees,
/// clockwise from 3 o'clock.
#[derive(Clone, Copy)]
enum C {
    M(f32, f32),
    L(f32, f32),
    Q(f32, f32, f32, f32),
    /// Arc of the circle at (cx, cy) with radius r from angle a0 to a1,
    /// continuing the current contour (or starting one).
    A(f32, f32, f32, f32, f32),
    Z,
    Circle(f32, f32, f32),
    Ellipse(f32, f32, f32, f32),
    RRect(f32, f32, f32, f32, f32),
}

use C::*;

/// Coverage accumulated from strokes and fills.
struct Pen<'a> {
    lib: &'a Rc<Library>,
    px: u32,
    k: f32,
    width: f32,
    mask: Vec<u8>,
}

impl Pen<'_> {
    fn path(&self, cmds: &[C]) -> Path {
        let k = self.k;
        let mut p = Path::new();
        let mut open = false;
        for c in cmds {
            match *c {
                M(x, y) => {
                    p.move_to(x * k, y * k);
                    open = true;
                }
                L(x, y) => {
                    p.line_to(x * k, y * k);
                }
                Q(cx, cy, x, y) => {
                    p.quad_to(cx * k, cy * k, x * k, y * k);
                }
                A(cx, cy, r, a0, a1) => {
                    let (a0, a1) = (a0.to_radians(), a1.to_radians());
                    let start = (cx + r * a0.cos(), cy + r * a0.sin());
                    if open {
                        p.line_to(start.0 * k, start.1 * k);
                    } else {
                        p.move_to(start.0 * k, start.1 * k);
                        open = true;
                    }
                    // Cubic segments of at most 90°.
                    let n = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
                    let step = (a1 - a0) / n as f32;
                    let h = 4.0 / 3.0 * (step / 4.0).tan();
                    for i in 0..n {
                        let (s0, s1) = (a0 + step * i as f32, a0 + step * (i + 1) as f32);
                        let (c0, n0) = (s0.cos(), s0.sin());
                        let (c1, n1) = (s1.cos(), s1.sin());
                        p.cubic_to(
                            (cx + r * (c0 - h * n0)) * k,
                            (cy + r * (n0 + h * c0)) * k,
                            (cx + r * (c1 + h * n1)) * k,
                            (cy + r * (n1 - h * c1)) * k,
                            (cx + r * c1) * k,
                            (cy + r * n1) * k,
                        );
                    }
                }
                Z => {
                    p.close();
                    open = false;
                }
                Circle(cx, cy, r) => {
                    p.circle(cx * k, cy * k, r * k, false);
                    open = false;
                }
                Ellipse(cx, cy, rx, ry) => {
                    let (kx, ky) = (rx * 0.552_284_8, ry * 0.552_284_8);
                    p.move_to((cx + rx) * k, cy * k)
                        .cubic_to((cx + rx) * k, (cy + ky) * k, (cx + kx) * k, (cy + ry) * k, cx * k, (cy + ry) * k)
                        .cubic_to((cx - kx) * k, (cy + ry) * k, (cx - rx) * k, (cy + ky) * k, (cx - rx) * k, cy * k)
                        .cubic_to((cx - rx) * k, (cy - ky) * k, (cx - kx) * k, (cy - ry) * k, cx * k, (cy - ry) * k)
                        .cubic_to((cx + kx) * k, (cy - ry) * k, (cx + rx) * k, (cy - ky) * k, (cx + rx) * k, cy * k)
                        .close();
                    open = false;
                }
                RRect(x, y, w, h, r) => {
                    p.rounded_rect(x * k, y * k, w * k, h * k, r * k, false);
                    open = false;
                }
            }
        }
        p
    }

    fn add(&mut self, m: Vec<u8>) {
        for (a, b) in self.mask.iter_mut().zip(m) {
            *a = (*a).max(b);
        }
    }

    /// Stroke open contours (round caps).
    fn line(&mut self, cmds: &[C]) {
        let m = self.path(cmds).stroke(self.lib, self.px, self.px, self.width, false);
        self.add(m);
    }

    /// Stroke closed contours.
    fn outline(&mut self, cmds: &[C]) {
        let m = self.path(cmds).stroke(self.lib, self.px, self.px, self.width, true);
        self.add(m);
    }

    fn fill(&mut self, cmds: &[C]) {
        let m = self.path(cmds).rasterize(self.lib, self.px, self.px, false);
        self.add(m);
    }

    /// Remove the area of `cmds` from what was drawn so far.
    fn cut(&mut self, cmds: &[C]) {
        let m = self.path(cmds).rasterize(self.lib, self.px, self.px, false);
        for (a, b) in self.mask.iter_mut().zip(m) {
            *a = ((*a as u32 * (255 - b as u32)) / 255) as u8;
        }
    }

    fn dot(&mut self, x: f32, y: f32, r: f32) {
        self.fill(&[Circle(x, y, r)]);
    }
}

/// A closed star outline (`points` tips).
fn star(cx: f32, cy: f32, outer: f32, inner: f32, points: usize) -> Vec<C> {
    let mut v = Vec::new();
    for i in 0..points * 2 {
        let a = -PI / 2.0 + i as f32 * PI / points as f32;
        let r = if i % 2 == 0 { outer } else { inner };
        let p = (cx + r * a.cos(), cy + r * a.sin());
        v.push(if i == 0 { M(p.0, p.1) } else { L(p.0, p.1) });
    }
    v.push(Z);
    v
}

/// A gear outline with `teeth` teeth.
fn gear(cx: f32, cy: f32, outer: f32, root: f32, teeth: usize) -> Vec<C> {
    let mut v = Vec::new();
    let period = 2.0 * PI / teeth as f32;
    let pt = |r: f32, a: f32| (cx + r * a.cos(), cy + r * a.sin());
    for i in 0..teeth {
        let a = i as f32 * period;
        // Valley, flank up, flat top, flank down.
        let pts = [pt(root, a - period * 0.5), pt(root, a - period * 0.30), pt(outer, a - period * 0.17), pt(outer, a + period * 0.17), pt(root, a + period * 0.30)];
        for (j, p) in pts.iter().enumerate() {
            v.push(if i == 0 && j == 0 { M(p.0, p.1) } else { L(p.0, p.1) });
        }
    }
    v.push(Z);
    v
}

/// A circular arrow around (cx, cy) from `a0` to `a1` degrees with an arrow
/// head at the end pointing along the direction of travel.
fn circular_arrow(p: &mut Pen, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    p.line(&[A(cx, cy, r, a0, a1)]);
    let a = a1.to_radians();
    let dir = if a1 > a0 { 1.0 } else { -1.0 };
    let tip = (cx + r * a.cos(), cy + r * a.sin());
    // Tangent in the direction of travel and the outward normal.
    let t = (-a.sin() * dir, a.cos() * dir);
    let n = (a.cos(), a.sin());
    let s = 3.0;
    p.line(&[M(tip.0 - t.0 * s + n.0 * s, tip.1 - t.1 * s + n.1 * s), L(tip.0, tip.1), L(tip.0 - t.0 * s - n.0 * s, tip.1 - t.1 * s - n.1 * s)]);
}

fn speaker(p: &mut Pen) {
    p.outline(&[M(3.5, 9.5), L(7.5, 9.5), L(12.0, 5.5), L(12.0, 18.5), L(7.5, 14.5), L(3.5, 14.5), Z]);
}

fn magnifier(p: &mut Pen) {
    p.outline(&[Circle(10.5, 10.5, 6.5)]);
    p.line(&[M(15.5, 15.5), L(20.0, 20.0)]);
}

const FILE: [C; 10] = [M(6.5, 3.0), L(13.0, 3.0), L(19.0, 9.0), L(19.0, 19.5), Q(19.0, 21.0, 17.5, 21.0), L(6.5, 21.0), Q(5.0, 21.0, 5.0, 19.5), L(5.0, 4.5), Q(5.0, 3.0, 6.5, 3.0), Z];

fn file(p: &mut Pen) {
    p.outline(&FILE);
    p.line(&[M(13.0, 3.0), L(13.0, 7.5), Q(13.0, 9.0, 14.5, 9.0), L(19.0, 9.0)]);
}

/// Coverage mask (`px` × `px`, one byte per pixel) of `icon`.
pub fn rasterize(lib: &Rc<Library>, icon: Icon, px: u32) -> Vec<u8> {
    let px = px.max(1);
    let k = px as f32 / 24.0;
    // 2 grid units, but never thinner than 1.5 px (small sizes stay legible).
    let width = (2.0 * k).max(1.5);
    let mut p = Pen { lib, px, k, width, mask: vec![0u8; (px * px) as usize] };
    match icon {
        Icon::Folder => {
            p.outline(&[M(3.0, 6.5), Q(3.0, 5.0, 4.5, 5.0), L(9.0, 5.0), L(11.0, 7.0), L(19.5, 7.0), Q(21.0, 7.0, 21.0, 8.5), L(21.0, 17.5), Q(21.0, 19.0, 19.5, 19.0), L(4.5, 19.0), Q(3.0, 19.0, 3.0, 17.5), Z]);
        }
        Icon::FolderOpen | Icon::Open => {
            p.line(&[M(4.5, 19.0), Q(3.0, 19.0, 3.0, 17.5), L(3.0, 6.5), Q(3.0, 5.0, 4.5, 5.0), L(9.0, 5.0), L(11.0, 7.0), L(17.0, 7.0), Q(18.5, 7.0, 18.5, 8.5), L(18.5, 10.0)]);
            p.outline(&[M(7.0, 10.0), L(20.5, 10.0), Q(22.0, 10.0, 21.5, 11.4), L(19.2, 17.8), Q(18.8, 19.0, 17.5, 19.0), L(4.5, 19.0), L(5.7, 11.2), Q(5.9, 10.0, 7.0, 10.0), Z]);
            if icon == Icon::Open {
                p.cut(&[RRect(10.0, 12.0, 6.0, 5.0, 1.0)]);
            }
        }
        Icon::File => file(&mut p),
        Icon::FileText => {
            file(&mut p);
            p.line(&[M(8.5, 13.0), L(15.5, 13.0), M(8.5, 16.5), L(15.5, 16.5)]);
            p.line(&[M(8.5, 9.5), L(10.0, 9.5)]);
        }
        Icon::FileCode => {
            file(&mut p);
            p.line(&[M(10.0, 12.0), L(8.0, 14.5), L(10.0, 17.0)]);
            p.line(&[M(14.0, 12.0), L(16.0, 14.5), L(14.0, 17.0)]);
        }
        Icon::Image => {
            p.outline(&[RRect(3.0, 4.0, 18.0, 16.0, 2.5)]);
            p.dot(8.5, 9.5, 1.9);
            p.line(&[M(4.0, 18.5), L(10.0, 12.5), L(14.0, 16.0), L(16.5, 13.5), L(20.0, 17.0)]);
        }
        Icon::Video => {
            p.outline(&[RRect(3.0, 5.0, 18.0, 14.0, 2.5)]);
            p.fill(&[M(10.0, 9.0), L(15.0, 12.0), L(10.0, 15.0), Z]);
            p.outline(&[M(10.0, 9.0), L(15.0, 12.0), L(10.0, 15.0), Z]);
        }
        Icon::Audio => {
            p.line(&[M(9.5, 17.0), L(9.5, 6.0), L(19.0, 4.0), L(19.0, 15.0)]);
            p.dot(7.0, 17.2, 2.7);
            p.dot(16.5, 15.2, 2.7);
        }
        Icon::Back => p.line(&[M(19.0, 12.0), L(5.0, 12.0), M(11.0, 6.0), L(5.0, 12.0), L(11.0, 18.0)]),
        Icon::Forward => p.line(&[M(5.0, 12.0), L(19.0, 12.0), M(13.0, 6.0), L(19.0, 12.0), L(13.0, 18.0)]),
        Icon::Up => p.line(&[M(12.0, 19.0), L(12.0, 5.0), M(6.0, 11.0), L(12.0, 5.0), L(18.0, 11.0)]),
        Icon::Down => p.line(&[M(12.0, 5.0), L(12.0, 19.0), M(6.0, 13.0), L(12.0, 19.0), L(18.0, 13.0)]),
        Icon::Home => {
            p.line(&[M(3.5, 11.0), L(12.0, 4.0), L(20.5, 11.0)]);
            p.line(&[M(6.0, 9.5), L(6.0, 19.0), Q(6.0, 20.0, 7.0, 20.0), L(17.0, 20.0), Q(18.0, 20.0, 18.0, 19.0), L(18.0, 9.5)]);
            p.line(&[M(10.0, 20.0), L(10.0, 15.0), L(14.0, 15.0), L(14.0, 20.0)]);
        }
        Icon::Search => magnifier(&mut p),
        Icon::Close => p.line(&[M(6.0, 6.0), L(18.0, 18.0), M(18.0, 6.0), L(6.0, 18.0)]),
        Icon::Plus => p.line(&[M(12.0, 5.0), L(12.0, 19.0), M(5.0, 12.0), L(19.0, 12.0)]),
        Icon::Minus => p.line(&[M(5.0, 12.0), L(19.0, 12.0)]),
        Icon::Trash => {
            p.line(&[M(4.0, 6.5), L(20.0, 6.5)]);
            p.line(&[M(9.0, 6.5), L(9.0, 4.5), Q(9.0, 3.5, 10.0, 3.5), L(14.0, 3.5), Q(15.0, 3.5, 15.0, 4.5), L(15.0, 6.5)]);
            p.line(&[M(6.0, 6.5), L(7.0, 19.0), Q(7.1, 20.5, 8.5, 20.5), L(15.5, 20.5), Q(16.9, 20.5, 17.0, 19.0), L(18.0, 6.5)]);
            p.line(&[M(10.0, 10.5), L(10.0, 16.5), M(14.0, 10.5), L(14.0, 16.5)]);
        }
        Icon::Edit => {
            p.outline(&[M(15.0, 4.5), L(19.5, 9.0), L(9.0, 19.5), L(4.0, 20.0), L(4.5, 15.0), Z]);
            p.line(&[M(12.5, 7.0), L(17.0, 11.5)]);
        }
        Icon::Copy => {
            p.outline(&[RRect(8.0, 8.0, 13.0, 13.0, 2.0)]);
            p.line(&[M(16.0, 8.0), L(16.0, 5.0), Q(16.0, 3.0, 14.0, 3.0), L(5.0, 3.0), Q(3.0, 3.0, 3.0, 5.0), L(3.0, 14.0), Q(3.0, 16.0, 5.0, 16.0), L(8.0, 16.0)]);
        }
        Icon::Paste => {
            p.line(&[M(8.0, 5.0), L(6.5, 5.0), Q(5.0, 5.0, 5.0, 6.5), L(5.0, 19.5), Q(5.0, 21.0, 6.5, 21.0), L(17.5, 21.0), Q(19.0, 21.0, 19.0, 19.5), L(19.0, 6.5), Q(19.0, 5.0, 17.5, 5.0), L(16.0, 5.0)]);
            p.outline(&[RRect(8.0, 3.0, 8.0, 4.0, 1.2)]);
        }
        Icon::Cut => {
            p.outline(&[Circle(6.5, 17.5, 2.8), Circle(17.5, 17.5, 2.8)]);
            p.line(&[M(8.6, 15.4), L(18.0, 4.0), M(15.4, 15.4), L(6.0, 4.0)]);
        }
        Icon::Save => {
            p.outline(&[M(5.0, 3.5), L(16.0, 3.5), L(20.5, 8.0), L(20.5, 19.0), Q(20.5, 20.5, 19.0, 20.5), L(5.0, 20.5), Q(3.5, 20.5, 3.5, 19.0), L(3.5, 5.0), Q(3.5, 3.5, 5.0, 3.5), Z]);
            p.line(&[M(7.5, 20.5), L(7.5, 14.0), L(16.5, 14.0), L(16.5, 20.5)]);
            p.line(&[M(7.5, 3.5), L(7.5, 7.5), L(14.0, 7.5)]);
        }
        Icon::Settings => {
            p.outline(&gear(12.0, 12.0, 9.6, 7.2, 8));
            p.outline(&[Circle(12.0, 12.0, 3.0)]);
        }
        Icon::Refresh => circular_arrow(&mut p, 12.0, 12.0, 7.5, 30.0, 320.0),
        Icon::RotateRight => circular_arrow(&mut p, 12.0, 13.0, 7.0, 90.0, 290.0),
        Icon::RotateLeft => circular_arrow(&mut p, 12.0, 13.0, 7.0, 90.0, -110.0),
        Icon::Check => p.line(&[M(5.0, 12.5), L(10.0, 17.5), L(19.5, 7.0)]),
        Icon::ChevronLeft => p.line(&[M(15.0, 5.5), L(8.5, 12.0), L(15.0, 18.5)]),
        Icon::ChevronRight => p.line(&[M(9.0, 5.5), L(15.5, 12.0), L(9.0, 18.5)]),
        Icon::ChevronUp => p.line(&[M(5.5, 15.0), L(12.0, 8.5), L(18.5, 15.0)]),
        Icon::ChevronDown => p.line(&[M(5.5, 9.0), L(12.0, 15.5), L(18.5, 9.0)]),
        Icon::Menu => p.line(&[M(4.0, 6.5), L(20.0, 6.5), M(4.0, 12.0), L(20.0, 12.0), M(4.0, 17.5), L(20.0, 17.5)]),
        Icon::MoreVertical => {
            for y in [5.5, 12.0, 18.5] {
                p.dot(12.0, y, 2.0);
            }
        }
        Icon::MoreHorizontal => {
            for x in [5.5, 12.0, 18.5] {
                p.dot(x, 12.0, 2.0);
            }
        }
        Icon::Grid => {
            p.outline(&[RRect(4.0, 4.0, 7.0, 7.0, 1.5), RRect(13.0, 4.0, 7.0, 7.0, 1.5), RRect(4.0, 13.0, 7.0, 7.0, 1.5), RRect(13.0, 13.0, 7.0, 7.0, 1.5)]);
        }
        Icon::List => {
            for y in [6.5, 12.0, 17.5] {
                p.dot(4.5, y, 1.5);
                p.line(&[M(9.0, y), L(20.0, y)]);
            }
        }
        Icon::Star => p.outline(&star(12.0, 12.8, 9.5, 4.3, 5)),
        Icon::StarFilled => {
            let s = star(12.0, 12.8, 9.5, 4.3, 5);
            p.fill(&s);
            p.outline(&s);
        }
        Icon::Info => {
            p.outline(&[Circle(12.0, 12.0, 9.0)]);
            p.line(&[M(12.0, 11.0), L(12.0, 16.5)]);
            p.dot(12.0, 7.8, 1.4);
        }
        Icon::Warning => {
            p.outline(&[M(12.0, 3.5), L(21.5, 19.5), L(2.5, 19.5), Z]);
            p.line(&[M(12.0, 9.5), L(12.0, 13.5)]);
            p.dot(12.0, 16.6, 1.35);
        }
        Icon::Error => {
            p.outline(&[Circle(12.0, 12.0, 9.0)]);
            p.line(&[M(9.0, 9.0), L(15.0, 15.0), M(15.0, 9.0), L(9.0, 15.0)]);
        }
        Icon::Question => {
            p.outline(&[Circle(12.0, 12.0, 9.0)]);
            p.line(&[M(9.5, 9.5), Q(9.5, 6.9, 12.0, 6.9), Q(14.5, 6.9, 14.5, 9.3), Q(14.5, 11.0, 12.8, 11.8), Q(12.0, 12.3, 12.0, 13.5)]);
            p.dot(12.0, 16.8, 1.3);
        }
        Icon::Keyboard => {
            p.outline(&[RRect(2.5, 6.0, 19.0, 12.0, 2.0)]);
            for x in [6.0, 9.33, 12.67, 16.0] {
                p.dot(x + 0.5, 9.5, 1.15);
            }
            for x in [7.5, 11.0, 14.5] {
                p.dot(x + 0.5, 12.5, 1.15);
            }
            p.line(&[M(8.0, 15.0), L(16.0, 15.0)]);
        }
        Icon::Network => {
            p.outline(&[Circle(12.0, 12.0, 9.0), Ellipse(12.0, 12.0, 4.0, 9.0)]);
            p.line(&[M(3.0, 12.0), L(21.0, 12.0)]);
        }
        Icon::Wifi => {
            for r in [5.0, 9.5] {
                p.line(&[A(12.0, 19.0, r, 225.0, 315.0)]);
            }
            p.line(&[A(12.0, 19.0, 14.0, 228.0, 312.0)]);
            p.dot(12.0, 19.0, 1.6);
        }
        Icon::Volume => {
            speaker(&mut p);
            p.line(&[A(12.0, 12.0, 4.0, -45.0, 45.0)]);
            p.line(&[A(12.0, 12.0, 8.0, -50.0, 50.0)]);
        }
        Icon::VolumeLow => {
            speaker(&mut p);
            p.line(&[A(12.0, 12.0, 4.0, -45.0, 45.0)]);
        }
        Icon::VolumeMute => {
            speaker(&mut p);
            p.line(&[M(15.5, 9.5), L(20.5, 14.5), M(20.5, 9.5), L(15.5, 14.5)]);
        }
        Icon::Display => {
            p.outline(&[RRect(3.0, 4.0, 18.0, 12.5, 2.0)]);
            p.line(&[M(12.0, 16.5), L(12.0, 20.0), M(8.0, 20.0), L(16.0, 20.0)]);
        }
        Icon::Power => {
            p.line(&[A(12.0, 13.0, 8.0, -55.0, 235.0)]);
            p.line(&[M(12.0, 3.5), L(12.0, 11.0)]);
        }
        Icon::Play => {
            let tri = [M(8.0, 5.5), L(18.5, 12.0), L(8.0, 18.5), Z];
            p.fill(&tri);
            p.outline(&tri);
        }
        Icon::Pause => p.fill(&[RRect(6.0, 5.0, 4.5, 14.0, 1.2), RRect(13.5, 5.0, 4.5, 14.0, 1.2)]),
        Icon::Stop => p.fill(&[RRect(6.0, 6.0, 12.0, 12.0, 2.0)]),
        Icon::ZoomIn => {
            magnifier(&mut p);
            p.line(&[M(7.5, 10.5), L(13.5, 10.5), M(10.5, 7.5), L(10.5, 13.5)]);
        }
        Icon::ZoomOut => {
            magnifier(&mut p);
            p.line(&[M(7.5, 10.5), L(13.5, 10.5)]);
        }
        Icon::Fullscreen => p.line(&[
            M(4.0, 9.0),
            L(4.0, 4.0),
            L(9.0, 4.0),
            M(15.0, 4.0),
            L(20.0, 4.0),
            L(20.0, 9.0),
            M(20.0, 15.0),
            L(20.0, 20.0),
            L(15.0, 20.0),
            M(9.0, 20.0),
            L(4.0, 20.0),
            L(4.0, 15.0),
        ]),
        Icon::FullscreenExit => p.line(&[
            M(9.0, 4.0),
            L(9.0, 9.0),
            L(4.0, 9.0),
            M(20.0, 9.0),
            L(15.0, 9.0),
            L(15.0, 4.0),
            M(15.0, 20.0),
            L(15.0, 15.0),
            L(20.0, 15.0),
            M(4.0, 15.0),
            L(9.0, 15.0),
            L(9.0, 20.0),
        ]),
        Icon::Sun => {
            p.outline(&[Circle(12.0, 12.0, 4.0)]);
            for i in 0..8 {
                let a = i as f32 * PI / 4.0;
                p.line(&[M(12.0 + 7.0 * a.cos(), 12.0 + 7.0 * a.sin()), L(12.0 + 9.5 * a.cos(), 12.0 + 9.5 * a.sin())]);
            }
        }
        Icon::Moon => {
            p.fill(&[Circle(11.5, 12.5, 8.5)]);
            p.cut(&[Circle(16.5, 8.0, 7.0)]);
        }
        Icon::Lock => {
            p.outline(&[RRect(5.0, 10.5, 14.0, 10.5, 2.0)]);
            p.line(&[M(8.0, 10.5), L(8.0, 7.5), A(12.0, 7.5, 4.0, 180.0, 360.0), L(16.0, 10.5)]);
            p.dot(12.0, 15.7, 1.4);
        }
        Icon::User => {
            p.outline(&[Circle(12.0, 8.0, 3.8)]);
            p.line(&[M(4.5, 20.0), Q(5.0, 14.0, 12.0, 14.0), Q(19.0, 14.0, 19.5, 20.0)]);
        }
        Icon::Clock => {
            p.outline(&[Circle(12.0, 12.0, 9.0)]);
            p.line(&[M(12.0, 7.0), L(12.0, 12.0), L(15.5, 14.0)]);
        }
        Icon::Calendar => {
            p.outline(&[RRect(3.5, 5.0, 17.0, 15.5, 2.0)]);
            p.line(&[M(3.5, 10.0), L(20.5, 10.0), M(8.0, 3.0), L(8.0, 6.5), M(16.0, 3.0), L(16.0, 6.5)]);
        }
        Icon::Download | Icon::Upload => {
            if icon == Icon::Download {
                p.line(&[M(12.0, 4.0), L(12.0, 15.0), M(7.0, 10.5), L(12.0, 15.5), L(17.0, 10.5)]);
            } else {
                p.line(&[M(12.0, 15.5), L(12.0, 4.5), M(7.0, 9.0), L(12.0, 4.0), L(17.0, 9.0)]);
            }
            p.line(&[M(4.0, 15.5), L(4.0, 18.5), Q(4.0, 20.0, 5.5, 20.0), L(18.5, 20.0), Q(20.0, 20.0, 20.0, 18.5), L(20.0, 15.5)]);
        }
        Icon::ExternalLink => {
            p.line(&[M(18.0, 13.5), L(18.0, 18.5), Q(18.0, 20.0, 16.5, 20.0), L(5.5, 20.0), Q(4.0, 20.0, 4.0, 18.5), L(4.0, 7.5), Q(4.0, 6.0, 5.5, 6.0), L(10.5, 6.0)]);
            p.line(&[M(14.0, 4.0), L(20.0, 4.0), L(20.0, 10.0), M(20.0, 4.0), L(11.0, 13.0)]);
        }
        Icon::Eye | Icon::EyeOff => {
            p.outline(&[M(2.5, 12.0), Q(7.0, 5.0, 12.0, 5.0), Q(17.0, 5.0, 21.5, 12.0), Q(17.0, 19.0, 12.0, 19.0), Q(7.0, 19.0, 2.5, 12.0), Z]);
            p.outline(&[Circle(12.0, 12.0, 3.0)]);
            if icon == Icon::EyeOff {
                p.cut(&[M(2.0, 3.5), L(3.5, 2.0), L(22.0, 20.5), L(20.5, 22.0), Z]);
                p.line(&[M(4.0, 4.0), L(20.0, 20.0)]);
            }
        }
        Icon::Filter => p.outline(&[M(3.5, 5.0), L(20.5, 5.0), L(14.0, 12.5), L(14.0, 19.0), L(10.0, 17.0), L(10.0, 12.5), Z]),
        Icon::Sort => p.line(&[
            M(8.0, 19.0),
            L(8.0, 5.0),
            M(4.5, 8.5),
            L(8.0, 5.0),
            L(11.5, 8.5),
            M(16.0, 5.0),
            L(16.0, 19.0),
            M(12.5, 15.5),
            L(16.0, 19.0),
            L(19.5, 15.5),
        ]),
        Icon::Bookmark => p.outline(&[M(6.5, 3.5), L(17.5, 3.5), L(17.5, 20.5), L(12.0, 16.5), L(6.5, 20.5), Z]),
        Icon::Tag => {
            p.outline(&[M(4.0, 4.0), L(11.5, 4.0), L(20.5, 13.0), L(13.0, 20.5), L(4.0, 11.5), Z]);
            p.dot(8.2, 8.2, 1.5);
        }
        Icon::Terminal => {
            p.outline(&[RRect(2.5, 4.0, 19.0, 16.0, 2.5)]);
            p.line(&[M(6.5, 9.0), L(9.5, 12.0), L(6.5, 15.0), M(12.0, 15.0), L(17.0, 15.0)]);
        }
        Icon::Calculator => {
            p.outline(&[RRect(4.5, 2.5, 15.0, 19.0, 2.5)]);
            p.outline(&[RRect(7.5, 5.5, 9.0, 3.5, 1.0)]);
            for y in [12.5, 15.75, 19.0 - 0.25] {
                for x in [8.5, 12.0, 15.5] {
                    p.dot(x, y, 1.15);
                }
            }
        }
        Icon::Cpu => {
            p.outline(&[RRect(6.0, 6.0, 12.0, 12.0, 2.0), RRect(9.5, 9.5, 5.0, 5.0, 1.0)]);
            for c in [9.0, 12.0, 15.0] {
                p.line(&[M(c, 3.0), L(c, 6.0), M(c, 18.0), L(c, 21.0), M(3.0, c), L(6.0, c), M(18.0, c), L(21.0, c)]);
            }
        }
        Icon::Activity => p.line(&[M(3.0, 13.0), L(7.0, 13.0), L(10.0, 6.0), L(14.0, 18.0), L(17.0, 11.0), L(21.0, 11.0)]),
        Icon::HardDrive => {
            p.outline(&[RRect(3.0, 13.0, 18.0, 7.0, 2.0)]);
            p.line(&[M(3.0, 15.0), L(6.5, 5.5), Q(7.0, 4.0, 8.5, 4.0), L(15.5, 4.0), Q(17.0, 4.0, 17.5, 5.5), L(21.0, 15.0)]);
            p.dot(17.0, 16.5, 1.2);
        }
        Icon::Undo => {
            p.line(&[M(9.0, 5.5), L(4.5, 10.0), L(9.0, 14.5)]);
            p.line(&[M(4.5, 10.0), L(14.5, 10.0), Q(20.0, 10.0, 20.0, 15.0), Q(20.0, 20.0, 14.5, 20.0), L(11.0, 20.0)]);
        }
        Icon::Redo => {
            p.line(&[M(15.0, 5.5), L(19.5, 10.0), L(15.0, 14.5)]);
            p.line(&[M(19.5, 10.0), L(9.5, 10.0), Q(4.0, 10.0, 4.0, 15.0), Q(4.0, 20.0, 9.5, 20.0), L(13.0, 20.0)]);
        }
        Icon::Sidebar => {
            p.outline(&[RRect(3.0, 4.0, 18.0, 16.0, 2.5)]);
            p.line(&[M(9.5, 4.0), L(9.5, 20.0)]);
        }
        Icon::Bell => {
            p.outline(&[M(6.0, 16.5), L(6.0, 10.5), Q(6.0, 4.5, 12.0, 4.5), Q(18.0, 4.5, 18.0, 10.5), L(18.0, 16.5), L(19.5, 18.0), L(4.5, 18.0), Z]);
            p.line(&[M(10.0, 21.0), L(14.0, 21.0)]);
        }
        Icon::Battery => {
            p.outline(&[RRect(2.5, 7.0, 17.0, 10.0, 2.0)]);
            p.fill(&[RRect(20.3, 10.0, 1.8, 4.0, 0.8), RRect(5.0, 9.5, 8.0, 5.0, 1.0)]);
        }
        Icon::Palette => {
            p.outline(&[
                M(12.0, 3.5),
                Q(3.5, 3.5, 3.5, 12.0),
                Q(3.5, 20.5, 12.0, 20.5),
                Q(14.0, 20.5, 14.0, 18.5),
                Q(14.0, 17.0, 13.0, 16.0),
                Q(12.0, 15.0, 13.0, 13.8),
                Q(14.0, 12.8, 16.0, 13.0),
                L(17.5, 13.0),
                Q(20.5, 13.0, 20.5, 10.0),
                Q(20.5, 3.5, 12.0, 3.5),
                Z,
            ]);
            for (x, y) in [(7.5, 10.5), (10.0, 7.0), (14.5, 7.0)] {
                p.dot(x, y, 1.35);
            }
        }
        Icon::Backspace => {
            p.outline(&[M(8.0, 5.0), L(20.0, 5.0), Q(21.5, 5.0, 21.5, 6.5), L(21.5, 17.5), Q(21.5, 19.0, 20.0, 19.0), L(8.0, 19.0), L(2.5, 12.0), Z]);
            p.line(&[M(11.5, 9.5), L(16.5, 14.5), M(16.5, 9.5), L(11.5, 14.5)]);
        }
        Icon::Maximize => p.outline(&[RRect(5.5, 5.5, 13.0, 13.0, 2.0)]),
        Icon::Restore => {
            p.outline(&[RRect(4.5, 8.5, 11.0, 11.0, 2.0)]);
            p.line(&[M(8.5, 8.5), L(8.5, 6.5), Q(8.5, 4.5, 10.5, 4.5), L(17.5, 4.5), Q(19.5, 4.5, 19.5, 6.5), L(19.5, 13.5), Q(19.5, 15.5, 17.5, 15.5), L(15.5, 15.5)]);
        }
    }
    p.mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_draws_inside_its_square() {
        let lib = Library::new().unwrap();
        for &icon in Icon::ALL {
            let m = rasterize(&lib, icon, 24);
            let ink: u32 = m.iter().map(|&v| v as u32).sum();
            assert!(ink > 255 * 12, "{} is (nearly) empty: {ink}", icon.name());
            // Nothing touches the outermost pixel ring (designs keep a margin).
            for i in 0..24 {
                for (x, y) in [(i, 0), (i, 23), (0, i), (23, i)] {
                    assert!(m[y * 24 + x] < 128, "{} touches the edge at {x},{y}", icon.name());
                }
            }
            assert_eq!(Icon::from_name(icon.name()), Some(icon));
        }
    }

    #[test]
    fn icons_scale() {
        let lib = Library::new().unwrap();
        let small: u32 = rasterize(&lib, Icon::Folder, 16).iter().map(|&v| v as u32).sum();
        let big: u32 = rasterize(&lib, Icon::Folder, 48).iter().map(|&v| v as u32).sum();
        // Stroke length and width both scale: ink grows with the square of
        // the size (the 1.5 px minimum stroke at 16 px adds a little).
        assert!(big > small * 6 && big < small * 10, "{small} {big}");
    }
}
