//! Original GPU translation of the L1 opaque surface and fixed studio reference.
//! Appended to the existing viewport shader so camera/output conventions are shared.

/// A PBR vertex: position, normal, linear base colour and (metallic, roughness, IOR), three
/// floats each. See `GpuScene::pbr_tri`.
pub const VERTEX_SIZE: usize = 48;

/// A PBR vertex with the product look's detail ([`VERTEX_SIZE`] bytes, then four floats: pattern
/// id + 0.25 + (coat IOR − 1) / 4, pattern strength, coat weight, coat roughness). Only scenes
/// that name a pattern or a coat carry them. See `GpuScene::pbr_tri_with`.
pub const DETAILED_VERTEX_SIZE: usize = 64;

/// The PBR pipeline's bind group for the product look's studio (`view::ibl`).
pub const STUDIO_GROUP: u32 = 3;

pub const SOURCE: &str = r#"
// The product look's studio (`view::ibl`, group 3): the prefiltered radiance cube (level m at
// perceptual roughness m / levels.x), its sampler, and the irradiance polynomial over
// (1, x, y, z, xy, yz, xz, z², x² − y²). Read only while `look_on()`.
@group(3) @binding(0) var studio_cube: texture_cube<f32>;
@group(3) @binding(1) var studio_smp: sampler;
struct Studio { irradiance: array<vec4<f32>, 9>, levels: vec4<f32>, };
@group(3) @binding(2) var<uniform> studio: Studio;
struct PbrOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) n: vec3<f32>,
    @location(1) base: vec3<f32>,
    @location(2) wp: vec3<f32>,
    @location(3) parameters: vec3<f32>,
    // Flat: the coat's IOR rides in the fraction of the pattern float and is compared exactly,
    // so it must not be interpolated (a triangle's three vertices carry the same detail).
    @location(4) @interpolate(flat, either) detail: vec4<f32>,
};

fn pbr_unit(v: vec3<f32>) -> vec3<f32> {
    let scale = max(max(abs(v.x), abs(v.y)), abs(v.z));
    if (scale == 0.0) { return vec3<f32>(0.0, 0.0, 1.0); }
    let s = v / scale;
    return s / length(s);
}

fn pbr_encode(c: vec3<f32>) -> vec3<f32> {
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055), c > vec3<f32>(0.0031308));
}

fn pbr_out(p: vec3<f32>, n: vec3<f32>, base: vec3<f32>, parameters: vec3<f32>, detail: vec4<f32>) -> PbrOut {
    var o: PbrOut;
    o.pos = u.vp * vec4<f32>(p, 1.0);
    o.n = n;
    o.base = base;
    o.wp = p;
    o.parameters = parameters;
    o.detail = detail;
    return o;
}

// A plain PBR vertex (`pbr::VERTEX_SIZE`): no pattern (id 0) and no coat (weight 0, IOR 1).
@vertex
fn vs_pbr(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) base: vec3<f32>, @location(3) parameters: vec3<f32>) -> PbrOut {
    return pbr_out(p, n, base, parameters, vec4<f32>(0.25, 0.0, 0.0, 1.0));
}

// A PBR vertex with the product look's detail (`pbr::DETAILED_VERTEX_SIZE`).
@vertex
fn vs_pbr_detailed(
    @location(0) p: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) base: vec3<f32>,
    @location(3) parameters: vec3<f32>,
    @location(4) detail: vec4<f32>,
) -> PbrOut {
    return pbr_out(p, n, base, parameters, detail);
}

/// The AO prepass's depth of a PBR surface (drawn there only with the product look).
@fragment
fn fs_ao_depth_pbr(i: PbrOut, @builtin(front_facing) front: bool) -> AoDepth {
    return opaque_depth(TOut(i.pos, i.n, vec4<f32>(i.base, 1.0), i.wp), front);
}

fn pbr_fresnel(f0: vec3<f32>, cosine: f32, metallic: f32, ior: f32) -> vec3<f32> {
    let t = 1.0 - cosine;
    // Match the shared surface: an index-matched dielectric has no interface.
    let grazing = select(1.0, metallic, ior == 1.0);
    return f0 + (vec3<f32>(grazing) - f0) * (t * t * t * t * t);
}

