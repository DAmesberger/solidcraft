// Shared shading library: patterns (crates/gpu/src/shading/mod.rs). Pure functions, `sc_` names,
// WebGL2-safe. The pattern, normal and maths nodes of a material graph (docs/material-graph.md
// §3.3, §3.4). Every pattern fades to its mean as its period nears the pixel footprint, so no
// graph aliases into moiré; a footprint of zero (the path tracer's) shows it unfiltered.

// A pattern's outputs: its value, its slope along the texcoord's u and v (per uv unit), the same
// slope as a spatial gradient (per mm; `sc_Texcoord::du` and `dv`), and the texcoord's tangent.
struct sc_Pattern {
    value: f32,
    slope: vec2<f32>,
    grad: vec3<f32>,
    tangent: vec3<f32>,
};

// How much of a pattern of period `period` shows at a footprint `fp` (same units).
fn sc_fade(fp: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.2, 0.6, fp / max(period, 1e-6));
}

// The fade of patterns with sharp edges and fine fibres: gone before two pixels a period.
fn sc_fade_sharp(fp: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.1, 0.3, fp / max(period, 1e-6));
}

fn sc_pattern_of(t: sc_Texcoord, value: f32, slope: vec2<f32>) -> sc_Pattern {
    return sc_Pattern(value, slope, slope.x * t.du + slope.y * t.dv, sc_tc_tangent(t));
}

// Three planar evaluations of a triplanar pattern, blended by the weights `w`.
fn sc_pattern_blend(a: sc_Pattern, b: sc_Pattern, c: sc_Pattern, w: vec3<f32>) -> sc_Pattern {
    let tangent = sc_proj_unit(w.x * a.tangent + w.y * b.tangent + w.z * c.tangent, vec3<f32>(1.0, 0.0, 0.0));
    return sc_Pattern(w.x * a.value + w.y * b.value + w.z * c.value, w.x * a.slope + w.y * b.slope + w.z * c.slope,
        w.x * a.grad + w.y * b.grad + w.z * c.grad, tangent);
}

// fBm of `sc_noise` at `p` (lattice units) in [0, 1], mean 0.5: each octave fades to the mean as
// its period nears the footprint `fp` (lattice units).
fn sc_noise_octaves(kind: u32, dimensions: u32, p: vec3<f32>, fp: f32, seed: u32, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    var sc_sum = 0.0;
    var sc_total = 0.0;
    var sc_amplitude = 1.0;
    var sc_frequency = 1.0;
    for (var k = 0u; k < clamp(octaves, 1u, 8u); k += 1u) {
        let n = sc_noise(kind, dimensions, p * sc_frequency, seed + k);
        sc_sum += sc_amplitude * mix(0.5, n, sc_fade(fp * sc_frequency, 1.0));
        sc_total += sc_amplitude;
        sc_amplitude *= gain;
        sc_frequency *= lacunarity;
    }
    return select(0.5, sc_sum / sc_total, sc_total > 0.0);
}

// `pattern.noise` over a texcoord: lattice step `scale_mm` (uv units). With `slope`, the slope by
// a fixed finite difference in pattern space.
fn sc_pattern_noise(t: sc_Texcoord, kind: u32, dimensions: u32, octaves: u32, scale_mm: f32, lacunarity: f32, gain: f32, seed: u32, slope: bool) -> sc_Pattern {
    let s = max(scale_mm, 1e-6);
    let p = vec3<f32>(t.uv / s, 0.0);
    let fp = t.footprint / s;
    let m = sc_noise_octaves(kind, dimensions, p, fp, seed, octaves, lacunarity, gain);
    if (!slope) {
        return sc_pattern_of(t, m, vec2<f32>(0.0));
    }
    let e = 0.05;
    let a = sc_noise_octaves(kind, dimensions, p + vec3<f32>(e, 0.0, 0.0), fp, seed, octaves, lacunarity, gain);
    let b = sc_noise_octaves(kind, dimensions, p + vec3<f32>(0.0, e, 0.0), fp, seed, octaves, lacunarity, gain);
    return sc_pattern_of(t, m, vec2<f32>(a - m, b - m) / (e * s));
}

