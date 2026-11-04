//! Original GPU translation of the L1 opaque surface and fixed studio reference.
//! Appended to the existing viewport shader so camera/output conventions are shared.

/// A PBR vertex: position, normal, linear base colour and (metallic, roughness, IOR), three
/// floats each. See `GpuScene::pbr_tri`.
pub const VERTEX_SIZE: usize = 48;

/// The PBR pipeline's bind group for the product look's studio (`view::ibl`).
pub const STUDIO_GROUP: u32 = 3;

/// The shading every PBR surface shares (docs/render-opaque-pbr.md): the studio bindings, the
/// BRDF, coat and environment terms, `Surface` and `pbr_shade`. A fragment entry builds a
/// `PbrFragment`, asks a material for the `Surface` there and returns `pbr_shade`'s colour.
macro_rules! shade_source {
    () => {
        r#"
// The product look's studio (`view::ibl`, group 3): the prefiltered radiance cube (level m at
// perceptual roughness m / levels.x), its sampler, the cyclorama's irradiance polynomial over
// (1, x, y, z, xy, yz, xz, z², x² − y²), the light count (levels.y), the room (half size, floor,
// ceiling, in subject extents) and the rig's lights, four `vec4`s each: (centre, half width),
// (width axis, half height), (height axis, 0), (radiance, 0). Read only while `look_on()`.
@group(3) @binding(0) var studio_cube: texture_cube<f32>;
@group(3) @binding(1) var studio_smp: sampler;
struct Studio { irradiance: array<vec4<f32>, 9>, levels: vec4<f32>, room: vec4<f32>, lights: array<vec4<f32>, 16>, };
@group(3) @binding(2) var<uniform> studio: Studio;
fn pbr_encode(c: vec3<f32>) -> vec3<f32> {
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055), c > vec3<f32>(0.0031308));
}

// The viewport's base BRDF: the shared library's (`sc_base_brdf`), under the name its oracle
// probes call.
fn pbr_brdf(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    return sc_base_brdf(base, parameters, n, v, l);
}

fn pbr_poly(c0: vec4<f32>, c1: vec4<f32>, c2: vec4<f32>, r: f32, t: vec4<f32>) -> f32 {
    return dot(c0, t) + r * (dot(c1, t) + r * dot(c2, t));
}

// The split-sum scale and bias (x: A, y: B; albedo F0·A + F90·B) of `sc_base_brdf` in a uniform
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
// ½ |Σ θᵢ (n · γ̂ᵢ)| (`ibl::rig::polygon_irradiance` on the CPU).
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

// The anisotropic studio lobe's spread per tap step along the tangent, in units of the extra
// width √(αt² − αl²) over the taps' own lobe αl: calibrated so that the taps (Gaussian weights
// over offsets −2..2 steps of 0.75) widen the taps' prefiltered lobe to the major axis's half
// width (`render::anisotropic_surface`, f64, checked in `view/ibl/tests.rs`).
const PBR_STRETCH_SCALE: f32 = 1.28;
// The taps' own lobe is at least this share of the major axis's α, so that neighbouring taps
// overlap (five sharp copies would ghost); across the tangent a strongly anisotropic lobe is
// therefore that much wider than its minor axis, though still sharper than the isotropic one.
const PBR_STRETCH_FLOOR: f32 = 0.36;