fn pbr_brdf(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let nv = clamp(dot(n, v), -1.0, 1.0);
    let nl = clamp(dot(n, l), -1.0, 1.0);
    if (nv <= 0.0 || nl <= 0.0) { return vec3<f32>(0.0); }
    let h = pbr_unit(v + l);
    let nh = clamp(dot(n, h), 0.0, 1.0);
    let vh = clamp(dot(v, h), 0.0, 1.0);
    let alpha = parameters.y * parameters.y;
    let a2 = alpha * alpha;
    let denominator = (1.0 - nh * nh) + nh * nh * a2;
    let distribution = a2 / (3.141592653589793 * denominator * denominator);
    let gv = 2.0 / (nv + sqrt(a2 + (1.0 - a2) * nv * nv));
    let gl = 2.0 / (nl + sqrt(a2 + (1.0 - a2) * nl * nl));
    let ratio = (parameters.z - 1.0) / (parameters.z + 1.0);
    let f0 = mix(vec3<f32>(ratio * ratio), base, parameters.x);
    let specular = pbr_fresnel(f0, vh, parameters.x, parameters.z) * (distribution * 0.25 * gv * gl);
    let diffuse = base * ((1.0 - parameters.x) / 3.141592653589793);
    return specular + (vec3<f32>(1.0) - pbr_fresnel(f0, nv, parameters.x, parameters.z)) * (vec3<f32>(1.0) - pbr_fresnel(f0, nl, parameters.x, parameters.z)) * diffuse;
}

fn pbr_poly(c0: vec4<f32>, c1: vec4<f32>, c2: vec4<f32>, r: f32, t: vec4<f32>) -> f32 {
    return dot(c0, t) + r * (dot(c1, t) + r * dot(c2, t));
}

// The split-sum scale and bias (x: A, y: B; albedo F0·A + F90·B) of `pbr_brdf` in a uniform
// environment: SolidCraft's own polynomial fit (`ibl::ENV_BRDF_A`, `ibl::ENV_BRDF_B`).
fn pbr_env_brdf(cosine: f32, r: f32) -> vec2<f32> {
    let nv = clamp(cosine, 0.0001, 1.0);
    let a2 = (r * r) * (r * r);
    let q = 1.0 - 2.0 * nv / (nv + sqrt(a2 + (1.0 - a2) * nv * nv));
    let s = 1.0 - nv;
    let t = vec4<f32>(1.0, s, s * s, s * s * s);
    let a = pbr_poly(vec4<f32>(1.00560, -0.56397, 2.50123, -2.93657), vec4<f32>(0.32333, -0.09745, -2.44274, 2.89048), vec4<f32>(-1.07421, 4.87385, -9.53132, 5.20517), r, t)
        + q * pbr_poly(vec4<f32>(-27.45309, 62.21089, -24.05595, -10.77288), vec4<f32>(10.98918, 41.63909, -157.76627, 106.99327), vec4<f32>(9.57360, -85.17991, 167.93185, -93.69565), r, t);
    let b = pbr_poly(vec4<f32>(-0.02603, 0.65203, -2.71929, 3.09764), vec4<f32>(0.09148, -3.19509, 11.52730, -9.30966), vec4<f32>(-0.07161, 5.26964, -16.16043, 8.05334), r, t)
        + q * pbr_poly(vec4<f32>(-30.15892, 134.44678, -183.67455, 78.98073), vec4<f32>(44.85160, -207.80167, 299.57468, -136.87570), vec4<f32>(-20.09456, 90.78933, -127.08962, 59.86296), r, t);
    let ab = clamp(vec2<f32>(a, b), vec2<f32>(0.0), vec2<f32>(1.0));
    // A surface reflects no more than it receives: the fit's albedo stays at or below one.
    return ab / max(ab.x + ab.y, 1.0);
}

// Where to look in a studio turned by `rotation` about +Z.
fn pbr_turn(d: vec3<f32>, rotation: f32) -> vec3<f32> {
    let s = sin(rotation);
    let c = cos(rotation);
    return vec3<f32>(c * d.x + s * d.y, -s * d.x + c * d.y, d.z);
}