// `pattern.noise` over a position `p` (mm) with footprint `fp` (mm): its gradient is spatial,
// and its slope is the gradient's x and y.
fn sc_pattern_noise3(p: vec3<f32>, fp: f32, kind: u32, dimensions: u32, octaves: u32, scale_mm: f32, lacunarity: f32, gain: f32, seed: u32, slope: bool) -> sc_Pattern {
    let s = max(scale_mm, 1e-6);
    let q = p / s;
    let f = fp / s;
    let m = sc_noise_octaves(kind, dimensions, q, f, seed, octaves, lacunarity, gain);
    let x = vec3<f32>(1.0, 0.0, 0.0);
    if (!slope) {
        return sc_Pattern(m, vec2<f32>(0.0), vec3<f32>(0.0), x);
    }
    let e = 0.05;
    let g = vec3<f32>(
        sc_noise_octaves(kind, dimensions, q + vec3<f32>(e, 0.0, 0.0), f, seed, octaves, lacunarity, gain),
        sc_noise_octaves(kind, dimensions, q + vec3<f32>(0.0, e, 0.0), f, seed, octaves, lacunarity, gain),
        sc_noise_octaves(kind, dimensions, q + vec3<f32>(0.0, 0.0, e), f, seed, octaves, lacunarity, gain)) - vec3<f32>(m);
    let grad = g / (e * s);
    return sc_Pattern(m, grad.xy, grad, x);
}

// Brushing lines along u: value noise stretched to `streak_mm` along and 1 / `density_per_mm`
// across, with `jitter` mixing in a finer, shorter variation; in [0, 1] about 0.5 by `contrast`.
fn sc_brushed_at(uv: vec2<f32>, across: f32, along: f32, contrast: f32, jitter: f32, seed: u32) -> f32 {
    let coarse = sc_noise(0u, 2u, vec3<f32>(uv.x / along, uv.y / across, 0.0), seed);
    let fine = sc_noise(0u, 2u, vec3<f32>(uv.x / (0.25 * along), uv.y / across + 0.5, 0.0), seed + 1u);
    return 0.5 + contrast * (mix(coarse, fine, 0.5 * clamp(jitter, 0.0, 1.0)) - 0.5);
}

fn sc_pattern_brushed(t: sc_Texcoord, density_per_mm: f32, streak_mm: f32, contrast: f32, jitter: f32, seed: u32, slope: bool) -> sc_Pattern {
    let across = 1.0 / max(density_per_mm, 1e-6);
    let along = max(streak_mm, 1e-6);
    let fade = sc_fade(t.footprint, across);
    let m = sc_brushed_at(t.uv, across, along, contrast * fade, jitter, seed);
    if (!slope) {
        return sc_pattern_of(t, m, vec2<f32>(0.0));
    }
    let ex = 0.05 * along;
    let ey = 0.05 * across;
    let a = sc_brushed_at(t.uv + vec2<f32>(ex, 0.0), across, along, contrast * fade, jitter, seed);
    let b = sc_brushed_at(t.uv + vec2<f32>(0.0, ey), across, along, contrast * fade, jitter, seed);
    return sc_pattern_of(t, m, vec2<f32>((a - m) / ex, (b - m) / ey));
}

// A yarn bundle's crown at `t` across it (−0.5..0.5 is its width): elliptic at `crown` 1, flat
// at 0.
fn sc_weave_crown(t: f32, crown: f32) -> f32 {
    let square = select(0.0, 1.0, abs(t) < 0.5);
    return mix(square, sqrt(max(1.0 - 4.0 * t * t, 0.0)), crown);
}

// A weave at `p` (in yarn pitches): warp yarns along x, weft along y, each rising over and
// dipping under the crossings (plain: over one, under one; twill: over two, under two, shifted
// one a row). The visible yarn is the higher; returns its height and its filament striations.
fn sc_weave_parts(p: vec2<f32>, crown: f32, fibres: f32, twill: bool) -> vec2<f32> {
    let cell = floor(p);
    let f = p - cell - vec2<f32>(0.5);
    let pi = 3.141592653589793;
    let rate = select(pi, 0.5 * pi, twill);
    let shift = select(0.5, 1.0, twill);
    let up_x = 0.5 + 0.5 * cos(rate * (p.x - shift + cell.y));
    let up_y = 0.5 - 0.5 * cos(rate * (p.y - shift + cell.x));
    let hx = sc_weave_crown(f.y, crown) * (0.6 + 0.4 * up_x);
    let hy = sc_weave_crown(f.x, crown) * (0.6 + 0.4 * up_y);
    let across = select(f.x, f.y, hx >= hy);
    let fibre = cos(2.0 * pi * fibres * across) * sc_weave_crown(across, 1.0);
    return vec2<f32>(max(hx, hy), fibre);
}

// The mean visible height of `sc_weave_parts` over a repeat, fitted through crown 0, ½ and 1.
fn sc_weave_mean(crown: f32, twill: bool) -> f32 {
    let c = clamp(crown, 0.0, 1.0);
    return select(0.92733 + c * (-0.20576 + c * 0.05744), 0.92733 + c * (-0.20964 + c * 0.05284), twill);
}

