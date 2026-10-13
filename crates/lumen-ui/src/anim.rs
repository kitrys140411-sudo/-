//! Springs: the toolkit's only animation primitive (hover and press feedback,
//! toggles, menus, dialogs, smooth scrolling), like the desktop's.
//!
//! Time-based and solved in closed form, so a step of 80 ms equals ten steps
//! of 8 ms: animations never depend on the frame rate. A spring can be
//! retargeted mid-flight and keeps its velocity, which is what makes
//! interrupted animations look natural.
//!
//! Widgets use them through [`crate::Ui::animate`] (state kept per widget id);
//! apps can own a [`Spring`] directly for their own motion.

/// Stiffness and damping of a spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// Angular frequency² (1/s²); higher is faster.
    pub stiffness: f32,
    /// 1 = critically damped (no overshoot), below 1 bounces.
    pub damping: f32,
}

impl SpringParams {
    /// Hover/press color changes: quick, no overshoot.
    pub const FAST: SpringParams = SpringParams { stiffness: 900.0, damping: 1.0 };
    /// Toggles, selections, small movements: snappy with a hint of bounce.
    pub const SNAPPY: SpringParams = SpringParams { stiffness: 700.0, damping: 0.86 };
    /// Popovers, dialogs and other things that appear (the shell's popover
    /// spring).
    pub const POPOVER: SpringParams = SpringParams { stiffness: 700.0, damping: 0.85 };
    /// Larger movements (smooth scrolling, panels).
    pub const GENTLE: SpringParams = SpringParams { stiffness: 260.0, damping: 1.0 };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub params: SpringParams,
}

impl Spring {
    /// At rest at `value`.
    pub fn new(value: f32, params: SpringParams) -> Spring {
        Spring { value, velocity: 0.0, target: value, params }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump to `value` without animating.
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn is_settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }

    /// Advance by `dt` seconds. `epsilon` is the distance (in the value's
    /// unit) below which the spring snaps to rest.
    pub fn step(&mut self, dt: f32, epsilon: f32) {
        if self.is_settled() {
            return;
        }
        let t = dt.clamp(0.0, 0.25) as f64;
        let w0 = (self.params.stiffness.max(1.0) as f64).sqrt();
        let zeta = self.params.damping.max(0.05) as f64;
        let x0 = (self.value - self.target) as f64;
        let v0 = self.velocity as f64;
        let (x, v);
        if (zeta - 1.0).abs() < 1e-4 {
            // Critically damped: x(t) = (x0 + (v0 + w0 x0) t) e^{-w0 t}
            let e = (-w0 * t).exp();
            let b = v0 + w0 * x0;
            x = (x0 + b * t) * e;
            v = (b - w0 * (x0 + b * t)) * e;
        } else if zeta < 1.0 {
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let e = (-zeta * w0 * t).exp();
            let b = (v0 + zeta * w0 * x0) / wd;
            let (s, c) = (wd * t).sin_cos();
            x = e * (x0 * c + b * s);
            v = e * ((b * wd - zeta * w0 * x0) * c - (x0 * wd + zeta * w0 * b) * s);
        } else {
            // Over-damped: two decaying exponentials.
            let r = w0 * (zeta * zeta - 1.0).sqrt();
            let (r1, r2) = (-zeta * w0 + r, -zeta * w0 - r);
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            x = c1 * (r1 * t).exp() + c2 * (r2 * t).exp();
            v = c1 * r1 * (r1 * t).exp() + c2 * r2 * (r2 * t).exp();
        }
        self.value = self.target + x as f32;
        self.velocity = v as f32;
        let eps = epsilon.max(1e-6);
        if (x as f32).abs() < eps && (self.velocity).abs() < eps * 10.0 {
            self.snap(self.target);
        }
    }
}

/// Ease-out cubic, for the few animations that are not springs (spinner
/// arcs, indeterminate progress).
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_size_does_not_matter() {
        let mut a = Spring::new(0.0, SpringParams::SNAPPY);
        let mut b = a;
        a.set_target(1.0);
        b.set_target(1.0);
        a.step(0.08, 1e-4);
        for _ in 0..10 {
            b.step(0.008, 1e-4);
        }
        assert!((a.value - b.value).abs() < 1e-4, "{} vs {}", a.value, b.value);
    }

    #[test]
    fn settles_on_target() {
        for p in [SpringParams::FAST, SpringParams::SNAPPY, SpringParams::GENTLE, SpringParams { stiffness: 300.0, damping: 1.6 }] {
            let mut s = Spring::new(0.0, p);
            s.set_target(10.0);
            let mut steps = 0;
            while !s.is_settled() && steps < 1000 {
                s.step(1.0 / 60.0, 0.001);
                steps += 1;
            }
            assert!(s.is_settled() && s.value == 10.0, "{p:?} after {steps} steps: {s:?}");
            assert!(steps < 120, "{p:?} too slow: {steps}");
        }
    }

    #[test]
    fn underdamped_overshoots_critically_damped_does_not() {
        let mut bouncy = Spring::new(0.0, SpringParams { stiffness: 600.0, damping: 0.5 });
        let mut crit = Spring::new(0.0, SpringParams::FAST);
        bouncy.set_target(1.0);
        crit.set_target(1.0);
        let (mut max_b, mut max_c) = (0.0f32, 0.0f32);
        for _ in 0..120 {
            bouncy.step(1.0 / 60.0, 1e-4);
            crit.step(1.0 / 60.0, 1e-4);
            max_b = max_b.max(bouncy.value);
            max_c = max_c.max(crit.value);
        }
        assert!(max_b > 1.05);
        assert!(max_c <= 1.0 + 1e-4);
    }
}