fn pbr_irradiance(n: vec3<f32>) -> vec3<f32> {
    let k = studio.irradiance;
    let e = k[0].xyz + k[1].xyz * n.x + k[2].xyz * n.y + k[3].xyz * n.z + k[4].xyz * (n.x * n.y) + k[5].xyz * (n.y * n.z)
        + k[6].xyz * (n.x * n.z) + k[7].xyz * (n.z * n.z) + k[8].xyz * (n.x * n.x - n.y * n.y);
    return max(e, vec3<f32>(0.0));
}

// The clear coat over `pbr_brdf` (`clear_coat.rs`, `path.wgsl`): a single-scattering GGX
// dielectric lobe (weight, perceptual roughness, IOR) over the base, which it attenuates at entry
// and exit by an upper bound of its directional reflectance. Weight 0 or IOR 1 is no coat.
fn pbr_coat_bound(f0: f32, cosine: f32) -> f32 {
    let minimum_vh = cosine / sqrt(2.0 * (1.0 + sqrt(max(0.0, 1.0 - cosine * cosine))));
    let t = 1.0 - clamp(minimum_vh, 0.0, 1.0);
    return f0 + (1.0 - f0) * (t * t * t * t * t);
}

fn pbr_coated(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let under = pbr_brdf(base, parameters, n, v, l);
    if (coat.x <= 0.0 || coat.z == 1.0) { return under; }
    let nv = clamp(dot(n, v), -1.0, 1.0);
    let nl = clamp(dot(n, l), -1.0, 1.0);
    if (nv <= 0.0 || nl <= 0.0) { return under; }
    let h = pbr_unit(v + l);
    let nh = clamp(dot(n, h), 0.0, 1.0);
    let vh = clamp(dot(v, h), 0.0, 1.0);
    let alpha = coat.y * coat.y;
    let a2 = alpha * alpha;
    let denominator = (1.0 - nh * nh) + nh * nh * a2;
    let distribution = a2 / (3.141592653589793 * denominator * denominator);
    let gv = 2.0 / (nv + sqrt(a2 + (1.0 - a2) * nv * nv));
    let gl = 2.0 / (nl + sqrt(a2 + (1.0 - a2) * nl * nl));
    let ratio = (coat.z - 1.0) / (coat.z + 1.0);
    let f0 = ratio * ratio;
    let t = 1.0 - vh;
    let fresnel = f0 + (1.0 - f0) * (t * t * t * t * t);
    let attenuation = (1.0 - coat.x * pbr_coat_bound(f0, nv)) * (1.0 - coat.x * pbr_coat_bound(f0, nl));
    return under * attenuation + vec3<f32>(coat.x * fresnel * distribution * 0.25 * gv * gl);
}

// A vertex's coat (weight, roughness, IOR) from its detail (`GpuScene::pbr_tri_with`).
fn pbr_coat_of(detail: vec4<f32>) -> vec3<f32> {
    let raw = clamp(1.0 + (fract(max(detail.x, 0.0)) - 0.25) * 4.0, 1.0, 3.0);
    // An index-matched coat is exactly 1 (no interface), as `clear_coat.rs` treats it.
    let ior = select(raw, 1.0, abs(raw - 1.0) < 1e-4);
    return vec3<f32>(clamp(detail.z, 0.0, 1.0), clamp(detail.w, 0.045, 1.0), ior);
}

// The studio's light reflected toward `v`, in parts: the base's specular, the coat's specular
// and the diffuse layer. Split-sum specular with multiple-scattering compensation; diffuse from
// the irradiance polynomial weighted by what the specular lobe leaves; a coat reflects its own
// single-scattering albedo w·(F0·A + B) at its roughness and passes the rest to the base
// (`ibl::reference::shade` on the CPU).
struct PbrIbl { specular: vec3<f32>, coat: vec3<f32>, diffuse: vec3<f32>, };

