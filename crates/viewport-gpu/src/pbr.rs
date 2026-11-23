//! Original GPU translation of the L1 opaque surface and fixed studio reference.
//! Appended to the existing viewport shader so camera/output conventions are shared.

pub const VERTEX_SIZE: usize = 48;

/// Bind group shared by the three PBR vertex paths and opaque faces.
pub const STUDIO_GROUP: u32 = 3;

pub const SOURCE: &str = concat!(
    crate::shading::library_source!(),
    "\n",
    r#"
// The selected studio shares the neutral room's map format and GGX table.
// The rig uniform supplies the mip schedule, room bounds and four light rectangles.
@group(3) @binding(0) var rig_specular: texture_2d<f32>;
@group(3) @binding(1) var studio_smp: sampler;
struct Studio { levels: vec4<f32>, room: vec4<f32>, lights: array<vec4<f32>, 16>, };
@group(3) @binding(2) var<uniform> studio: Studio;
@group(3) @binding(11) var rig_diffuse: texture_2d<f32>;
@group(3) @binding(12) var rig_dfg: texture_2d<f32>;
// World Z-up to the shared maps' Y-up frame.
fn pbr_studio_uv(d: vec3<f32>) -> vec2<f32> {
    let n = pbr_unit(d);
    // At a pole longitude is arbitrary. On the X meridian, the second argument
    // of atan2 is zero and fast GPU math can select the opposite longitude.
    if (n.y == 0.0) {
        return vec2<f32>(0.5 + 0.25 * sign(n.x), acos(clamp(n.z, -1.0, 1.0)) / 3.141592653589793);
    }
    return studio_uv(vec3<f32>(n.x, n.z, -n.y));
}

fn pbr_studio_radiance(d: vec3<f32>, level: f32) -> vec3<f32> {
    return textureSampleLevel(rig_specular, studio_smp, pbr_studio_uv(d), level).rgb;
}

fn pbr_dfg(cosine: f32, roughness: f32) -> vec2<f32> {
    let half_texel = 0.5 / vec2<f32>(textureDimensions(rig_dfg));
    let uv = clamp(vec2<f32>(cosine, roughness), half_texel, 1.0 - half_texel);
    let ab = textureSampleLevel(rig_dfg, studio_smp, uv, 0.0).rg;
    // Finite quadrature can put a lossless lobe just above unit reflectance.
    return ab / max(1.0, ab.x + ab.y);
}

// Where to look in a studio turned by `rotation` about +Z.
fn pbr_turn(d: vec3<f32>, rotation: f32) -> vec3<f32> {
    let s = sin(rotation);
    let c = cos(rotation);
    return vec3<f32>(c * d.x + s * d.y, -s * d.x + c * d.y, d.z);
}

fn pbr_irradiance(n: vec3<f32>) -> vec3<f32> {
    return 3.141592653589793 * textureSampleLevel(rig_diffuse, studio_smp, pbr_studio_uv(n), 0.0).rgb;
}

// Whether the frame places the studio around the scene (`ibl::StudioRoom`).
fn pbr_room_on() -> bool {
    return u.look_room.w > 0.0;
}

// A world point in the studio's own frame: about the studio's centre (half an extent above the
// ground over the subject), in subject extents, turned with the studio. The centre without a room.
fn pbr_studio_point(wp: vec3<f32>, rotation: f32) -> vec3<f32> {
    if (!pbr_room_on()) { return vec3<f32>(0.0); }
    let q = (wp - u.look_room.xyz) / u.look_room.w - vec3<f32>(0.0, 0.0, 0.5);
    return pbr_turn(q, rotation);
}

// Box projection: where the ray from studio point `q` along `r` (studio frame) leaves the room,
// as a direction from the studio's centre; `r` itself without a room.
fn pbr_room_exit(q: vec3<f32>, r: vec3<f32>) -> vec3<f32> {
    if (!pbr_room_on()) { return r; }
    let lo = vec3<f32>(-studio.room.x, -studio.room.x, studio.room.y);
    let hi = vec3<f32>(studio.room.x, studio.room.x, studio.room.z);
    let p = clamp(q, lo, hi);
    let s = select(r, vec3<f32>(0.000001), abs(r) < vec3<f32>(0.000001));
    let t3 = (select(lo, hi, s > vec3<f32>(0.0)) - p) / s;
    let t = max(min(t3.x, min(t3.y, t3.z)), 0.0);
    return p + t * r;
}

// Contact darkening of the floor under the subject, where a reflection meets it (`hit`: studio
// frame, from `pbr_room_exit`): 1 away from the footprint, darker under it.
fn pbr_floor_contact(hit: vec3<f32>, rotation: f32) -> f32 {
    if (!pbr_room_on() || hit.z > studio.room.y + 0.001) { return 1.0; }
    // The ground shadow's ellipse (`ibl::StudioRoom::footprint`), darkest in the middle.
    let w = pbr_turn(hit, -rotation);
    let f = max(u.look_foot.xy / u.look_room.w, vec2<f32>(0.02));
    let d = length(w.xy / f);
    return 1.0 - 0.4 * (1.0 - smoothstep(0.35, 1.1, d));
}

// One edge's term of Lambert's polygon irradiance: θ (n · γ̂), `a` and `b` unit.
fn pbr_edge(a: vec3<f32>, b: vec3<f32>, n: vec3<f32>) -> f32 {
    let c = cross(a, b);
    let l = length(c);
    if (l < 0.000001) { return 0.0; }
    return acos(clamp(dot(a, b), -1.0, 1.0)) * dot(n, c) / l;
}

// The irradiance over unit radiance a rectangle (corners `v`, relative to the surface point)
// gives a surface with unit normal `n`: the rectangle clipped to the upper half-space, then
// ½ |Σ θᵢ (n · γ̂ᵢ)| (`solidcraft_viewport::studio::polygon_irradiance` on the CPU).
fn pbr_rect_irradiance(v: array<vec3<f32>, 4>, n: vec3<f32>) -> f32 {
    var poly: array<vec3<f32>, 5>;
    var count = 0u;
    for (var i = 0u; i < 4u; i += 1u) {
        let a = v[i];
        let b = v[(i + 1u) % 4u];
        let da = dot(a, n);
        let db = dot(b, n);
        if (da >= 0.0 && count < 5u) { poly[count] = a; count += 1u; }
        if ((da >= 0.0) != (db >= 0.0) && count < 5u) { poly[count] = mix(a, b, da / (da - db)); count += 1u; }
    }
    if (count < 3u) { return 0.0; }
    var sum = 0.0;
    for (var i = 0u; i < count; i += 1u) {
        let a = poly[i];
        let b = poly[(i + 1u) % count];
        if (length(a) < 0.000001 || length(b) < 0.000001) { continue; }
        sum += pbr_edge(normalize(a), normalize(b), n);
    }
    return 0.5 * abs(sum);
}

// The rig's lights' irradiance on a surface at studio point `q` with unit normal `n` (studio
// frame), each from its emitting side only.
fn pbr_rig_irradiance(q: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    var e = vec3<f32>(0.0);
    for (var k = 0u; k < min(u32(studio.levels.y), 4u); k += 1u) {
        let c = studio.lights[4u * k];
        let w = studio.lights[4u * k + 1u];
        let h = studio.lights[4u * k + 2u];
        let radiance = studio.lights[4u * k + 3u].xyz;
        if (dot(q - c.xyz, cross(w.xyz, h.xyz)) <= 0.0) { continue; }
        let du = w.xyz * c.w;
        let dv = h.xyz * w.w;
        let o = c.xyz - q;
        let corners = array<vec3<f32>, 4>(o - du - dv, o + du - dv, o + du + dv, o - du + dv);
        e += radiance * pbr_rect_irradiance(corners, n);
    }
    return e;
}

fn pbr_env_level(roughness: f32) -> f32 {
    let r = clamp(roughness, 0.0, 1.0);
    let top = studio.levels.x;
    if (top < 1.0) {
        return 0.0;
    }
    let one = 1.0 / top;
    if (r >= one) {
        return r * top;
    }
    return max(r - studio.levels.z, 0.0) / max(one - studio.levels.z, 0.0001);
}

fn pbr_specular_occlusion(nv: f32, ao: f32, r: f32) -> f32 {
    return clamp(pow(max(nv, 0.0) + ao, exp2(-16.0 * r * r - 1.0)) - 1.0 + ao, 0.0, 1.0);
}

fn pbr_studio(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, wp: vec3<f32>, ao: f32) -> vec3<f32> {
    let nv = max(dot(n, v), 0.0001);
    let ratio = (parameters.z - 1.0) / (parameters.z + 1.0);
    let f0 = mix(vec3<f32>(ratio * ratio), base, parameters.x);
    let f90 = select(1.0, parameters.x, parameters.z == 1.0);
    let ab = pbr_dfg(nv, parameters.y);
    let single = f0 * ab.x + vec3<f32>(f90 * ab.y);
    let specular = single * (vec3<f32>(1.0) + f0 * (1.0 / max(ab.x + ab.y, 0.0001) - 1.0));
    let diffuse = base * ((1.0 - parameters.x) / 3.141592653589793) * max(vec3<f32>(1.0) - specular, vec3<f32>(0.0));
    let rotation = u.look.z;
    let ns = pbr_turn(n, rotation);
    let q = pbr_studio_point(wp, rotation);
    let hit = pbr_room_exit(q, pbr_turn(2.0 * nv * n - v, rotation));
    let irradiance = pbr_irradiance(ns) + pbr_rig_irradiance(q, ns);
    let seen = pbr_studio_radiance(hit, pbr_env_level(parameters.y)) * pbr_floor_contact(hit, rotation);
    return diffuse * irradiance * ao + specular * seen * pbr_specular_occlusion(nv, ao, parameters.y);
}

struct PbrOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) n: vec3<f32>,
    @location(1) base: vec3<f32>,
    @location(2) wp: vec3<f32>,
    @location(3) parameters: vec3<f32>,
    // 1: lit on both sides (the material's `two_sided`), 0 for one side.
    @location(4) @interpolate(flat) two: f32,
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

