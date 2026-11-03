// Shared shading library: patterns (crates/gpu/src/shading/mod.rs). Pure functions, `sc_` names,
// WebGL2-safe. The pattern, normal and maths nodes of a material graph (docs/material-graph.md
// §3.3, §3.4), as the test-only reference (`material_graph::reference`, track T) defines them.
// Every pattern fades toward its mean as its period nears the pixel footprint, so no graph
// aliases into moiré; a footprint of zero (the path tracer's) shows it unfiltered. Slopes are
// forward differences with a step of `0.05` of the pattern's period along each axis.

// A pattern's outputs on a texcoord: its value, its slope along u and v (per uv unit) and the
// same slope as a world gradient (per mm).
struct sc_Pattern {
    value: f32,
    slope: vec2<f32>,
    grad: vec3<f32>,
};

// How much of a pattern of period `period` shows at a footprint `fp` (same units).
fn sc_fade(fp: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.2, 0.6, fp / max(period, 1e-6));
}

// The weave's sharper fade: its cells have sharp edges and its fibres are fine.
fn sc_fade_sharp(fp: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.1, 0.3, fp / max(period, 1e-6));
}

// A pattern from its value at uv (`m`) and at uv + (h.x, 0) and uv + (0, h.y) (`a`, `b`).
fn sc_pattern_of(t: sc_Texcoord, m: f32, a: f32, b: f32, h: vec2<f32>) -> sc_Pattern {
    let slope = vec2<f32>((a - m) / h.x, (b - m) / h.y);
    return sc_Pattern(m, slope, slope.x * t.du + slope.y * t.dv);
}

fn sc_pattern_flat(m: f32) -> sc_Pattern {
    return sc_Pattern(m, vec2<f32>(0.0), vec3<f32>(0.0));
}

// Three planar evaluations of a triplanar pattern, blended by the weights `w`.
fn sc_pattern_blend(a: sc_Pattern, b: sc_Pattern, c: sc_Pattern, w: vec3<f32>) -> sc_Pattern {
    return sc_Pattern(w.x * a.value + w.y * b.value + w.z * c.value, w.x * a.slope + w.y * b.slope + w.z * c.slope,
        w.x * a.grad + w.y * b.grad + w.z * c.grad);
}

// `pattern.noise` in 2D at `uv`: `sc_fbm` at (uv / scale_mm, 0), its deviation from 0.5 scaled by `k`.
fn sc_noise2_at(uv: vec2<f32>, k: f32, kind: u32, octaves: u32, scale_mm: f32, lacunarity: f32, gain: f32, seed: u32) -> f32 {
    return 0.5 + k * (sc_fbm(kind, 2u, vec3<f32>(uv / scale_mm, 0.0), seed, octaves, lacunarity, gain) - 0.5);
}

// `pattern.noise` over a texcoord, faded by `sc_fade(footprint, scale_mm)`; with `slope`, its slope.
fn sc_pattern_noise(t: sc_Texcoord, kind: u32, octaves: u32, scale_mm: f32, lacunarity: f32, gain: f32, seed: u32, slope: bool) -> sc_Pattern {
    let k = sc_fade(sc_tc_footprint(t), scale_mm);
    let m = sc_noise2_at(t.uv, k, kind, octaves, scale_mm, lacunarity, gain, seed);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = 0.05 * scale_mm;
    let a = sc_noise2_at(t.uv + vec2<f32>(h, 0.0), k, kind, octaves, scale_mm, lacunarity, gain, seed);
    let b = sc_noise2_at(t.uv + vec2<f32>(0.0, h), k, kind, octaves, scale_mm, lacunarity, gain, seed);
    return sc_pattern_of(t, m, a, b, vec2<f32>(h));
}