fn pbr_ibl_parts(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, rotation: f32) -> PbrIbl {
    let nv = max(dot(n, v), 0.0001);
    let metallic = parameters.x;
    let roughness = parameters.y;
    let ratio = (parameters.z - 1.0) / (parameters.z + 1.0);
    let f0 = mix(vec3<f32>(ratio * ratio), base, metallic);
    let f90 = select(1.0, metallic, parameters.z == 1.0);
    let ab = pbr_env_brdf(nv, roughness);
    let single = f0 * ab.x + vec3<f32>(f90 * ab.y);
    let compensation = vec3<f32>(1.0) + f0 * (1.0 / max(ab.x + ab.y, 0.0001) - 1.0);
    let r = 2.0 * nv * n - v;
    let prefiltered = textureSampleLevel(studio_cube, studio_smp, pbr_turn(r, rotation), roughness * studio.levels.x).rgb;
    // The diffuse layer takes what the specular lobe does not reflect, so a white lossless
    // surface reflects exactly what it receives.
    let specular = single * compensation;
    let diffuse = base * ((1.0 - metallic) / 3.141592653589793) * max(vec3<f32>(1.0) - specular, vec3<f32>(0.0));
    var out = PbrIbl(prefiltered * specular, vec3<f32>(0.0), diffuse * pbr_irradiance(pbr_turn(n, rotation)));
    if (coat.x > 0.0 && coat.z != 1.0) {
        let coat_ratio = (coat.z - 1.0) / (coat.z + 1.0);
        let cab = pbr_env_brdf(nv, coat.y);
        let albedo = coat.x * (coat_ratio * coat_ratio * cab.x + cab.y);
        let seen = textureSampleLevel(studio_cube, studio_smp, pbr_turn(r, rotation), coat.y * studio.levels.x).rgb;
        out = PbrIbl(out.specular * (1.0 - albedo), seen * albedo, out.diffuse * (1.0 - albedo));
    }
    return out;
}

fn pbr_ibl(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, rotation: f32) -> vec3<f32> {
    let parts = pbr_ibl_parts(base, parameters, vec3<f32>(0.0, 1.0, 1.0), n, v, rotation);
    return parts.specular + parts.diffuse;
}

// What of a uniform light a coat passes to the base, seen at `cosine` (the ambient kind).
fn pbr_coat_keep(coat: vec3<f32>, cosine: f32) -> f32 {
    if (coat.x <= 0.0 || coat.z == 1.0) { return 1.0; }
    let ratio = (coat.z - 1.0) / (coat.z + 1.0);
    let ab = pbr_env_brdf(max(cosine, 0.0001), coat.y);
    return 1.0 - coat.x * (ratio * ratio * ab.x + ab.y);
}

// Specular occlusion from ambient occlusion `ao` (Lagarde and de Rousiers 2014, "Moving
// Frostbite to PBR", mathematics only), at perceptual roughness `r`.
fn pbr_specular_occlusion(nv: f32, ao: f32, r: f32) -> f32 {
    return clamp(pow(max(nv, 0.0) + ao, exp2(-16.0 * r * r - 1.0)) - 1.0 + ao, 0.0, 1.0);
}

// The product look's procedural surface detail (`view::SurfacePattern`), in millimetres from the
// world position: a glass-fibre weave, a solder mask's orange peel and silkscreen grain. Each
// fades to its mean as its period shrinks below the pixel's footprint (taken before any branch
// in `fs_pbr`).
const PATTERN_WEAVE: i32 = 1;
const PATTERN_MASK: i32 = 2;
const PATTERN_MASK_OVER_WEAVE: i32 = 3;
const PATTERN_SILKSCREEN: i32 = 4;
// The weave's means over a cell (numerical, 2400² samples): the visible yarn's height and its
// fibre striation.
const WEAVE_MEAN: f32 = 0.779014;
const WEAVE_FIBRE_MEAN: f32 = -0.003887;
// Relative albedo swing of the weave at strength 1, and its relief (mm) on bare laminate.
const WEAVE_CONTRAST: f32 = 0.55;
const WEAVE_RELIEF: f32 = 0.006;
// A routed edge shows the plies as a subtle layering: its swing relative to a face's.
const WEAVE_EDGE: f32 = 0.25;
// Filament striations along each yarn: how many per yarn width, and their swing relative to
// the yarn's crown.
const WEAVE_FIBRES: f32 = 7.0;
const WEAVE_FIBRE_CONTRAST: f32 = 0.3;
// Orange peel: two long, gentle wavelengths (mm) and their relief (mm), so a gloss mask's
// softbox reflection keeps its shape with only a slight waviness.
const PEEL_WAVE: vec2<f32> = vec2<f32>(2.4, 0.9);
const PEEL_RELIEF: vec2<f32> = vec2<f32>(0.006, 0.001);
// The look's direct lights' angular half-size (rad) over two: the GGX α they add to a coat.
const LOOK_LIGHT_SPREAD: f32 = 0.1;
// Silkscreen grain: two cell sizes (mm), their relative albedo swing and relief (mm).
const GRAIN_CELL: vec2<f32> = vec2<f32>(0.05, 0.16);
const GRAIN_CONTRAST: f32 = 0.18;
const GRAIN_RELIEF: f32 = 0.002;