@vertex
fn vs_pbr(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) base: vec3<f32>, @location(3) parameters: vec3<f32>) -> PbrOut {
    var o: PbrOut;
    o.pos = u.vp * vec4<f32>(p, 1.0);
    o.n = n;
    o.base = base;
    o.wp = p;
    o.parameters = parameters;
    return o;
}

fn pbr_brdf(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let angles = sc_brdf_angles(n, v, l, pbr_unit(v + l));
    if (angles.nv <= 0.0 || angles.nl <= 0.0) { return vec3<f32>(0.0); }
    return sc_base_at(base, parameters, angles);
}

// `pbr_materials`, the palette's PBR materials, is declared in viewport.wgsl.

// A placement of a shared smooth mesh whose palette entry names its material (the product
// preview's instanced PBR parts): moved as `vs_tri_inst` moves it.
@vertex
fn vs_pbr_inst(
    @location(0) p: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(3) r0: vec4<f32>,
    @location(4) r1: vec4<f32>,
    @location(5) r2: vec4<f32>,
    @location(6) c: u32,
    @location(9) s: u32,
) -> PbrOut {
    let q = vec4<f32>(p, 1.0);
    // Moved with its body, like every other placement (`placed_by` in viewport.wgsl): by its own
    // motion, else its entry's (read only while something moves).
    var m = 0u;
    if (u.screen.z >= 0.5) { m = placement_motion(s, palette_entry(c)); }
    let wp = placed_by(m, vec3<f32>(dot(r0, q), dot(r1, q), dot(r2, q)));
    let at = vec2<u32>(c & 2047u, (c >> 11u) * 2u);
    var o: PbrOut;
    o.pos = u.vp * vec4<f32>(wp, 1.0);
    o.n = turned_by(m, vec3<f32>(dot(r0.xyz, n), dot(r1.xyz, n), dot(r2.xyz, n)));
    o.base = textureLoad(pbr_materials, at, 0).xyz;
    o.wp = wp;
    let optics = textureLoad(pbr_materials, at + vec2<u32>(0u, 1u), 0);
    o.parameters = optics.xyz;
    o.two = optics.w;
    return o;
}

