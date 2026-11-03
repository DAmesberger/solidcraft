//! Forward-mode derivatives for the reference: a value with its gradient in world space, per mm
//! (docs/material-graph.md §3, "Slopes"). The GPU back ends carry the same gradients for
//! slope-marked wires; the reference carries them everywhere, which changes no value.

use std::ops::{Add, Div, Mul, Neg, Sub};

/// A scalar and its world-space gradient (per mm).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct D {
    pub v: f64,
    pub g: [f64; 3],
}

pub(super) const ZERO3: [f64; 3] = [0.0; 3];

impl D {
    pub const fn c(v: f64) -> D {
        D { v, g: ZERO3 }
    }
    pub const fn new(v: f64, g: [f64; 3]) -> D {
        D { v, g }
    }
    /// `self`'s value with the gradient scaled by `k`: the chain rule for an outer function whose
    /// derivative at `self.v` is `k`.
    pub fn chain(self, v: f64, k: f64) -> D {
        D { v, g: scale(self.g, k) }
    }
    pub fn floor(self) -> D {
        D::c(self.v.floor())
    }
    /// `x − floor(x)`, as WGSL's `fract`.
    pub fn fract(self) -> D {
        D { v: self.v - self.v.floor(), g: self.g }
    }
    pub fn abs(self) -> D {
        let s = if self.v < 0.0 { -1.0 } else { 1.0 };
        self.chain(self.v.abs(), s)
    }
    /// `sqrt(max(x, 0))`; its derivative is zero where x ≤ 0.
    pub fn sqrt0(self) -> D {
        if self.v > 0.0 {
            let r = self.v.sqrt();
            self.chain(r, 0.5 / r)
        } else {
            D::c(0.0)
        }
    }
    pub fn sin(self) -> D {
        self.chain(self.v.sin(), self.v.cos())
    }
    pub fn cos(self) -> D {
        self.chain(self.v.cos(), -self.v.sin())
    }
    pub fn min(self, o: D) -> D {
        // WGSL min returns either on a tie; the reference takes `self` (its value is the same).
        if o.v < self.v { o } else { self }
    }
    pub fn max(self, o: D) -> D {
        if o.v > self.v { o } else { self }
    }
    /// `min(max(x, lo), hi)`: the maths node's clamp, defined for lo > hi too.
    pub fn clamp(self, lo: D, hi: D) -> D {
        self.max(lo).min(hi)
    }
    /// `atan2(y = self, x)`; its gradient is zero within 1e-20 mm² of the origin.
    pub fn atan2(self, x: D) -> D {
        let r2 = self.v * self.v + x.v * x.v;
        let v = self.v.atan2(x.v);
        if r2 < 1e-20 {
            return D::c(v);
        }
        D { v, g: scale(sub3(scale(self.g, x.v), scale(x.g, self.v)), 1.0 / r2) }
    }
    /// `max(a, 0)^b`, with `0^b` as f64's `powf` takes it.
    pub fn pow0(self, b: D) -> D {
        let a = self.v.max(0.0);
        let v = a.powf(b.v);
        let mut g = ZERO3;
        if self.v > 0.0 {
            g = add3(g, scale(self.g, b.v * a.powf(b.v - 1.0)));
            g = add3(g, scale(b.g, v * a.ln()));
        }
        D { v, g }
    }
}

impl Add for D {
    type Output = D;
    fn add(self, o: D) -> D {
        D { v: self.v + o.v, g: add3(self.g, o.g) }
    }
}
impl Sub for D {
    type Output = D;
    fn sub(self, o: D) -> D {
        D { v: self.v - o.v, g: sub3(self.g, o.g) }
    }
}
impl Mul for D {
    type Output = D;
    fn mul(self, o: D) -> D {
        D { v: self.v * o.v, g: add3(scale(self.g, o.v), scale(o.g, self.v)) }
    }
}
impl Div for D {
    type Output = D;
    fn div(self, o: D) -> D {
        let v = self.v / o.v;
        D { v, g: scale(sub3(self.g, scale(o.g, v)), 1.0 / o.v) }
    }
}
impl Neg for D {
    type Output = D;
    fn neg(self) -> D {
        D { v: -self.v, g: scale(self.g, -1.0) }
    }
}
impl Mul<f64> for D {
    type Output = D;
    fn mul(self, k: f64) -> D {
        D { v: self.v * k, g: scale(self.g, k) }
    }
}
impl Add<f64> for D {
    type Output = D;
    fn add(self, k: f64) -> D {
        D { v: self.v + k, g: self.g }
    }
}

/// The denominator rule of `divide`, `remap` and `smoothstep`: held at least 1e-6 away from zero,
/// keeping its sign (zero counts as positive). As WGSL:
/// `select(max(b, 1e-6), min(b, -1e-6), b < 0.0)`.
pub fn guard(b: D) -> D {
    if b.v < 0.0 {
        if b.v > -1e-6 { D::c(-1e-6) } else { b }
    } else if b.v < 1e-6 {
        D::c(1e-6)
    } else {
        b
    }
}

/// WGSL's `smoothstep` on a guarded denominator: `t = clamp((x − e0) / guard(e1 − e0), 0, 1)`,
/// `t²(3 − 2t)`.
pub fn smoothstep(e0: D, e1: D, x: D) -> D {
    let t = ((x - e0) / guard(e1 - e0)).clamp(D::c(0.0), D::c(1.0));
    t * t * (D::c(3.0) - t * 2.0)
}

pub fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn length3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}
pub fn mix3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// The shared library's `sc_unit`: a safe normalise, +Z for a zero vector.
pub fn unit(v: [f64; 3]) -> [f64; 3] {
    let s = v[0].abs().max(v[1].abs()).max(v[2].abs());
    if s == 0.0 || !s.is_finite() {
        return [0.0, 0.0, 1.0];
    }
    let w = scale(v, 1.0 / s);
    scale(w, 1.0 / length3(w))
}

/// The part of `v` perpendicular to unit `n`.
pub fn tangential(v: [f64; 3], n: [f64; 3]) -> [f64; 3] {
    sub3(v, scale(n, dot3(v, n)))
}