fn sc_weave_at(uv: vec2<f32>, pitch: vec2<f32>, crown: f32, fibres: f32, fibre_amount: f32, fade: f32, twill: bool) -> f32 {
    let parts = sc_weave_parts(uv / pitch, crown, fibres, twill);
    return clamp(0.5 + fade * (parts.x - sc_weave_mean(crown, twill) + fibre_amount * parts.y), 0.0, 1.0);
}

// `pattern.weave`: in [0, 1], mean 0.5; `fibre_contrast` of the striations show until they near
// the footprint.
fn sc_pattern_weave(t: sc_Texcoord, pitch_mm: vec2<f32>, crown: f32, fibres: f32, fibre_contrast: f32, twill: bool, slope: bool) -> sc_Pattern {
    let pitch = max(abs(pitch_mm), vec2<f32>(1e-4));
    let period = min(pitch.x, pitch.y);
    let fade = sc_fade_sharp(t.footprint, period);
    let fibre_amount = fibre_contrast * sc_fade_sharp(t.footprint, period / max(fibres, 1.0));
    let m = sc_weave_at(t.uv, pitch, crown, fibres, fibre_amount, fade, twill);
    if (!slope) {
        return sc_pattern_of(t, m, vec2<f32>(0.0));
    }
    let e = 0.05 * pitch;
    let a = sc_weave_at(t.uv + vec2<f32>(e.x, 0.0), pitch, crown, fibres, fibre_amount, fade, twill);
    let b = sc_weave_at(t.uv + vec2<f32>(0.0, e.y), pitch, crown, fibres, fibre_amount, fade, twill);
    return sc_pattern_of(t, m, vec2<f32>((a - m) / e.x, (b - m) / e.y));
}

// The integral of a unit square wave (1 on odd cells) from 0 to `x`.
fn sc_square_integral(x: f32) -> f32 {
    let h = 0.5 * x;
    return floor(h) + max(2.0 * fract(h) - 1.0, 0.0);
}

// A unit square wave at `x`, box-filtered over `w` (same units).
fn sc_square_filtered(x: f32, w: f32) -> f32 {
    let width = max(w, 1e-6);
    return clamp((sc_square_integral(x + 0.5 * width) - sc_square_integral(x - 0.5 * width)) / width, 0.0, 1.0);
}

// `pattern.checker`: 0 or 1 per cell of `size_mm`, box-filtered over the footprint (mean 0.5).
fn sc_pattern_checker(t: sc_Texcoord, size_mm: vec2<f32>) -> sc_Pattern {
    let size = max(abs(size_mm), vec2<f32>(1e-6));
    let p = t.uv / size;
    let a = sc_square_filtered(p.x, t.footprint / size.x);
    let b = sc_square_filtered(p.y, t.footprint / size.y);
    return sc_pattern_of(t, a + b - 2.0 * a * b, vec2<f32>(0.0));
}

// The integral from 0 to `x` of lines of width `d` (a fraction of a cell) centred on integers.
fn sc_lines_integral(x: f32, d: f32) -> f32 {
    let y = x + 0.5 * d;
    return floor(y) * d + min(fract(y), d);
}

// `pattern.grid`: 1 on lines `line_mm` wide every `size_mm`, box-filtered over the footprint.
fn sc_pattern_grid(t: sc_Texcoord, size_mm: vec2<f32>, line_mm: f32) -> sc_Pattern {
    let size = max(abs(size_mm), vec2<f32>(1e-6));
    let p = t.uv / size;
    let d = clamp(vec2<f32>(line_mm) / size, vec2<f32>(0.0), vec2<f32>(1.0));
    let w = max(vec2<f32>(t.footprint) / size, vec2<f32>(1e-6));
    let a = clamp((sc_lines_integral(p.x + 0.5 * w.x, d.x) - sc_lines_integral(p.x - 0.5 * w.x, d.x)) / w.x, 0.0, 1.0);
    let b = clamp((sc_lines_integral(p.y + 0.5 * w.y, d.y) - sc_lines_integral(p.y - 0.5 * w.y, d.y)) / w.y, 0.0, 1.0);
    return sc_pattern_of(t, a + b - a * b, vec2<f32>(0.0));
}

// `pattern.gradient`: a ramp from `start` to `end` (uv units) along u, or over the distance from
// the uv origin, clamped to [0, 1].
fn sc_pattern_gradient(t: sc_Texcoord, start: f32, end: f32, radial: bool) -> sc_Pattern {
    let span = end - start;
    let width = select(min(span, -1e-6), max(span, 1e-6), span >= 0.0);
    let r = length(t.uv);
    let x = select(t.uv.x, r, radial);
    let along = select(vec2<f32>(1.0, 0.0), t.uv / max(r, 1e-6), radial);
    let ramp = (x - start) / width;
    let inside = ramp > 0.0 && ramp < 1.0;
    return sc_pattern_of(t, clamp(ramp, 0.0, 1.0), select(vec2<f32>(0.0), along / width, inside));
}