// A built triangle vertex whose palette entry names its material (the product preview's blocks).
@vertex
fn vs_pbr_palette(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: u32) -> PbrOut {
    let at = vec2<u32>(c & 2047u, (c >> 11u) * 2u);
    // Moved with its body, like every other built triangle.
    let m = motion_at(c);
    let wp = placed_by(m, p);
    var o: PbrOut;
    o.pos = u.vp * vec4<f32>(wp, 1.0);
    o.n = turned_by(m, n);
    o.base = textureLoad(pbr_materials, at, 0).xyz;
    o.wp = wp;
    let optics = textureLoad(pbr_materials, at + vec2<u32>(0u, 1u), 0);
    o.parameters = optics.xyz;
    o.two = optics.w;
    return o;
}

@fragment
fn fs_ao_depth_pbr(i: PbrOut, @builtin(front_facing) front: bool) -> AoDepth {
    return opaque_depth(TOut(i.pos, i.n, vec4<f32>(i.base, 1.0), i.wp, 0u, 0.0, 0u), front);
}

@fragment
fn fs_pbr(i: PbrOut) -> @location(0) vec4<f32> {
    var n = pbr_unit(i.n);
    var view = u.back.xyz;
    if (u.eye.w > 0.5) { view = u.eye.xyz - i.wp; }
    let v = pbr_unit(view);
    // A two-sided material's back, seen, is lit as its front (an open sheet's other side).
    if (i.two > 0.5 && dot(n, v) < 0.0) { n = -n; }
    var ao = 1.0;
    if (look_on()) { ao = occlusion(TOut(i.pos, n, vec4<f32>(i.base, 1.0), i.wp, 0u, 0.0, 0u)); }
    return vec4<f32>(pbr_lit(i.base, i.parameters, n, v, i.wp, i.pos.xy, ao), 1.0);
}