// The prefiltered studio for an anisotropic GGX lobe of perceptual `roughness` and `anisotropy`
// (OpenPBR's axes, `sc_aniso_alpha`), reflected along `lookup` (studio frame; a direction or a
// room hit point, not necessarily unit) and stretched along the unit studio-frame `tangent`: five
// fixed taps along the tangent's direction perpendicular to the lookup, each read at the level of
// αl = max(αb, `PBR_STRETCH_FLOOR`·αt), with normalised Gaussian weights, so a uniform studio
// returns itself exactly. A tangent along the lookup has no visible stretch: one tap at the
// lobe's mean level.
fn pbr_stretched_studio(lookup: vec3<f32>, tangent: vec3<f32>, roughness: f32, anisotropy: f32) -> vec3<f32> {
    let d = sc_unit(lookup);
    let side = tangent - d * dot(tangent, d);
    if (dot(side, side) < 0.000001) {
        return textureSampleLevel(studio_cube, studio_smp, d, roughness * studio.levels.x).rgb;
    }
    let across = sc_unit(side);
    let alpha = sc_aniso_alpha(roughness, anisotropy);
    let own = max(alpha.y, PBR_STRETCH_FLOOR * alpha.x);
    let level = clamp(sqrt(own), 0.0, 1.0) * studio.levels.x;
    let spread = PBR_STRETCH_SCALE * sqrt(max(alpha.x * alpha.x - own * own, 0.0));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < 5u; k += 1u) {
        let o = (f32(k) - 2.0) * 0.75;
        let w = exp(-0.5 * o * o);
        let a = clamp(o * spread, -1.5, 1.5);
        sum += w * textureSampleLevel(studio_cube, studio_smp, d * cos(a) + across * sin(a), level).rgb;
        total += w;
    }
    return sum / total;
}

// The studio's light reflected toward `v`, in parts: the base's specular, the coat's specular
// and the diffuse layer. Split-sum specular with multiple-scattering compensation; diffuse from
// the irradiance polynomial weighted by what the specular lobe leaves; a coat reflects its own
// single-scattering albedo w·(F0·A + B) at its roughness and passes the rest to the base
// (`ibl::reference::shade` on the CPU).
struct PbrIbl { specular: vec3<f32>, coat: vec3<f32>, diffuse: vec3<f32>, };

fn pbr_ibl_parts(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, rotation: f32) -> PbrIbl {
    let r = 2.0 * max(dot(n, v), 0.0001) * n - v;
    return pbr_studio_parts(base, parameters, coat, n, v, pbr_turn(r, rotation), pbr_irradiance(pbr_turn(n, rotation)), 1.0);
}

// `pbr_ibl_parts` with the inputs a material graph adds (see `pbr_studio_layers`): the coat's
// colour and normal, the direction `r` the base's specular looks up from, and its anisotropic
// `stretch` = (unit world tangent, anisotropy), (0, 0, 0, 0) when isotropic.
fn pbr_ibl_layers(base: vec3<f32>, parameters: vec3<f32>, specular: vec4<f32>, coat: vec3<f32>, coat_color: vec3<f32>, n: vec3<f32>, coat_n: vec3<f32>, v: vec3<f32>, r: vec3<f32>, rotation: f32, stretch: vec4<f32>) -> PbrIbl {
    let rc = 2.0 * max(dot(coat_n, v), 0.0001) * coat_n - v;
    return pbr_studio_layers(base, parameters, specular, coat, coat_color, n, coat_n, v, pbr_turn(r, rotation), pbr_turn(rc, rotation), pbr_irradiance(pbr_turn(n, rotation)), 1.0, 1.0, pbr_stretch(stretch, rotation));
}

// An anisotropic `stretch` = (world tangent, anisotropy) in the studio's frame.
fn pbr_stretch(stretch: vec4<f32>, rotation: f32) -> vec4<f32> {
    if (!(stretch.w > 0.0)) { return vec4<f32>(0.0); }
    return vec4<f32>(pbr_turn(stretch.xyz, rotation), stretch.w);
}

// The studio's parts at a world point `wp` in the room: the reflection box-projected against the
// room and darkened where it meets the floor under the subject, and the diffuse layer lit by the
// cyclorama and the rig's lights.
fn pbr_room_parts(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, rotation: f32, wp: vec3<f32>) -> PbrIbl {
    let r = 2.0 * max(dot(n, v), 0.0001) * n - v;
    let q = pbr_studio_point(wp, rotation);
    let ns = pbr_turn(n, rotation);
    let hit = pbr_room_exit(q, pbr_turn(r, rotation));
    let irradiance = pbr_irradiance(ns) + pbr_rig_irradiance(q, ns);
    return pbr_studio_parts(base, parameters, coat, n, v, hit, irradiance, pbr_floor_contact(hit, rotation));
}