fn sc_noise3_at(q: vec3<f32>, k: f32, kind: u32, octaves: u32, scale_mm: f32, lacunarity: f32, gain: f32, seed: u32) -> f32 {
    return 0.5 + k * (sc_fbm(kind, 3u, q / scale_mm, seed, octaves, lacunarity, gain) - 0.5);
}

// `pattern.noise` in 3D at a position `q` (mm) with footprint `fp` (mm): `sc_fbm` at q / scale_mm,
// faded by `sc_fade(fp, scale_mm)`. `grad` is along the position's own axes (per mm); its slope
// output is zero.
fn sc_pattern_noise3(q: vec3<f32>, fp: f32, kind: u32, octaves: u32, scale_mm: f32, lacunarity: f32, gain: f32, seed: u32, slope: bool) -> sc_Pattern {
    let k = sc_fade(fp, scale_mm);
    let m = sc_noise3_at(q, k, kind, octaves, scale_mm, lacunarity, gain, seed);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = 0.05 * scale_mm;
    let g = vec3<f32>(
        sc_noise3_at(q + vec3<f32>(h, 0.0, 0.0), k, kind, octaves, scale_mm, lacunarity, gain, seed),
        sc_noise3_at(q + vec3<f32>(0.0, h, 0.0), k, kind, octaves, scale_mm, lacunarity, gain, seed),
        sc_noise3_at(q + vec3<f32>(0.0, 0.0, h), k, kind, octaves, scale_mm, lacunarity, gain, seed)) - vec3<f32>(m);
    return sc_Pattern(m, vec2<f32>(0.0), g / h);
}

// `pattern.brushed` at `uv`: p = (u / streak_mm, v · density_per_mm),
// mix(value noise at p, value noise at p·(0.5, 3) + (19, 7) with seed + 1, jitter), its
// contrast about 0.5 scaled by `k`.
fn sc_brushed_at(uv: vec2<f32>, k: f32, density: f32, streak: f32, contrast: f32, jitter: f32, seed: u32) -> f32 {
    let p = vec2<f32>(uv.x / streak, uv.y * density);
    let a = sc_value_noise2(p, seed);
    let b = sc_value_noise2(p * vec2<f32>(0.5, 3.0) + vec2<f32>(19.0, 7.0), seed + 1u);
    return 0.5 + k * (contrast * (a + (b - a) * jitter - 0.5));
}

// `pattern.brushed`: faded by `sc_fade(footprint, 1 / density_per_mm)`.
fn sc_pattern_brushed(t: sc_Texcoord, density: f32, streak: f32, contrast: f32, jitter: f32, seed: u32, slope: bool) -> sc_Pattern {
    let k = sc_fade(sc_tc_footprint(t), 1.0 / density);
    let m = sc_brushed_at(t.uv, k, density, streak, contrast, jitter, seed);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = 0.05 * vec2<f32>(streak, 1.0 / density);
    let a = sc_brushed_at(t.uv + vec2<f32>(h.x, 0.0), k, density, streak, contrast, jitter, seed);
    let b = sc_brushed_at(t.uv + vec2<f32>(0.0, h.y), k, density, streak, contrast, jitter, seed);
    return sc_pattern_of(t, m, a, b, h);
}

// A yarn's crown at `t` across it, flattened by `crown`: mix(1, sqrt(max(1 − 4t², 0)), crown).
fn sc_weave_crown(t: f32, crown: f32) -> f32 {
    return 1.0 + (sqrt(max(1.0 - 4.0 * t * t, 0.0)) - 1.0) * crown;
}