fn pbr_lit(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, wp: vec3<f32>, pixel: vec2<f32>, ao: f32) -> vec3<f32> {
    let lights = array<vec4<f32>, 3>(vec4<f32>(0.6, 0.0, 0.8, 3.0), vec4<f32>(-0.8, 0.0, 0.6, 1.0), vec4<f32>(0.0, -0.6, 0.8, 1.5));
    var radiance = vec3<f32>(0.0);
    for (var k = 0u; k < 3u && u.product_lights_meta.x == 0.0 && !look_on(); k += 1u) {
        let light = lights[k];
        radiance += pbr_brdf(base, parameters, n, v, light.xyz) * (light.w * max(dot(n, light.xyz), 0.0));
    }
    for (var k = 0u; k < min(u32(u.product_lights_meta.y),21u) && u.product_lights_meta.x > 0.0; k += 1u) {
        let source=u.product_lights[k*4u];
        let colour=u.product_lights[k*4u+1u];
        let axis=u.product_lights[k*4u+2u];
        let falloff=u.product_lights[k*4u+3u];
        if source.w == 4.0 {
            radiance += base*colour.xyz*(1.0-parameters.x)/3.141592653589793*ao;
            continue;
        }
        let sample=product_light_sample(k,wp);
        radiance += pbr_brdf(base,parameters,n,v,sample[0])*sample[1]*max(dot(n,sample[0]),0.0);
    }
    if (look_on()) {
        radiance += pbr_studio(base, parameters, n, v, wp, ao) * u.look.y;
        let encoded = look_encode(pbr_neutral(radiance * u.look.x), pixel);
        if (u.back.w > 0.5) { return srgb_decode(encoded); }
        return encoded;
    }
    let mapped = radiance / (vec3<f32>(1.0) + radiance);
    if (u.back.w > 0.5) { return mapped; }
    return pbr_encode(mapped);
}
"#
);
