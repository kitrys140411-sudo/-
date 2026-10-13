//! Test hooks for driving apps headless (see the README's testing section).
//!
//! - `LUMEN_UI_TEST=1`: events are printed to stdout, one per line
//!   (`ready`, `click <label>`, `toggle <label> on`, `focus <label>`, and
//!   whatever the app logs with [`crate::Ui::log`]).
//! - `LUMEN_UI_TEST_MAP=<file>`: after every frame whose layout changed, a
//!   JSON description of the labelled widgets is written there (atomically):
//!   `{"size":[w,h],"scale":s,"widgets":[{"kind":"button","label":"Add",
//!   "x":..,"y":..,"w":..,"h":..,"hovered":false,"focused":false,
//!   "value":"..."}]}` in logical window coordinates, so a test can click a
//!   widget by its label.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;

use crate::geom::Rect;
use crate::tree::Tree;

pub(crate) struct TestLog {
    enabled: bool,
    map: Option<PathBuf>,
    last: String,
    /// Lines kept for the in-process test harness.
    captured: Option<std::cell::RefCell<Vec<String>>>,
}

impl TestLog {
    pub fn from_env() -> TestLog {
        let enabled = std::env::var_os("LUMEN_UI_TEST").is_some_and(|v| !v.is_empty() && v != "0");
        let map = std::env::var_os("LUMEN_UI_TEST_MAP").map(PathBuf::from);
        TestLog { enabled, map, last: String::new(), captured: None }
    }

    /// Keep event lines in memory (the harness reads them).
    pub fn capture(&mut self) {
        self.captured = Some(Default::default());
    }

    pub fn take_captured(&self) -> Vec<String> {
        self.captured.as_ref().map(|c| std::mem::take(&mut *c.borrow_mut())).unwrap_or_default()
    }

    pub fn enabled(&self) -> bool {
        self.enabled || self.map.is_some() || self.captured.is_some()
    }

    pub fn event(&self, line: &str) {
        if let Some(c) = &self.captured {
            c.borrow_mut().push(line.to_string());
        }
        if self.enabled {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }

    /// Write the widget map if it changed.
    pub fn write_map(&mut self, tree: &Tree, size: (f32, f32), scale: f32, focus: Option<crate::Id>, hovered: Option<crate::Id>) {
        let Some(path) = &self.map else { return };
        let mut s = format!("{{\"size\":[{},{}],\"scale\":{},\"widgets\":[", size.0, size.1, scale);
        let mut first = true;
        // Visible part of each node: clipped by its clipping ancestors.
        let mut stack: Vec<(u32, Rect)> = tree.roots.iter().rev().map(|r| (r.node, Rect::new(0.0, 0.0, size.0, size.1))).collect();
        let mut order = Vec::new();
        while let Some((i, clip)) = stack.pop() {
            let n = &tree.nodes[i as usize];
            order.push((i, clip));
            let clip = if n.clip { clip.intersect(&n.rect) } else { clip };
            let kids: Vec<u32> = tree.children(i).collect();
            for c in kids.into_iter().rev() {
                stack.push((c, clip));
            }
        }
        for (i, clip) in order {
            let n = &tree.nodes[i as usize];
            let Some(label) = &n.label else { continue };
            if n.kind.is_empty() {
                continue;
            }
            let r = n.rect.intersect(&clip);
            if r.is_empty() || n.opacity <= 0.0 {
                continue;
            }
            if !first {
                s.push(',');
            }
            first = false;
            let _ = write!(
                s,
                "{{\"kind\":{},\"label\":{},\"x\":{:.1},\"y\":{:.1},\"w\":{:.1},\"h\":{:.1},\"hovered\":{},\"focused\":{}",
                json_str(n.kind),
                json_str(label),
                r.x,
                r.y,
                r.w,
                r.h,
                n.id.is_some() && n.id == hovered,
                n.id.is_some() && n.id == focus
            );
            if let Some(v) = &n.value {
                let _ = write!(s, ",\"value\":{}", json_str(v));
            }
            s.push('}');
        }
        s.push_str("]}\n");
        if s == self.last {
            return;
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &s).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            self.last = s;
        }
    }
}

pub(crate) fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_escaping() {
        assert_eq!(super::json_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }
}
