//! Projections, patterns, images and normals of the reference (the shared library's
//! `projection.wgsl` and `patterns.wgsl`, and the normal nodes), in f64. The formulas are written
//! out in the module documentation of [`super`]; this file is their one implementation.

use super::dual::{D, ZERO3, add3, cross, dot3, length3, scale, tangential, unit};
use super::noise;
use super::{Num, Plane, Point, ReferenceTexture, Texcoord};

/// The step of every pattern's finite-difference slope, as a fraction of its period along each
/// axis (the former `pat_bumps`' 0.05).
pub const SLOPE_STEP: f64 = 0.05;

/// `pat_fade`: how much of a pattern of `period` shows at footprint `fp` (both in uv units).
pub fn fade(fp: f64, period: f64) -> f64 {
    1.0 - smooth(0.2, 0.6, fp / period.max(1e-6))
}

/// `pat_weave_fade`: the weave's sharper fade.
pub fn weave_fade(fp: f64, period: f64) -> f64 {
    1.0 - smooth(0.1, 0.3, fp / period.max(1e-6))
}

/// WGSL's `smoothstep` with constant edges (e0 < e1).
pub fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One plane of a projection from a texcoord's uv as duals (value and world gradient).
fn plane(u: D, v: D, p: &Point) -> Plane {
    let gu = u.g;
    let gv = v.g;
    let fp = p.footprint_mm * length3(gu).max(length3(gv));
    Plane { uv: [u.v, v.v], gu, gv, fp, tangent: plane_tangent(gu, p) }
}

/// The unit world direction of increasing u along the surface; the geometric tangent where u
/// does not change along it.
pub fn plane_tangent(gu: [f64; 3], p: &Point) -> [f64; 3] {
    let t = tangential(gu, p.world_normal);
    if length3(t) < 1e-12 { p.tangent } else { unit(t) }
}