// The weave's raw height at `p` (in pitches): warp along x, weft along y, each over and under the
// crossings (plain: one over, one under; twill: two and two, shifted one a row); the visible yarn
// is the higher, with filament striations across it.
fn sc_weave_raw(p: vec2<f32>, twill: bool, crown: f32, fibres: f32, fibre_contrast: f32) -> f32 {
    let pi = 3.141592653589793;
    let cell = floor(p);
    let f = p - cell - vec2<f32>(0.5);
    let plain_x = 0.5 + 0.5 * cos(pi * (p.x - 0.5 + cell.y));
    let plain_y = 0.5 - 0.5 * cos(pi * (p.y - 0.5 + cell.x));
    let twill_x = clamp(0.5 + 0.7071067811865476 * cos(0.5 * pi * (p.x - 0.5 + cell.y) - 0.25 * pi), 0.0, 1.0);
    let twill_y = clamp(0.5 - 0.7071067811865476 * cos(0.5 * pi * (p.y - 0.5 + cell.x) - 0.25 * pi), 0.0, 1.0);
    let up_x = select(plain_x, twill_x, twill);
    let up_y = select(plain_y, twill_y, twill);
    let hx = sc_weave_crown(f.y, crown) * (0.6 + 0.4 * up_x);
    let hy = sc_weave_crown(f.x, crown) * (0.6 + 0.4 * up_y);
    let across = select(f.x, f.y, hx >= hy);
    let fibre = 1.0 - fibre_contrast * 0.5 * (1.0 - cos(2.0 * pi * fibres * across));
    return max(hx, hy) * fibre;
}

fn sc_weave_at(uv: vec2<f32>, k: f32, pitch: vec2<f32>, twill: bool, crown: f32, fibres: f32, fc: f32) -> f32 {
    return 0.5 + k * (sc_weave_raw(uv / pitch, twill, crown, fibres, fc) - 0.5);
}

// `pattern.weave`: faded by `sc_fade_sharp(footprint, min(pitch))`, its fibres by
// `sc_fade_sharp(footprint, min(pitch) / max(fibres, 1))`.
fn sc_pattern_weave(t: sc_Texcoord, pitch: vec2<f32>, crown: f32, fibres: f32, fibre_contrast: f32, twill: bool, slope: bool) -> sc_Pattern {
    let fp = sc_tc_footprint(t);
    let period = min(pitch.x, pitch.y);
    let k = sc_fade_sharp(fp, period);
    let fc = fibre_contrast * sc_fade_sharp(fp, period / max(fibres, 1.0));
    let m = sc_weave_at(t.uv, k, pitch, twill, crown, fibres, fc);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = 0.05 * pitch;
    let a = sc_weave_at(t.uv + vec2<f32>(h.x, 0.0), k, pitch, twill, crown, fibres, fc);
    let b = sc_weave_at(t.uv + vec2<f32>(0.0, h.y), k, pitch, twill, crown, fibres, fc);
    return sc_pattern_of(t, m, a, b, h);
}

// One axis of the checker: the square wave (+1 on even cells, −1 on odd ones) box-filtered over
// `w` cells where one edge is inside the box, sign · min(2d / w, 1), d the distance to the
// nearest edge in cells (no differences of nearby values, so f32 keeps its precision).
fn sc_checker_axis(t: f32, w: f32) -> f32 {
    let x = t * 0.5 - floor(t * 0.5);
    let sign = select(-1.0, 1.0, x < 0.5);
    let d = 2.0 * min(min(x, abs(x - 0.5)), 1.0 - x);
    return sign * min(2.0 * d / w, 1.0);
}

fn sc_checker_at(uv: vec2<f32>, size: vec2<f32>, w: vec2<f32>, k: f32) -> f32 {
    return 0.5 - 0.5 * sc_checker_axis(uv.x / size.x, w.x) * sc_checker_axis(uv.y / size.y, w.y) * k;
}

// `pattern.checker`: 0 on cells whose index sum is even, 1 on odd ones, faded by
// `sc_fade(footprint, min(size))`.
fn sc_pattern_checker(t: sc_Texcoord, size: vec2<f32>, slope: bool) -> sc_Pattern {
    let fp = sc_tc_footprint(t);
    let w = max(vec2<f32>(fp) / size, vec2<f32>(1e-6));
    let k = sc_fade(fp, min(size.x, size.y));
    let m = sc_checker_at(t.uv, size, w, k);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = 0.05 * size;
    return sc_pattern_of(t, m, sc_checker_at(t.uv + vec2<f32>(h.x, 0.0), size, w, k), sc_checker_at(t.uv + vec2<f32>(0.0, h.y), size, w, k), h);
}