struct PbrDetail { albedo: f32, n: vec3<f32>, };

/// The look's detail parameters: weave pitch along world X and Y, ply thickness (mm) and world
/// units per millimetre.
fn look_detail_params() -> vec4<f32> {
    return u.look_detail;
}

// An integer hash of a lattice point, in [0, 1].
fn pat_hash(c: vec2<i32>) -> f32 {
    var h = (bitcast<u32>(c.x) * 1597334677u) ^ (bitcast<u32>(c.y) * 3812015801u);
    h = h * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h) / 4294967295.0;
}

// Value noise in [0, 1] (mean 0.5) with a lattice step of one.
fn pat_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * (vec2<f32>(3.0) - 2.0 * f);
    let c = vec2<i32>(i);
    let a = pat_hash(c);
    let b = pat_hash(c + vec2<i32>(1, 0));
    let d = pat_hash(c + vec2<i32>(0, 1));
    let e = pat_hash(c + vec2<i32>(1, 1));
    return mix(mix(a, b, w.x), mix(d, e, w.x), w.y);
}

// A yarn bundle's elliptic crown at `t` across it (−0.5..0.5 is its width): 1 on its axis, 0 at
// its edges.
fn pat_crown(t: f32) -> f32 {
    return sqrt(max(1.0 - 4.0 * t * t, 0.0));
}

// A plain weave at `p` (in yarn pitches): warp yarns along x centred on each row, weft yarns
// along y centred on each column, each rising over one crossing and dipping under the next, with
// over-under parity alternating cell by cell. The visible yarn is the higher one, so each yarn
// runs on continuously, the crossing yarn shows where it dips, and the gaps are only the
// windows between four crossings. Returns the visible yarn's height and its filament
// striations (running along it), both zero mean over a cell.
fn pat_weave_parts(p: vec2<f32>) -> vec2<f32> {
    let cell = floor(p);
    let f = p - cell - vec2<f32>(0.5);
    let pi = 3.141592653589793;
    let up_x = 0.5 + 0.5 * cos(pi * (p.x - 0.5 + cell.y));
    let up_y = 0.5 - 0.5 * cos(pi * (p.y - 0.5 + cell.x));
    let hx = pat_crown(f.y) * (0.6 + 0.4 * up_x);
    let hy = pat_crown(f.x) * (0.6 + 0.4 * up_y);
    let across = select(f.x, f.y, hx >= hy);
    let fibre = cos(2.0 * pi * WEAVE_FIBRES * across) * pat_crown(across);
    return vec2<f32>(max(hx, hy) - WEAVE_MEAN, fibre - WEAVE_FIBRE_MEAN);
}

fn pat_weave(p: vec2<f32>) -> f32 {
    return pat_weave_parts(p).x;
}

// How much of a pattern of period `period` (mm) shows at a pixel footprint `fp` (mm).
fn pat_fade(fp: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.2, 0.6, fp / max(period, 1e-6));
}

// The weave's fade: its cells have sharp edges and its fibres are fine, so it is gone well
// before its period reaches two pixels (no moiré at board distance).
fn pat_weave_fade(fp: f32, period: f32) -> f32 {
    return 1.0 - smoothstep(0.1, 0.3, fp / max(period, 1e-6));
}

