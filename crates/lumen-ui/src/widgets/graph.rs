//! [`LineGraph`]: live line graphs (CPU, memory, network history in System
//! Monitor), drawn with the painter's lines and area fills.

use crate::color::Color;
use crate::geom::{Rect, Vec2};
use crate::layout::{Layout, Size, Sizing};
use crate::tree::Node;
use crate::ui::Ui;

/// Samples plotted right-aligned: the newest at the right edge, `capacity`
/// samples across the width (older ones scroll off to the left), each
/// series a line over a soft filled area, on a rounded panel with
/// horizontal grid lines.
#[must_use = "a LineGraph does nothing until shown"]
pub struct LineGraph<'a> {
    name: String,
    series: Vec<(&'a [f32], Option<Color>)>,
    min: f32,
    max: Option<f32>,
    capacity: usize,
    fill: bool,
    grid: u32,
    layout: Layout,
}

impl<'a> LineGraph<'a> {
    /// `name` identifies the graph in the test widget map.
    pub fn new(name: impl Into<String>) -> LineGraph<'a> {
        LineGraph {
            name: name.into(),
            series: Vec::new(),
            min: 0.0,
            max: None,
            capacity: 60,
            fill: true,
            grid: 4,
            layout: Layout::default().size(Size::FILL, Size::Px(120.0)),
        }
    }

    /// Add a series (oldest sample first) in the next palette color.
    pub fn series(mut self, samples: &'a [f32]) -> LineGraph<'a> {
        self.series.push((samples, None));
        self
    }

    /// Add a series in `color`.
    pub fn series_colored(mut self, samples: &'a [f32], color: Color) -> LineGraph<'a> {
        self.series.push((samples, Some(color)));
        self
    }

    /// The value range of the y axis (default: 0 to the largest sample).
    pub fn range(mut self, min: f32, max: f32) -> LineGraph<'a> {
        self.min = min;
        self.max = Some(max);
        self
    }

    /// How many samples span the width (default 60).
    pub fn capacity(mut self, n: usize) -> LineGraph<'a> {
        self.capacity = n.max(2);
        self
    }

    /// Lines only, no filled areas.
    pub fn no_fill(mut self) -> LineGraph<'a> {
        self.fill = false;
        self
    }

    /// Number of horizontal grid divisions (0 for none; default 4).
    pub fn grid_lines(mut self, n: u32) -> LineGraph<'a> {
        self.grid = n;
        self
    }

    pub fn show(self, ui: &mut Ui) {
        let t = ui.theme().clone();
        let palette = graph_palette(&t);
        let max = self.max.unwrap_or_else(|| {
            let m = self.series.iter().flat_map(|(s, _)| s.iter().copied()).fold(f32::MIN, f32::max);
            if m > self.min { m } else { self.min + 1.0 }
        });
        let (min, cap, fill, grid) = (self.min, self.capacity, self.fill, self.grid);
        let series: Vec<(Vec<f32>, Color)> = self
            .series
            .iter()
            .enumerate()
            .map(|(i, (s, c))| (s[s.len().saturating_sub(cap)..].to_vec(), c.unwrap_or(palette[i % palette.len()])))
            .collect();
        let mut n = Node::new(self.layout);
        n.kind = "graph";
        n.label = Some(self.name);
        if ui.test.enabled() {
            n.value = series.first().and_then(|(s, _)| s.last()).map(|v| format!("{v}"));
        }
        let (bg, line_c, radius) = (t.input_bg, t.separator, t.radius);
        n.paint = Some(Box::new(move |p, r| {
            p.rounded(r, radius, bg);
            let inner = Rect::new(r.x + 1.0, r.y + 6.0, r.w - 2.0, r.h - 7.0);
            for k in 1..grid {
                let y = (inner.y + inner.h * k as f32 / grid as f32).round();
                p.fill(Rect::new(r.x + 8.0, y, r.w - 16.0, 1.0), line_c);
            }
            p.push_clip(r);
            let span = (max - min).max(f32::EPSILON);
            let step = inner.w / (cap - 1) as f32;
            for (samples, color) in &series {
                if samples.len() < 2 {
                    continue;
                }
                let first = cap - samples.len();
                let pts: Vec<Vec2> = samples
                    .iter()
                    .enumerate()
                    .map(|(i, v)| Vec2::new(inner.x + (first + i) as f32 * step, inner.bottom() - ((v - min) / span).clamp(0.0, 1.0) * inner.h))
                    .collect();
                if fill {
                    p.area(&pts, r.bottom(), color.with_alpha(color.a * 0.32), 0.15);
                }
                p.polyline(&pts, 2.0, *color);
            }
            p.pop_clip();
        }));
        ui.add_node(n);
    }
}

impl Sizing for LineGraph<'_> {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

/// Distinct series colors that sit well on the dark theme (the accent
/// first).
pub fn graph_palette(t: &crate::theme::Theme) -> [Color; 6] {
    [t.accent, Color::hex("#73d99f"), Color::hex("#f2a65a"), Color::hex("#c792ea"), Color::hex("#ff7b9c"), Color::hex("#5ccfe6")]
}

impl Ui {
    /// A line graph of one series of `samples` in `0..=max`.
    pub fn graph(&mut self, name: &str, samples: &[f32], max: f32) {
        LineGraph::new(name).series(samples).range(0.0, max).show(self);
    }
}