fn sc_grid_axis(x: f32, s: f32, half: f32, w: f32) -> f32 {
    let t = x / s + 0.5;
    let d = abs(t - floor(t) - 0.5) * s;
    return 1.0 - smoothstep(half - w, half + w, d);
}

fn sc_grid_at(uv: vec2<f32>, size: vec2<f32>, half: f32, w: f32, mean: f32, k: f32) -> f32 {
    return mean + k * (max(sc_grid_axis(uv.x, size.x, half, w), sc_grid_axis(uv.y, size.y, half, w)) - mean);
}

// `pattern.grid`: 1 within line_mm / 2 of a line u = i·size.x or v = j·size.y, edges smoothed
// over half the footprint, faded toward the covered fraction by `sc_fade(footprint, min(size))`.
fn sc_pattern_grid(t: sc_Texcoord, size: vec2<f32>, line_mm: f32, slope: bool) -> sc_Pattern {
    let fp = sc_tc_footprint(t);
    let half = 0.5 * line_mm;
    let w = 0.5 * max(fp, 1e-6);
    let covered = clamp(vec2<f32>(line_mm) / size, vec2<f32>(0.0), vec2<f32>(1.0));
    let mean = 1.0 - (1.0 - covered.x) * (1.0 - covered.y);
    let k = sc_fade(fp, min(size.x, size.y));
    let m = sc_grid_at(t.uv, size, half, w, mean, k);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = 0.05 * size;
    return sc_pattern_of(t, m, sc_grid_at(t.uv + vec2<f32>(h.x, 0.0), size, half, w, mean, k), sc_grid_at(t.uv + vec2<f32>(0.0, h.y), size, half, w, mean, k), h);
}

fn sc_gradient_at(uv: vec2<f32>, start: f32, den: f32, radial: bool) -> f32 {
    return clamp((select(uv.x, length(uv), radial) - start) / den, 0.0, 1.0);
}

// `pattern.gradient`: (u − start) / (end − start), or (|uv| − start) / (end − start) for
// `radial`, the denominator held 1e-6 from zero, clamped to [0, 1]. No fade.
fn sc_pattern_gradient(t: sc_Texcoord, start: f32, end: f32, radial: bool, slope: bool) -> sc_Pattern {
    let den = sc_denominator(end - start);
    let m = sc_gradient_at(t.uv, start, den, radial);
    if (!slope) {
        return sc_pattern_flat(m);
    }
    let h = vec2<f32>(0.05 * max(abs(end - start), 1e-6));
    return sc_pattern_of(t, m, sc_gradient_at(t.uv + vec2<f32>(h.x, 0.0), start, den, radial), sc_gradient_at(t.uv + vec2<f32>(0.0, h.y), start, den, radial), h);
}

// `normal.from_height`: sc_unit(n − relief_mm · tangential(grad)), n the geometric normal and
// `grad` the height's world gradient (per mm).
fn sc_normal_from_height(n: vec3<f32>, grad: vec3<f32>, relief_mm: f32) -> vec3<f32> {
    return sc_unit(n - relief_mm * (grad - n * dot(grad, n)));
}

// `normal.map`: t = 2·color − 1 (y negated for DirectX), its x and y scaled by `strength`, in the
// frame of `tangent`, n × tangent and n.
fn sc_normal_map(n: vec3<f32>, tangent: vec3<f32>, color: vec3<f32>, strength: f32, directx: bool) -> vec3<f32> {
    let t = color * 2.0 - vec3<f32>(1.0);
    let y = select(t.y, -t.y, directx);
    return sc_unit(tangent * (t.x * strength) + cross(n, tangent) * (y * strength) + n * t.z);
}