// Value noise of lattice step `size` (mm) at `uv` (mm), faded by the footprint `fp`: its
// deviation from the mean and its gradient (per mm).
fn pat_bumps(uv: vec2<f32>, size: f32, fp: f32) -> vec3<f32> {
    let p = uv / size;
    let m = pat_noise(p);
    let g = vec2<f32>(pat_noise(p + vec2<f32>(0.05, 0.0)) - m, pat_noise(p + vec2<f32>(0.0, 0.05)) - m) / (0.05 * size);
    return pat_fade(fp, size) * vec3<f32>(m - 0.5, g);
}

// The surface detail at `wp` with unit normal `n`: the albedo factor and the perturbed normal.
// `detail` is the vertex's (pattern, strength, …); `footprint` is `fwidth` of the world position;
// `k` is `look_detail_params()`.
fn look_detail(wp: vec3<f32>, n: vec3<f32>, footprint: vec3<f32>, detail: vec4<f32>, k: vec4<f32>) -> PbrDetail {
    let id = i32(floor(max(detail.x, 0.0)));
    let strength = clamp(detail.y, 0.0, 1.0);
    if (id < PATTERN_WEAVE || id > PATTERN_SILKSCREEN) { return PbrDetail(1.0, n); }
    let mm = 1.0 / max(k.w, 1e-6);
    let q = wp * mm;
    let fp = max(max(footprint.x, footprint.y), footprint.z) * mm;
    // A face's axes are the board's X and Y; an edge's run along it and up through the plies.
    let top = abs(n.z) >= 0.7;
    let run = pbr_unit(vec3<f32>(-n.y, n.x, 0.0));
    let axis_u = select(run, vec3<f32>(1.0, 0.0, 0.0), top);
    let axis_v = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), top);
    let uv = vec2<f32>(dot(q, axis_u), dot(q, axis_v));
    var albedo = 0.0;
    var slope = vec2<f32>(0.0);
    if (id == PATTERN_WEAVE || id == PATTERN_MASK_OVER_WEAVE) {
        let along = abs(run.x) * k.x + abs(run.y) * k.y;
        let period = max(select(vec2<f32>(along, k.z), k.xy, top), vec2<f32>(1e-3));
        let fade = pat_weave_fade(fp, min(period.x, period.y)) * select(WEAVE_EDGE, 1.0, top);
        let fibres = pat_weave_fade(fp, min(period.x, period.y) / WEAVE_FIBRES) * WEAVE_FIBRE_CONTRAST;
        let p = uv / period;
        let parts = pat_weave_parts(p);
        let m = parts.x;
        let g = vec2<f32>(pat_weave(p + vec2<f32>(0.05, 0.0)) - m, pat_weave(p + vec2<f32>(0.0, 0.05)) - m) / (0.05 * period);
        albedo += fade * strength * WEAVE_CONTRAST * (m + fibres * parts.y);
        if (id == PATTERN_WEAVE) { slope += fade * strength * WEAVE_RELIEF * g; }
    }
    if (id == PATTERN_MASK || id == PATTERN_MASK_OVER_WEAVE) {
        // Over a weave the strength is the weave's show-through; the peel stays at full.
        let peel = select(strength, 1.0, id == PATTERN_MASK_OVER_WEAVE);
        slope += peel * (pat_bumps(uv, PEEL_WAVE.x, fp).yz * PEEL_RELIEF.x + pat_bumps(uv + vec2<f32>(17.0, 0.0), PEEL_WAVE.y, fp).yz * PEEL_RELIEF.y);
    }
    if (id == PATTERN_SILKSCREEN) {
        let fine = pat_bumps(uv, GRAIN_CELL.x, fp);
        let coarse = pat_bumps(uv + vec2<f32>(0.0, 31.0), GRAIN_CELL.y, fp);
        albedo += strength * GRAIN_CONTRAST * 2.0 * (fine.x + coarse.x);
        slope += strength * GRAIN_RELIEF * (fine.yz + coarse.yz);
    }
    return PbrDetail(max(1.0 + albedo, 0.0), pbr_unit(n - slope.x * axis_u - slope.y * axis_v));
}