// `pbr_room_parts` with the inputs a material graph adds: the base's reflection `r` (the mirror
// direction; an anisotropic base's spreads by `stretch`) and the coat's own mirror direction
// about `coat_n` are each box-projected and floor-darkened; the coat colour tints the base; an
// anisotropic base's `stretch` (as `pbr_ibl_layers`) spreads its lookup along its tangent.
fn pbr_room_layers(base: vec3<f32>, parameters: vec3<f32>, specular: vec4<f32>, coat: vec3<f32>, coat_color: vec3<f32>, n: vec3<f32>, coat_n: vec3<f32>, v: vec3<f32>, r: vec3<f32>, rotation: f32, wp: vec3<f32>, stretch: vec4<f32>) -> PbrIbl {
    let q = pbr_studio_point(wp, rotation);
    let ns = pbr_turn(n, rotation);
    let hit = pbr_room_exit(q, pbr_turn(r, rotation));
    let contact = pbr_floor_contact(hit, rotation);
    var coat_hit = hit;
    var coat_contact = contact;
    if (coat.x > 0.0 && coat.z != 1.0 && (any(coat_n != n) || pbr_bent(r, n, v))) {
        let rc = 2.0 * max(dot(coat_n, v), 0.0001) * coat_n - v;
        coat_hit = pbr_room_exit(q, pbr_turn(rc, rotation));
        coat_contact = pbr_floor_contact(coat_hit, rotation);
    }
    let irradiance = pbr_irradiance(ns) + pbr_rig_irradiance(q, ns);
    return pbr_studio_layers(base, parameters, specular, coat, coat_color, n, coat_n, v, hit, coat_hit, irradiance, contact, coat_contact, pbr_stretch(stretch, rotation));
}