// `normal.blend`: the normalised lerp of `a` and `b`.
fn sc_normal_blend(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return sc_unit(mix(a, b, vec3<f32>(t)));
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

// `normalize` of a vector (`sc_unit`): zero gives +Z.
fn sc_normalize3(v: vec3<f32>) -> vec3<f32> {
    return sc_unit(v);
}

// A vector2 is padded with a zero z, so zero stays zero.
fn sc_normalize2(v: vec2<f32>) -> vec2<f32> {
    return sc_unit(vec3<f32>(v, 0.0)).xy * select(0.0, 1.0, any(v != vec2<f32>(0.0)));
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
    let hsv = sc_rgb_to_hsv(c);
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

// A surface normal input: NaN components replaced by `fallback`'s, then `sc_unit`; `fallback`
// itself if a component is infinite.
fn sc_clean_normal(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let bits = bitcast<vec3<u32>>(v) & vec3<u32>(0x7fffffffu);
    let w = select(v, fallback, bits > vec3<u32>(0x7f800000u));
    return select(sc_unit(w), fallback, any(bits == vec3<u32>(0x7f800000u)));
}

// A surface tangent input: made tangential to unit `n` and normalised; `fallback` where that
// vanishes or is not finite.
fn sc_clean_tangent(t: vec3<f32>, n: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let g = t - n * dot(t, n);
    let bits = bitcast<vec3<u32>>(g) & vec3<u32>(0x7fffffffu);
    return select(sc_unit(g), fallback, length(g) < 1e-12 || any(bits >= vec3<u32>(0x7f800000u)));
}

// Material-graph evaluation, shared by both GPU targets' generated code (crates/gpu/src/material/
// codegen.rs): its input, its output and the helpers it calls.

// What a material is evaluated at: the record (vec4 index of its uniform data), the unit
// direction toward the eye, the unit geometric normal in world and object space, the geometric
// tangent (unit, perpendicular to `n`; an unconnected `geometry_tangent`, and the fallback where
// a projection defines none), the facing |n·v|, the position (mm) in both spaces, the pixel
// footprint in world mm (0: unfiltered) and the object frame.
struct sc_GraphCtx {
    record: u32,
    v: vec3<f32>,
    n: vec3<f32>,
    tangent: vec3<f32>,
    facing: f32,
    n_object: vec3<f32>,
    p_world: vec3<f32>,
    p_object: vec3<f32>,
    fp_world: f32,
    frame: sc_ObjectFrame,
};

// One evaluated surface point: the WGSL mirror of `material_graph::SurfaceSample` (the
// `OpaqueSurface` core, its clear coat and the inputs the core lacks), with its shading frame.
// Colours are linear.
struct sc_Surface {
    base_color: vec3<f32>,
    metalness: f32,
    // Perceptual.
    roughness: f32,
    // The dielectric base's; 1: no interface of its own.
    ior: f32,
    // 0: no coat.
    coat_weight: f32,
    coat_roughness: f32,
    // 1: no coat interface.
    coat_ior: f32,
    // Unit shading normals of the base and of the coat.
    normal: vec3<f32>,
    coat_normal: vec3<f32>,
    specular_weight: f32,
    specular_color: vec3<f32>,
    // 0: isotropic. Stretched along `tangent` (unit; zero while isotropic), turned by
    // `rotation` radians about the normal.
    anisotropy: f32,
    rotation: f32,
    tangent: vec3<f32>,
    // Tint of light through the coat.
    coat_color: vec3<f32>,
    // Relative luminance, like the product lights.
    emission_luminance: f32,
    emission_color: vec3<f32>,
    transmission_weight: f32,
    transmission_color: vec3<f32>,
    transmission_depth_mm: f32,
    // Cut-out coverage, not glass.
    opacity: f32,
};

// An `image` node's outputs.
struct sc_Image { color: vec3<f32>, value: f32, alpha: f32, };

// An image with no texture (none bound, or not decoded yet, §5.6): its `default` colour with no
// coverage (alpha 0), so a mask read through its alpha shows no mark; `value` is `channel`'s (0
// the Rec. 709 luminance, 1–3 r, g, b, 4 alpha).
fn sc_image_default(color: vec3<f32>, channel: u32) -> sc_Image {
    let picked = select(select(select(sc_luminance(color), color.r, channel == 1u), color.g, channel == 2u), color.b, channel == 3u);
    return sc_Image(color, select(picked, 0.0, channel == 4u), 0.0);
}

// A sampled texel's image outputs: its colour, `channel`'s value (0 the Rec. 709 luminance, 1–3
// r, g, b, 4 alpha) and its alpha.
fn sc_image_of(rgba: vec4<f32>, channel: u32) -> sc_Image {
    let picked = select(select(select(select(sc_luminance(rgba.rgb), rgba.r, channel == 1u), rgba.g, channel == 2u), rgba.b, channel == 3u), rgba.a, channel == 4u);
    return sc_Image(rgba.rgb, picked, rgba.a);
}

// Three planar samples of a triplanar image, blended by the weights `w`.
fn sc_image_blend(a: sc_Image, b: sc_Image, c: sc_Image, w: vec3<f32>) -> sc_Image {
    return sc_Image(w.x * a.color + w.y * b.color + w.z * c.color, w.x * a.value + w.y * b.value + w.z * c.value,
        w.x * a.alpha + w.y * b.alpha + w.z * c.alpha);
}

// An unconnected texcoord: the origin, with no gradient.
fn sc_texcoord_none() -> sc_Texcoord {
    return sc_Texcoord(vec2<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), 0.0);
}

// `surface.mix` in v1 (decision O3): the two surfaces' parameters blended, `t` = 1 being `fg`;
// normals and the tangent by normalised lerp.
fn sc_surface_mix(bg: sc_Surface, fg: sc_Surface, t: f32) -> sc_Surface {
    return sc_Surface(
        mix(bg.base_color, fg.base_color, vec3<f32>(t)),
        mix(bg.metalness, fg.metalness, t),
        mix(bg.roughness, fg.roughness, t),
        mix(bg.ior, fg.ior, t),
        mix(bg.coat_weight, fg.coat_weight, t),
        mix(bg.coat_roughness, fg.coat_roughness, t),
        mix(bg.coat_ior, fg.coat_ior, t),
        sc_unit(mix(bg.normal, fg.normal, vec3<f32>(t))),
        sc_unit(mix(bg.coat_normal, fg.coat_normal, vec3<f32>(t))),
        mix(bg.specular_weight, fg.specular_weight, t),
        mix(bg.specular_color, fg.specular_color, vec3<f32>(t)),
        mix(bg.anisotropy, fg.anisotropy, t),
        mix(bg.rotation, fg.rotation, t),
        sc_unit(mix(bg.tangent, fg.tangent, vec3<f32>(t))),
        mix(bg.coat_color, fg.coat_color, vec3<f32>(t)),
        mix(bg.emission_luminance, fg.emission_luminance, t),
        mix(bg.emission_color, fg.emission_color, vec3<f32>(t)),
        mix(bg.transmission_weight, fg.transmission_weight, t),
        mix(bg.transmission_color, fg.transmission_color, vec3<f32>(t)),
        mix(bg.transmission_depth_mm, fg.transmission_depth_mm, t),
        mix(bg.opacity, fg.opacity, t));
}

// The evaluated surface as returned: the tangent is zero while the surface is isotropic.
fn sc_surface_finish(s: sc_Surface) -> sc_Surface {
    var sc_out = s;
    sc_out.tangent = select(s.tangent, vec3<f32>(0.0), s.anisotropy == 0.0);
    return sc_out;
}
