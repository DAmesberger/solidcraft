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
// perceptual roughness m / levels.x), its sampler, and the irradiance polynomial over
// (1, x, y, z, xy, yz, xz, z², x² − y²). Read only while `look_on()`.
@group(3) @binding(0) var studio_cube: texture_cube<f32>;
@group(3) @binding(1) var studio_smp: sampler;
struct Studio { irradiance: array<vec4<f32>, 9>, levels: vec4<f32>, };
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

// One evaluated surface point, as a material gives it to `pbr_shade`: the WGSL mirror of
// `solidcraft_render::material_graph::SurfaceSample` (the `OpaqueSurface` core, its clear coat,
// and the inputs the core lacks), with the point's shading frame. Colours are linear. Today's
// `pbr_shade` reads the core, the coat and `normal`; the other inputs keep their
// `surface_from_opaque` defaults until the shading that reads them lands.
struct Surface {
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

// `SurfaceSample::from(OpaqueSurface)`: base colour, (metallic, roughness, IOR) and the coat's
// (weight, roughness, IOR), every other input at its default, both normals `n`.
fn surface_from_opaque(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>) -> Surface {
    let one = vec3<f32>(1.0);
    return Surface(base, parameters.x, parameters.y, parameters.z, coat.x, coat.y, coat.z, n, n, 1.0, one, 0.0, 0.0, vec3<f32>(0.0), one, 0.0, one, 0.0, one, 1.0, 1.0);
}

// The lighting of every PBR surface, whatever material evaluated `s`: without the product look
// the fixed studio (or the frame's product lights), per-channel Reinhard and one encode; with it
// the product lights, the studio (occluded), exposure, PBR Neutral and a dithered encode.
fn pbr_shade(s: Surface, f: PbrFragment) -> vec4<f32> {
    let n = s.normal;
    let v = f.v;
    let parameters = vec3<f32>(s.metalness, s.roughness, s.ior);
    let coat = vec3<f32>(s.coat_weight, s.coat_roughness, s.coat_ior);
    // Without the look: no occlusion.
    var base = s.base_color;
    var ao = 1.0;
    let look = look_on();
    if (look) {
        base = clamp(base, vec3<f32>(0.0), vec3<f32>(1.0));
        // Occlusion darkens only the indirect light: the studio and the ambient kind.
        ao = occlusion(TOut(f.pos, f.n, vec4<f32>(base, 1.0), f.wp));
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
        radiance += sc_coated_brdf(base, parameters, coat, n, v, light.xyz) * (light.w * max(dot(n, light.xyz), 0.0));
    }
    for (var k = 0u; k < min(u32(u.product_lights_meta.y),21u) && u.product_lights_meta.x > 0.0; k += 1u) {
        let source=u.product_lights[k*4u];
        let colour=u.product_lights[k*4u+1u];
        let axis=u.product_lights[k*4u+2u];
        let falloff=u.product_lights[k*4u+3u];
        if source.w == 4.0 {
            radiance += base*colour.xyz*(1.0-parameters.x)/3.141592653589793*ambient;
            continue;
        }
        let sample=product_light_sample(k,f.wp);
        radiance += sc_coated_brdf(base,parameters,lit_coat,n,v,sample[0])*sample[1]*max(dot(n,sample[0]),0.0);
    }
    if (look) {
        // The product look: the studio (occluded), exposure, PBR Neutral and a dithered encode.
        let p = look_params();
        let parts = pbr_ibl_parts(base, parameters, coat, n, v, p.z);
        let nv = dot(n, v);
        let indirect = parts.diffuse * ao + parts.specular * pbr_specular_occlusion(nv, ao, parameters.y)
            + parts.coat * pbr_specular_occlusion(nv, ao, coat.y);
        radiance += indirect * p.y;
        let encoded = look_encode(look_tone(radiance * p.x), f.pos.xy);
        if (u.back.w > 0.5) { return vec4<f32>(to_linear(encoded), 1.0); }
        return vec4<f32>(encoded, 1.0);
    }
    let mapped = radiance / (vec3<f32>(1.0) + radiance);
    if (u.back.w > 0.5) { return vec4<f32>(mapped, 1.0); }
    return vec4<f32>(pbr_encode(mapped), 1.0);
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
    return opaque_depth(TOut(i.pos, i.n, vec4<f32>(i.base, 1.0), i.wp), front);
}

/// The AO term of a PBR surface, on the prepass's depth (drawn only with the product look).
@fragment
fn fs_ao_term_pbr(i: PbrOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return ao_term(TOut(i.pos, i.n, vec4<f32>(i.base, 1.0), i.wp), front);
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
/// (material graphs' generated shaders, which drop the gate when they use it).
#[cfg(test)]
pub const SHADE: &str = concat!(crate::shading::library_source!(), "\n", shade_source!());

/// The PBR shader appended to the viewport's: the shared shading library (`crate::shading`), the
/// shared shading, then the vertex stream.
pub const SOURCE: &str = concat!(crate::shading::library_source!(), "\n", shade_source!(), vertex_source!());