// Whether `r` is not the mirror direction of `v` about `n` (an anisotropic base's bent lookup).
fn pbr_bent(r: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> bool {
    return any(r != 2.0 * max(dot(n, v), 0.0001) * n - v);
}

// The studio's parts given where the reflection looks in the cube (`lookup`, studio frame), the
// diffuse irradiance and the reflection's floor contact.
fn pbr_studio_parts(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, lookup: vec3<f32>, irradiance: vec3<f32>, contact: f32) -> PbrIbl {
    return pbr_studio_layers(base, parameters, vec4<f32>(1.0), coat, vec3<f32>(1.0), n, n, v, lookup, lookup, irradiance, contact, contact, vec4<f32>(0.0));
}

// `pbr_studio_parts` with a material graph's specular tint `tint` = (weight, colour) and coat: its colour tints the base on the way in and
// out (`sc_coat_tint`), and it reflects about its own unit normal `coat_n`, looking up
// `coat_lookup` with its floor contact. An anisotropic base (`stretch` = (unit studio-frame
// tangent, anisotropy > 0)) reads the studio through `pbr_stretched_studio`. The base's split-sum
// albedo stays the isotropic one at its roughness, whatever its lookup.
fn pbr_studio_layers(base: vec3<f32>, parameters: vec3<f32>, tint: vec4<f32>, coat: vec3<f32>, coat_color: vec3<f32>, n: vec3<f32>, coat_n: vec3<f32>, v: vec3<f32>, lookup: vec3<f32>, coat_lookup: vec3<f32>, irradiance: vec3<f32>, contact: f32, coat_contact: f32, stretch: vec4<f32>) -> PbrIbl {
    let nv = max(dot(n, v), 0.0001);
    let metallic = parameters.x;
    let roughness = parameters.y;
    let ratio = (parameters.z - 1.0) / (parameters.z + 1.0);
    let f0 = mix(vec3<f32>(ratio * ratio), base, metallic);
    let f90 = select(1.0, metallic, parameters.z == 1.0);
    let ab = pbr_env_brdf(nv, roughness);
    var single = f0 * ab.x + vec3<f32>(f90 * ab.y);
    var compensation = vec3<f32>(1.0) + f0 * (1.0 / max(ab.x + ab.y, 0.0001) - 1.0);
    // A specular tint off its default (`sc_specular_f0`, `sc_specular_f90`).
    if (any(tint != vec4<f32>(1.0))) {
        let tinted = sc_specular_f0(base, metallic, parameters.z, tint);
        single = tinted * ab.x + sc_specular_f90(metallic, parameters.z, tint) * ab.y;
        compensation = vec3<f32>(1.0) + tinted * (1.0 / max(ab.x + ab.y, 0.0001) - 1.0);
    }
    var prefiltered: vec3<f32>;
    if (stretch.w > 0.0) {
        prefiltered = pbr_stretched_studio(lookup, stretch.xyz, roughness, stretch.w) * contact;
    } else {
        prefiltered = textureSampleLevel(studio_cube, studio_smp, lookup, roughness * studio.levels.x).rgb * contact;
    }
    // The diffuse layer takes what the specular lobe does not reflect, so a white lossless
    // surface reflects exactly what it receives.
    let specular = single * compensation;
    let diffuse = base * ((1.0 - metallic) / 3.141592653589793) * max(vec3<f32>(1.0) - specular, vec3<f32>(0.0));
    var out = PbrIbl(prefiltered * specular, vec3<f32>(0.0), diffuse * irradiance);
    if (coat.x > 0.0 && coat.z != 1.0) {
        let cv = max(dot(coat_n, v), 0.0001);
        let coat_ratio = (coat.z - 1.0) / (coat.z + 1.0);
        let cab = pbr_env_brdf(cv, coat.y);
        let albedo = coat.x * (coat_ratio * coat_ratio * cab.x + cab.y);
        let seen = textureSampleLevel(studio_cube, studio_smp, coat_lookup, coat.y * studio.levels.x).rgb * coat_contact;
        out = PbrIbl(out.specular * (1.0 - albedo), seen * albedo, out.diffuse * (1.0 - albedo));
    }
    if (coat.x > 0.0 && any(coat_color != vec3<f32>(1.0))) {
        let through = sc_coat_tint(coat.x, coat_color);
        out = PbrIbl(out.specular * (through * through), out.coat, out.diffuse * (through * through));
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

// The look's direct lights' angular half-size (rad) over two: the GGX α they add to a coat.
const LOOK_LIGHT_SPREAD: f32 = 0.1;

// What a fragment is shaded at (`pbr_fragment`): its pixel, world position, interpolated normal
// (not unit; a zero normal is never occluded), the unit direction toward the eye, and the pixel's
// footprint (`fwidth` of the world position) that materials fade their detail by.
struct PbrFragment { pos: vec4<f32>, wp: vec3<f32>, n: vec3<f32>, v: vec3<f32>, footprint: vec3<f32>, };

// The fragment at `pos`. Its derivatives come first, in uniform control flow: a fragment entry
// calls this before any branch.
fn pbr_fragment(pos: vec4<f32>, wp: vec3<f32>, n: vec3<f32>) -> PbrFragment {
    let footprint = fwidth(wp);
    var view = u.back.xyz;
    if (u.eye.w > 0.5) { view = u.eye.xyz - wp; }
    return PbrFragment(pos, wp, n, sc_unit(view), footprint);
}

// One evaluated surface point, as a material gives it to `pbr_shade`: the shared library's
// `sc_Surface` (the WGSL mirror of `material_graph::SurfaceSample`), which both GPU targets of the
// material system fill.
alias Surface = sc_Surface;

// `SurfaceSample::from(OpaqueSurface)`: base colour, (metallic, roughness, IOR) and the coat's
// (weight, roughness, IOR), every other input at its default, both normals `n`.
fn surface_from_opaque(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>) -> Surface {
    let one = vec3<f32>(1.0);
    return Surface(base, parameters.x, parameters.y, parameters.z, coat.x, coat.y, coat.z, n, n, 1.0, one, 0.0, 0.0, vec3<f32>(0.0), one, 0.0, one, 0.0, one, 1.0, 1.0);
}

// The lighting of every PBR surface, whatever material evaluated `s`: without the product look
// the fixed studio (or the frame's product lights), per-channel Reinhard and one encode; with it
// the product lights, the studio (occluded), exposure, PBR Neutral and a dithered encode.
// Every light reflects through `sc_layered_brdf` (an anisotropic base along `s.tangent`, a tinted
// specular, a tinted coat about `s.coat_normal`); emission adds after the lighting, before
// exposure. Alpha is 1, except for a transmissive surface (`sc_see_through`) or a cut-out
// (`s.opacity` < 1, `sc_cover`): its colour is then premultiplied, for a blended "over"
// (src + dst·(1 − a)) like today's see-through glass, without refraction.
fn pbr_shade(s: Surface, f: PbrFragment) -> vec4<f32> {
    let n = s.normal;
    let coat_n = s.coat_normal;
    let v = f.v;
    let parameters = vec3<f32>(s.metalness, s.roughness, s.ior);
    let coat = vec3<f32>(s.coat_weight, s.coat_roughness, s.coat_ior);
    let specular = vec4<f32>(s.specular_weight, s.specular_color);
    // An anisotropic lobe's direction; zero (isotropic) without anisotropy or a usable tangent.
    var t = vec3<f32>(0.0);
    if (s.anisotropy > 0.0) { t = sc_aniso_tangent(n, s.tangent, s.rotation); }
    let see = sc_see_through(s.base_color, s.transmission_weight, s.transmission_color);
    // Without the look: no occlusion.
    var base = see.rgb;
    var ao = 1.0;
    let look = look_on();
    if (look) {
        base = clamp(base, vec3<f32>(0.0), vec3<f32>(1.0));
        // Occlusion darkens only the indirect light: the studio and the ambient kind.
        ao = occlusion(TOut(f.pos, f.n, vec4<f32>(base, 1.0), f.wp, 0u));
    }
    let ambient = select(1.0, ao * pbr_coat_keep(coat, dot(coat_n, v)), look);
    let coat_tint = sc_coat_tint(coat.x, s.coat_color);
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
        radiance += sc_layered_brdf(base, parameters, specular, s.anisotropy, t, coat, s.coat_color, n, coat_n, v, light.xyz) * (light.w * max(dot(n, light.xyz), 0.0));
    }
    for (var k = 0u; k < min(u32(u.product_lights_meta.y),21u) && u.product_lights_meta.x > 0.0; k += 1u) {
        let source=u.product_lights[k*4u];
        let colour=u.product_lights[k*4u+1u];
        let axis=u.product_lights[k*4u+2u];
        let falloff=u.product_lights[k*4u+3u];
        if source.w == 4.0 {
            radiance += base*colour.xyz*(1.0-parameters.x)/3.141592653589793*ambient*(coat_tint*coat_tint);
            continue;
        }
        let sample=product_light_sample(k,f.wp);
        radiance += sc_layered_brdf(base,parameters,specular,s.anisotropy,t,lit_coat,s.coat_color,n,coat_n,v,sample[0])*sample[1]*max(dot(n,sample[0]),0.0);
    }
    if (look) {
        // The product look: the studio (occluded), exposure, PBR Neutral and a dithered encode.
        let p = look_params();
        // An anisotropic base spreads its studio lookup along its tangent (`pbr_stretched_studio`)
        // about the mirror direction.
        let r = 2.0 * max(dot(n, v), 0.0001) * n - v;
        var stretch = vec4<f32>(0.0);
        if (s.anisotropy > 0.0 && dot(t, t) > 0.0) { stretch = vec4<f32>(t, s.anisotropy); }
        let parts = pbr_room_layers(base, parameters, specular, coat, s.coat_color, n, coat_n, v, r, p.z, f.wp, stretch);
        let nv = dot(n, v);
        let indirect = parts.diffuse * ao + parts.specular * pbr_specular_occlusion(nv, ao, parameters.y)
            + parts.coat * pbr_specular_occlusion(dot(coat_n, v), ao, coat.y);
        radiance += indirect * p.y;
        radiance += s.emission_color * s.emission_luminance;
        let encoded = look_encode(look_tone(radiance * p.x), f.pos.xy);
        // An sRGB target re-encodes what it stores: hand it the exact inverse of the dithered
        // encode (`srgb_decode`, with the straight segment), so the stored bytes are `encoded`.
        // `to_linear` skips that segment and lifted dark channels (≤ 10/255) up to 12.92×, which
        // the dither flipped pixel by pixel into coloured stripes.
        if (u.back.w > 0.5) { return sc_cover(vec4<f32>(srgb_decode(encoded), see.a), s.opacity); }
        return sc_cover(vec4<f32>(encoded, see.a), s.opacity);
    }
    radiance += s.emission_color * s.emission_luminance;
    let mapped = radiance / (vec3<f32>(1.0) + radiance);
    if (u.back.w > 0.5) { return sc_cover(vec4<f32>(mapped, see.a), s.opacity); }
    return sc_cover(vec4<f32>(pbr_encode(mapped), see.a), s.opacity);
}
"#
    };
}

