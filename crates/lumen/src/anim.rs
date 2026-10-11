//! Time-based animation primitives.
//!
//! Everything animates from wall-clock time (the predicted presentation time of
//! the frame being rendered), never from frame counts, so a dropped or delayed
//! frame never slows an animation down.

use std::time::Duration;

/// A damped harmonic oscillator (spring). With `damping_ratio = 1` it is
/// critically damped: the fastest approach without overshoot. Springs can be
/// retargeted mid-flight and keep their velocity, which is what makes
/// interrupted animations look natural.
#[derive(Debug, Clone, Copy)]
pub struct Spring {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    /// Angular frequency² (stiffness / mass), in 1/s².
    pub stiffness: f64,
    pub damping_ratio: f64,
    /// Distance and speed below which the spring snaps to rest.
    pub epsilon: f64,
}

impl Spring {
    pub fn new(value: f64, params: SpringParams) -> Self {
        Spring {
            value,
            velocity: 0.0,
            target: value,
            stiffness: params.stiffness,
            damping_ratio: params.damping_ratio,
            epsilon: params.epsilon,
        }
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// Jump to a value without animating.
    pub fn snap(&mut self, value: f64) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn is_settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }

    /// Advance by `dt` using the closed-form solution of the damped oscillator
    /// (exact for any step size, so no sub-stepping and no instability).
    pub fn step(&mut self, dt: Duration) {
        if self.is_settled() {
            return;
        }
        let t = dt.as_secs_f64().min(0.25);
        let w0 = self.stiffness.sqrt();
        let zeta = self.damping_ratio;
        let x0 = self.value - self.target;
        let v0 = self.velocity;
        let (x, v);
        if (zeta - 1.0).abs() < 1e-6 {
            // Critically damped: x(t) = (x0 + (v0 + w0 x0) t) e^{-w0 t}
            let e = (-w0 * t).exp();
            let b = v0 + w0 * x0;
            x = (x0 + b * t) * e;
            v = (b - w0 * (x0 + b * t)) * e;
        } else if zeta < 1.0 {
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let e = (-zeta * w0 * t).exp();
            let a = x0;
            let b = (v0 + zeta * w0 * x0) / wd;
            let (s, c) = (wd * t).sin_cos();
            x = e * (a * c + b * s);
            v = e * ((b * wd - zeta * w0 * a) * c - (a * wd + zeta * w0 * b) * s);
        } else {
            // Over-damped: sum of two decaying exponentials.
            let r = w0 * (zeta * zeta - 1.0).sqrt();
            let r1 = -zeta * w0 + r;
            let r2 = -zeta * w0 - r;
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            x = c1 * (r1 * t).exp() + c2 * (r2 * t).exp();
            v = c1 * r1 * (r1 * t).exp() + c2 * r2 * (r2 * t).exp();
        }
        self.value = self.target + x;
        self.velocity = v;
        if x.abs() < self.epsilon && v.abs() < self.epsilon * 10.0 {
            self.snap(self.target);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SpringParams {
    pub stiffness: f64,
    pub damping_ratio: f64,
    pub epsilon: f64,
}

impl SpringParams {
    pub const fn new(stiffness: f64, damping_ratio: f64, epsilon: f64) -> Self {
        SpringParams { stiffness, damping_ratio, epsilon }
    }
}

/// A 2D spring (positions).
#[derive(Debug, Clone, Copy)]
pub struct Spring2 {
    pub x: Spring,
    pub y: Spring,
}

impl Spring2 {
    pub fn new(x: f64, y: f64, params: SpringParams) -> Self {
        Spring2 { x: Spring::new(x, params), y: Spring::new(y, params) }
    }
    pub fn set_target(&mut self, x: f64, y: f64) {
        self.x.set_target(x);
        self.y.set_target(y);
    }
    pub fn snap(&mut self, x: f64, y: f64) {
        self.x.snap(x);
        self.y.snap(y);
    }
    pub fn step(&mut self, dt: Duration) {
        self.x.step(dt);
        self.y.step(dt);
    }
    pub fn is_settled(&self) -> bool {
        self.x.is_settled() && self.y.is_settled()
    }
    pub fn value(&self) -> (f64, f64) {
        (self.x.value, self.y.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: SpringParams = SpringParams::new(400.0, 1.0, 0.001);

    #[test]
    fn critically_damped_converges_without_overshoot() {
        let mut s = Spring::new(0.0, P);
        s.set_target(100.0);
        let mut max: f64 = 0.0;
        for _ in 0..120 {
            s.step(Duration::from_millis(8));
            max = max.max(s.value);
        }
        assert!(s.is_settled());
        assert_eq!(s.value, 100.0);
        assert!(max <= 100.0 + 1e-9, "overshoot {max}");
    }

    #[test]
    fn step_size_independent() {
        // The closed form makes one big step equal to many small ones.
        let mut a = Spring::new(0.0, P);
        let mut b = Spring::new(0.0, P);
        a.set_target(1.0);
        b.set_target(1.0);
        a.step(Duration::from_millis(80));
        for _ in 0..10 {
            b.step(Duration::from_millis(8));
        }
        assert!((a.value - b.value).abs() < 1e-9, "{} vs {}", a.value, b.value);
    }

    #[test]
    fn underdamped_overshoots_and_settles() {
        let mut s = Spring::new(0.0, SpringParams::new(600.0, 0.6, 0.001));
        s.set_target(1.0);
        let mut max: f64 = 0.0;
        for _ in 0..400 {
            s.step(Duration::from_millis(5));
            max = max.max(s.value);
        }
        assert!(max > 1.01);
        assert!(s.is_settled());
    }

    #[test]
    fn retarget_keeps_velocity() {
        let mut s = Spring::new(0.0, P);
        s.set_target(1.0);
        s.step(Duration::from_millis(30));
        let v = s.velocity;
        s.set_target(2.0);
        assert_eq!(s.velocity, v);
    }
}