@fragment
fn fs_pbr(i: PbrOut) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow: the pixel's footprint, over which the product
    // look's patterns fade to their mean.
    let footprint = fwidth(i.wp);
    var n = pbr_unit(i.n);
    var view = u.back.xyz;
    if (u.eye.w > 0.5) { view = u.eye.xyz - i.wp; }
    let v = pbr_unit(view);
    // Without the look: the vertex's base and normal, no coat, no occlusion.
    var base = i.base;
    var coat = vec3<f32>(0.0, 1.0, 1.0);
    var ao = 1.0;
    let look = look_on();
    if (look) {
        let detail = look_detail(i.wp, n, footprint, i.detail, look_detail_params());
        base = clamp(base * detail.albedo, vec3<f32>(0.0), vec3<f32>(1.0));
        // Relief fades out toward grazing views, where it would only streak the reflections.
        n = pbr_unit(mix(n, detail.n, smoothstep(0.05, 0.35, abs(dot(n, v)))));
        coat = pbr_coat_of(i.detail);
        // Occlusion darkens only the indirect light: the studio and the ambient kind.
        ao = occlusion(TOut(i.pos, i.n, vec4<f32>(base, 1.0), i.wp));
    }
    let ambient = select(1.0, ao * pbr_coat_keep(coat, dot(n, v)), look);
    // In the look a direct light is a softbox, not a point: a coat's lobe for it widens by the
    // light's angular size (Karis 2013, sphere-light roughness widening, mathematics only), so
    // a gloss coat shows a soft highlight instead of a clipped dot. GGX stays normalised, so the
    // light's energy is unchanged.
    let lit_coat = select(coat, vec3<f32>(coat.x, sqrt(min(coat.y * coat.y + LOOK_LIGHT_SPREAD, 1.0)), coat.z), look);
    let lights = array<vec4<f32>, 3>(vec4<f32>(0.6, 0.0, 0.8, 3.0), vec4<f32>(-0.8, 0.0, 0.6, 1.0), vec4<f32>(0.0, -0.6, 0.8, 1.5));
    var radiance = vec3<f32>(0.0);
    // The fixed studio lights only the default preview: the look's studio replaces it.
    for (var k = 0u; k < 3u && u.product_lights_meta.x == 0.0 && !look; k += 1u) {
        let light = lights[k];
        radiance += pbr_coated(base, i.parameters, coat, n, v, light.xyz) * (light.w * max(dot(n, light.xyz), 0.0));
    }
    for (var k = 0u; k < min(u32(u.product_lights_meta.y),21u) && u.product_lights_meta.x > 0.0; k += 1u) {
        let source=u.product_lights[k*4u];
        let colour=u.product_lights[k*4u+1u];
        let axis=u.product_lights[k*4u+2u];
        let falloff=u.product_lights[k*4u+3u];
        if source.w == 4.0 {
            radiance += base*colour.xyz*(1.0-i.parameters.x)/3.141592653589793*ambient;
            continue;
        }
        let sample=product_light_sample(k,i.wp);
        radiance += pbr_coated(base,i.parameters,lit_coat,n,v,sample[0])*sample[1]*max(dot(n,sample[0]),0.0);
    }
    if (look) {
        // The product look: the studio (occluded), exposure, PBR Neutral and a dithered encode.
        let p = look_params();
        let parts = pbr_ibl_parts(base, i.parameters, coat, n, v, p.z);
        let nv = dot(n, v);
        let indirect = parts.diffuse * ao + parts.specular * pbr_specular_occlusion(nv, ao, i.parameters.y)
            + parts.coat * pbr_specular_occlusion(nv, ao, coat.y);
        radiance += indirect * p.y;
        let encoded = look_encode(look_tone(radiance * p.x), i.pos.xy);
        if (u.back.w > 0.5) { return vec4<f32>(to_linear(encoded), 1.0); }
        return vec4<f32>(encoded, 1.0);
    }
    let mapped = radiance / (vec3<f32>(1.0) + radiance);
    if (u.back.w > 0.5) { return vec4<f32>(mapped, 1.0); }
    return vec4<f32>(pbr_encode(mapped), 1.0);
}
"#;