/// The PBR vertex stream ([`VERTEX_SIZE`]): its vertex entry, its occlusion fragments, and the
/// built-in "vertex constant" `material_eval` behind `fs_pbr`.
macro_rules! vertex_source {
    () => {
        r#"
struct PbrOut {
    // Invariant as `TOut`'s: the AO term pass matches the prepass's depth exactly.
    @invariant @builtin(position) pos: vec4<f32>,
    @location(0) n: vec3<f32>,
    @location(1) base: vec3<f32>,
    @location(2) wp: vec3<f32>,
    @location(3) parameters: vec3<f32>,
};

// A PBR vertex (`pbr::VERTEX_SIZE`).
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

/// The AO prepass's depth of a PBR surface (drawn there only with the product look).
@fragment
fn fs_ao_depth_pbr(i: PbrOut, @builtin(front_facing) front: bool) -> AoDepth {
    return opaque_depth(TOut(i.pos, i.n, vec4<f32>(i.base, 1.0), i.wp, 0u), front);
}

/// The AO term of a PBR surface, on the prepass's depth (drawn only with the product look).
@fragment
fn fs_ao_term_pbr(i: PbrOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return ao_term(TOut(i.pos, i.n, vec4<f32>(i.base, 1.0), i.wp, 0u), front);
}

// A material's input at a fragment of a PBR vertex stream: the fragment and the vertex's
// constants, its linear base colour and (metallic, roughness, IOR).
struct MatCtx { frag: PbrFragment, base: vec3<f32>, parameters: vec3<f32>, };

// The built-in "vertex constant" material: the vertex's base colour and optical parameters at the
// fragment's unit normal, uncoated (PBR vertices carry no coat).
fn material_eval(c: MatCtx) -> Surface {
    return surface_from_opaque(c.base, c.parameters, vec3<f32>(0.0, 1.0, 1.0), sc_unit(c.frag.n));
}

@fragment
fn fs_pbr(i: PbrOut) -> @location(0) vec4<f32> {
    let f = pbr_fragment(i.pos, i.wp, i.n);
    return pbr_shade(material_eval(MatCtx(f, i.base, i.parameters)), f);
}
"#
    };
}

/// [`SOURCE`]'s shared shading alone, for shaders that bring their own material and entries
/// (material graphs' generated shaders).
pub const SHADE: &str = concat!(crate::shading::library_source!(), "\n", shade_source!());

/// The PBR shader appended to the viewport's: the shared shading library (`crate::shading`), the
/// shared shading, then the vertex stream.
pub const SOURCE: &str = concat!(crate::shading::library_source!(), "\n", shade_source!(), vertex_source!());