fn sub3d(a: [D; 3], b: [D; 3]) -> [D; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3d(a: [D; 3], b: [D; 3]) -> D {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn consts(v: [f64; 3]) -> [D; 3] {
    v.map(D::c)
}

/// `sc_repeat_axis`: `axis / |axis|²`, zero for a zero axis.
pub fn repeat_axis(a: [f64; 3]) -> [f64; 3] {
    let l2 = dot3(a, a);
    if l2 > 0.0 { scale(a, 1.0 / l2) } else { ZERO3 }
}

/// `texcoord.planar`: uv = ((q − origin)·u/|u|², (q − origin)·v/|v|²).
pub fn planar(q: [D; 3], origin: [f64; 3], u: [f64; 3], v: [f64; 3], p: &Point) -> Texcoord {
    let d = sub3d(q, consts(origin));
    let pl = plane(dot3d(d, consts(repeat_axis(u))), dot3d(d, consts(repeat_axis(v))), p);
    Texcoord { planes: vec![(pl, 1.0)] }
}

/// `sc_triplanar_weights`.
pub fn triplanar_weights(n: [f64; 3], sharpness: f64) -> [f64; 3] {
    let w = n.map(|c| c.abs().max(1e-6).powf(sharpness));
    let s = w[0] + w[1] + w[2];
    w.map(|c| c / s)
}

/// `texcoord.triplanar`: r = (q − origin) / scale_mm per axis; the x plane reads (r.y, r.z), the
/// y plane (r.x, r.z) and the z plane (r.x, r.y), weighted by [`triplanar_weights`] of the
/// projection space's normal.
pub fn triplanar(q: [D; 3], n: [f64; 3], origin: [f64; 3], scale_mm: [f64; 3], sharpness: f64, p: &Point) -> Texcoord {
    let d = sub3d(q, consts(origin));
    let r = [d[0] * (1.0 / scale_mm[0]), d[1] * (1.0 / scale_mm[1]), d[2] * (1.0 / scale_mm[2])];
    let w = triplanar_weights(n, sharpness);
    Texcoord { planes: vec![(plane(r[1], r[2], p), w[0]), (plane(r[0], r[2], p), w[1]), (plane(r[0], r[1], p), w[2])] }
}

/// `texcoord.cylindrical`: u = atan2((q − o)·(axis × reference), (q − o)·reference), times
/// `radius_mm` for `arc_length`; v = (q − o)·axis (`sc_cylindrical_uv`).
pub fn cylindrical(q: [D; 3], origin: [f64; 3], axis: [f64; 3], reference: [f64; 3], radius: f64, arc_length: bool, p: &Point) -> Texcoord {
    let d = sub3d(q, consts(origin));
    let side = cross(axis, reference);
    let angle = dot3d(d, consts(side)).atan2(dot3d(d, consts(reference)));
    let u = if arc_length { angle * radius } else { angle };
    Texcoord { planes: vec![(plane(u, dot3d(d, consts(axis)), p), 1.0)] }
}

/// `texcoord.transform`: uv' = R(rotation)·(uv ⊙ scale) + offset, on every plane.
pub fn transform(t: &Texcoord, offset: [f64; 2], scale2: [f64; 2], rotation: f64, p: &Point) -> Texcoord {
    let (s, c) = rotation.sin_cos();
    let planes = t
        .planes
        .iter()
        .map(|(pl, w)| {
            let u = D::new(pl.uv[0], pl.gu) * scale2[0];
            let v = D::new(pl.uv[1], pl.gv) * scale2[1];
            let nu = u * c - v * s + offset[0];
            let nv = u * s + v * c + offset[1];
            (plane(nu, nv, p), *w)
        })
        .collect();
    Texcoord { planes }
}

/// A pattern's result on one plane: value, slope (d value / d u, d value / d v) and the tangent.
#[derive(Clone, Copy, Debug)]
pub struct PatternOut {
    pub value: f64,
    pub slope: [f64; 2],
}

/// The forward-difference slope of `f` at `uv` with steps `h` (uv units).
fn slope(f: &dyn Fn([f64; 2]) -> f64, uv: [f64; 2], h: [f64; 2], at: f64) -> [f64; 2] {
    [(f([uv[0] + h[0], uv[1]]) - at) / h[0], (f([uv[0], uv[1] + h[1]]) - at) / h[1]]
}

fn pattern(f: &dyn Fn([f64; 2]) -> f64, uv: [f64; 2], period: [f64; 2]) -> PatternOut {
    let value = f(uv);
    PatternOut { value, slope: slope(f, uv, [SLOPE_STEP * period[0], SLOPE_STEP * period[1]], value) }
}

/// Fade `raw` toward `mean` by `k` ∈ [0, 1] (1: all of the pattern shows).
fn toward(mean: f64, raw: f64, k: f64) -> f64 {
    mean + k * (raw - mean)
}

/// `pattern.noise` in 2D: `sc_fbm` at (uv / scale_mm, 0), faded toward 0.5 by
/// `pat_fade(fp, scale_mm)`.
pub fn noise2(pl: &Plane, scale_mm: f64, lacunarity: f64, gain: f64, seed: u32, kind: i64, octaves: i64) -> PatternOut {
    let k = fade(pl.fp, scale_mm);
    let f = |uv: [f64; 2]| toward(0.5, noise::fbm(kind, 2, [uv[0] / scale_mm, uv[1] / scale_mm, 0.0], seed, octaves, lacunarity, gain), k);
    pattern(&f, pl.uv, [scale_mm, scale_mm])
}

/// `pattern.noise` in 3D: `sc_fbm` at position / scale_mm; returns the value and its gradient
/// along the position's own axes (forward differences of 0.05·scale_mm).
pub fn noise3(q: [f64; 3], fp: f64, scale_mm: f64, lacunarity: f64, gain: f64, seed: u32, kind: i64, octaves: i64) -> (f64, [f64; 3]) {
    let k = fade(fp, scale_mm);
    let f = |q: [f64; 3]| toward(0.5, noise::fbm(kind, 3, q.map(|c| c / scale_mm), seed, octaves, lacunarity, gain), k);
    let v = f(q);
    let h = SLOPE_STEP * scale_mm;
    let d = |axis: usize| {
        let mut r = q;
        r[axis] += h;
        (f(r) - v) / h
    };
    (v, [d(0), d(1), d(2)])
}

/// `pattern.brushed`: value noise stretched along u, p = (u / streak_mm, v · density_per_mm),
/// `mix(sc_value_noise2(p, seed), sc_value_noise2(p·(0.5, 3) + (19, 7), seed + 1), jitter)`,
/// contrast about 0.5, faded by `pat_fade(fp, 1 / density_per_mm)`.
pub fn brushed(pl: &Plane, density: f64, streak: f64, contrast: f64, jitter: f64, seed: u32) -> PatternOut {
    let k = fade(pl.fp, 1.0 / density);
    let f = |uv: [f64; 2]| {
        let p = [uv[0] / streak, uv[1] * density];
        let a = noise::value2(p, seed);
        let b = noise::value2([p[0] * 0.5 + 19.0, p[1] * 3.0 + 7.0], seed.wrapping_add(1));
        let n = a + (b - a) * jitter;
        toward(0.5, 0.5 + contrast * (n - 0.5), k)
    };
    pattern(&f, pl.uv, [streak, 1.0 / density])
}

/// A yarn's crown at `t` across it, flattened by `crown` ∈ [0, 1]:
/// `mix(1, sqrt(max(1 − 4t², 0)), crown)`.
fn crown_at(t: f64, crown: f64) -> f64 {
    1.0 + ((1.0 - 4.0 * t * t).max(0.0).sqrt() - 1.0) * crown
}

/// The weave's raw height at `p` (in pitches): the former `pat_weave_parts` with a variable
/// crown, kind and fibre striations. `kind` 0 is plain, 1 is a 2/2 twill.
pub fn weave_raw(p: [f64; 2], kind: i64, crown: f64, fibres: f64, fibre_contrast: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2};
    let cell = [p[0].floor(), p[1].floor()];
    let f = [p[0] - cell[0] - 0.5, p[1] - cell[1] - 0.5];
    let (up_x, up_y) = if kind == 1 {
        (
            (0.5 + 0.5 * SQRT_2 * (FRAC_PI_2 * (p[0] - 0.5 + cell[1]) - FRAC_PI_4).cos()).clamp(0.0, 1.0),
            (0.5 - 0.5 * SQRT_2 * (FRAC_PI_2 * (p[1] - 0.5 + cell[0]) - FRAC_PI_4).cos()).clamp(0.0, 1.0),
        )
    } else {
        (0.5 + 0.5 * (PI * (p[0] - 0.5 + cell[1])).cos(), 0.5 - 0.5 * (PI * (p[1] - 0.5 + cell[0])).cos())
    };
    let hx = crown_at(f[1], crown) * (0.6 + 0.4 * up_x);
    let hy = crown_at(f[0], crown) * (0.6 + 0.4 * up_y);
    // WGSL `select(f.x, f.y, hx >= hy)`: across the visible yarn.
    let across = if hx >= hy { f[1] } else { f[0] };
    let fibre = 1.0 - fibre_contrast * 0.5 * (1.0 - (2.0 * PI * fibres * across).cos());
    hx.max(hy) * fibre
}

/// `pattern.weave`: [`weave_raw`] at uv / pitch_mm, faded toward 0.5 by
/// `pat_weave_fade(fp, min(pitch))`; the fibres by `pat_weave_fade(fp, min(pitch) / max(fibres, 1))`.
pub fn weave(pl: &Plane, pitch: [f64; 2], crown: f64, fibres: f64, fibre_contrast: f64, kind: i64) -> PatternOut {
    let period = pitch[0].min(pitch[1]);
    let k = weave_fade(pl.fp, period);
    let fc = fibre_contrast * weave_fade(pl.fp, period / fibres.max(1.0));
    let f = |uv: [f64; 2]| toward(0.5, weave_raw([uv[0] / pitch[0], uv[1] / pitch[1]], kind, crown, fibres, fc), k);
    pattern(&f, pl.uv, pitch)
}

/// One axis of the checker: the square wave (+1 on even cells, −1 on odd ones) box-filtered over
/// a width `w` (in cells) where one edge is inside the box: `sign · min(2d / w, 1)`, d the
/// distance to the nearest edge in cells. Written without differences of nearby values, so it
/// keeps its precision in `f32`.
fn checker_axis(t: f64, w: f64) -> f64 {
    let x = t * 0.5 - (t * 0.5).floor();
    let sign = if x < 0.5 { 1.0 } else { -1.0 };
    let d = 2.0 * x.min((x - 0.5).abs()).min(1.0 - x);
    sign * (2.0 * d / w).min(1.0)
}

/// `pattern.checker`: 0 on cells whose index sum is even, 1 on odd ones:
/// `0.5 − 0.5·ix·iy·pat_fade(fp, min(size))`, t = uv / size and w = max(fp / size, 1e-6) per
/// axis.
pub fn checker(pl: &Plane, size: [f64; 2]) -> PatternOut {
    let w = [(pl.fp / size[0]).max(1e-6), (pl.fp / size[1]).max(1e-6)];
    let k = fade(pl.fp, size[0].min(size[1]));
    let f = |uv: [f64; 2]| 0.5 - 0.5 * checker_axis(uv[0] / size[0], w[0]) * checker_axis(uv[1] / size[1], w[1]) * k;
    pattern(&f, pl.uv, size)
}

/// `pattern.grid`: 1 within `line_mm / 2` of a line u = i·size.x or v = j·size.y, edges smoothed
/// over half the footprint, faded toward the covered fraction by `pat_fade(fp, min(size))`.
pub fn grid(pl: &Plane, size: [f64; 2], line: f64) -> PatternOut {
    let half = 0.5 * line;
    let w = 0.5 * pl.fp.max(1e-6);
    let mean_axis = |s: f64| (line / s).clamp(0.0, 1.0);
    let mean = 1.0 - (1.0 - mean_axis(size[0])) * (1.0 - mean_axis(size[1]));
    let k = fade(pl.fp, size[0].min(size[1]));
    let axis = |x: f64, s: f64| {
        let t = x / s + 0.5;
        let d = (t - t.floor() - 0.5).abs() * s;
        1.0 - smooth(half - w, half + w, d)
    };
    let f = |uv: [f64; 2]| toward(mean, axis(uv[0], size[0]).max(axis(uv[1], size[1])), k);
    pattern(&f, pl.uv, size)
}

/// `pattern.gradient`: t = (u − start) / guard(end − start) (`linear`) or
/// (|uv| − start) / guard(end − start) (`radial`), clamped to [0, 1]. No fade.
pub fn gradient(pl: &Plane, start: f64, end: f64, radial: bool) -> PatternOut {
    let den = super::dual::guard(D::c(end - start)).v;
    let f = |uv: [f64; 2]| {
        let x = if radial { (uv[0] * uv[0] + uv[1] * uv[1]).sqrt() } else { uv[0] };
        ((x - start) / den).clamp(0.0, 1.0)
    };
    let h = (end - start).abs().max(1e-6);
    pattern(&f, pl.uv, [h, h])
}

/// sRGB transfer decode (IEC 61966-2-1), as the GPU's sRGB texture formats apply per texel.
pub fn srgb_decode(c: f64) -> f64 {
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A texel index under a wrap mode (0 repeat, 1 clamp to edge, 2 mirrored repeat).
fn wrap_index(i: i64, n: i64, wrap: i64) -> i64 {
    match wrap {
        1 => i.clamp(0, n - 1),
        2 => {
            let m = i.rem_euclid(2 * n);
            if m >= n { 2 * n - 1 - m } else { m }
        }
        _ => i.rem_euclid(n),
    }
}

/// Bilinear sampling of mip level 0 at `uv`, with v = 1 at the image's top row (texel row 0):
/// texel coordinates x = u·width − 0.5, y = (1 − v)·height − 0.5. sRGB texels are decoded before
/// filtering. RGBA.
pub fn sample(t: &ReferenceTexture, uv: [f64; 2], wrap: i64, srgb: bool) -> [f64; 4] {
    let (w, h) = (i64::from(t.width), i64::from(t.height));
    if w == 0 || h == 0 {
        return [0.0; 4];
    }
    let x = uv[0] * w as f64 - 0.5;
    let y = (1.0 - uv[1]) * h as f64 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let texel = |i: f64, j: f64| {
        let ix = wrap_index(i as i64, w, wrap);
        let iy = wrap_index(j as i64, h, wrap);
        let k = usize::try_from(iy * w + ix).unwrap_or(0);
        let c = t.texels.get(k).copied().unwrap_or([0.0; 4]);
        if srgb { [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2]), c[3]] } else { c }
    };
    let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1.0, y0), texel(x0, y0 + 1.0), texel(x0 + 1.0, y0 + 1.0));
    let mut out = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        out[k] = top + (bottom - top) * fy;
    }
    out
}

/// Rec.709 luminance weights (the `luminance` node and an `rgb` image's `value`).
pub const LUMINANCE: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// `normal.from_height`: `sc_unit(n − relief_mm · tangential(∇height))`, n the geometric world
/// normal and ∇height the height's world gradient per mm.
pub fn from_height(height: &Num, relief: f64, p: &Point) -> [f64; 3] {
    let n = p.world_normal;
    let g = tangential(height.c[0].g, n);
    unit(add3(n, scale(g, -relief)))
}

/// `normal.map`: t = 2c − 1 (y negated for `directx`), t.xy scaled by `strength`,
/// `sc_unit(t.x·T + t.y·B + t.z·n)` with T the frame tangent and B = n × T.
pub fn normal_map(color: [f64; 3], strength: f64, directx: bool, tangent: [f64; 3], p: &Point) -> [f64; 3] {
    let n = p.world_normal;
    let mut t = color.map(|c| 2.0 * c - 1.0);
    if directx {
        t[1] = -t[1];
    }
    let b = cross(n, tangent);
    unit(add3(add3(scale(tangent, t[0] * strength), scale(b, t[1] * strength)), scale(n, t[2])))
}