// `normal.from_height`: the unit normal `n` tilted against the height's spatial gradient `grad`
// (per mm) scaled by `relief_mm`.
fn sc_normal_from_height(n: vec3<f32>, grad: vec3<f32>, relief_mm: f32) -> vec3<f32> {
    let g = grad * relief_mm;
    return sc_proj_unit(n - (g - n * dot(g, n)), n);
}

// `normal.map`: a tangent-space normal (`color` in [0, 1], OpenGL convention, or DirectX with
// green flipped) in the frame of `tangent` about `n`, its tilt scaled by `strength`.
fn sc_normal_map(n: vec3<f32>, tangent: vec3<f32>, color: vec3<f32>, strength: f32, directx: bool) -> vec3<f32> {
    let t = sc_proj_unit(tangent - n * dot(tangent, n), sc_default_tangent(n));
    let b = cross(n, t);
    var sc_m = color * 2.0 - vec3<f32>(1.0);
    sc_m.y = select(sc_m.y, -sc_m.y, directx);
    let tilted = sc_m.x * t + sc_m.y * b + max(sc_m.z, 0.0) * n;
    return sc_proj_unit(mix(n, sc_proj_unit(tilted, n), clamp(strength, 0.0, 1.0)), n);
}

// `normal.blend`: the normalised lerp of `a` and `b`.
fn sc_normal_blend(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return sc_proj_unit(mix(a, b, clamp(t, 0.0, 1.0)), a);
}

// Maths nodes whose formula is more than an operator.

// A denominator held at least 1e-6 from zero, keeping its sign.
fn sc_denominator(b: f32) -> f32 {
    return select(min(b, -1e-6), max(b, 1e-6), b >= 0.0);
}

// Rec. 709 luminance of a linear colour.
fn sc_luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// `normalize` of a vector; zero gives +Z (or +X for a vector2).
fn sc_normalize3(v: vec3<f32>) -> vec3<f32> {
    return sc_proj_unit(v, vec3<f32>(0.0, 0.0, 1.0));
}

fn sc_normalize2(v: vec2<f32>) -> vec2<f32> {
    let l = length(v);
    return select(vec2<f32>(1.0, 0.0), v / l, l > 1e-20);
}

fn sc_rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let high = max(max(c.r, c.g), c.b);
    let low = min(min(c.r, c.g), c.b);
    let d = high - low;
    let safe = max(d, 1e-20);
    var sc_h = 0.0;
    if (d > 0.0) {
        if (high == c.r) {
            sc_h = (c.g - c.b) / safe;
        } else if (high == c.g) {
            sc_h = 2.0 + (c.b - c.r) / safe;
        } else {
            sc_h = 4.0 + (c.r - c.g) / safe;
        }
    }
    return vec3<f32>(fract(sc_h / 6.0 + 1.0), select(0.0, d / high, high > 0.0), high);
}

fn sc_hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let q = (k + vec3<f32>(c.x * 6.0)) - 6.0 * floor((k + vec3<f32>(c.x * 6.0)) / 6.0);
    return c.z - c.z * c.y * clamp(min(q, vec3<f32>(4.0) - q), vec3<f32>(0.0), vec3<f32>(1.0));
}

// `hsv_adjust`: turn the hue by `hue` radians and scale saturation and value.
fn sc_hsv_adjust(c: vec3<f32>, hue: f32, saturation: f32, value: f32) -> vec3<f32> {
    let hsv = sc_rgb_to_hsv(max(c, vec3<f32>(0.0)));
    let h = fract(hsv.x + hue / 6.283185307179586 + 1.0);
    return sc_hsv_to_rgb(vec3<f32>(h, clamp(hsv.y * saturation, 0.0, 1.0), hsv.z * value));
}

// A surface input as the surface node admits it: clamped to [lo, hi], NaN replaced by `fallback`.
fn sc_clean(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    let nan = (bitcast<u32>(x) & 0x7fffffffu) > 0x7f800000u;
    return select(clamp(x, lo, hi), fallback, nan);
}

fn sc_clean3(x: vec3<f32>, lo: f32, hi: f32, fallback: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(sc_clean(x.x, lo, hi, fallback.x), sc_clean(x.y, lo, hi, fallback.y), sc_clean(x.z, lo, hi, fallback.z));
}

// A unit direction, NaN or zero replaced by `fallback`.
fn sc_clean_unit(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let bits = bitcast<vec3<u32>>(v) & vec3<u32>(0x7fffffffu);
    let bad = any(bits >= vec3<u32>(0x7f800000u));
    return select(sc_proj_unit(v, fallback), fallback, bad);
}
