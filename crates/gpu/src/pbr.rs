//! Original GPU translation of the L1 opaque surface and fixed studio reference.
//! Appended to the existing viewport shader so camera/output conventions are shared.

pub const VERTEX_SIZE: usize = 48;

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

// The studio's light reflected toward `v`: split-sum specular with multiple-scattering
// compensation, and diffuse from the irradiance polynomial weighted by what the specular lobe
// leaves (`ibl::reference::shade` on the CPU).
fn pbr_ibl(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, rotation: f32) -> vec3<f32> {
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
    return prefiltered * specular + diffuse * pbr_irradiance(pbr_turn(n, rotation));
}

@fragment
fn fs_pbr(i: PbrOut) -> @location(0) vec4<f32> {
    let n = pbr_unit(i.n);
    var view = u.back.xyz;
    if (u.eye.w > 0.5) { view = u.eye.xyz - i.wp; }
    let v = pbr_unit(view);
    let lights = array<vec4<f32>, 3>(vec4<f32>(0.6, 0.0, 0.8, 3.0), vec4<f32>(-0.8, 0.0, 0.6, 1.0), vec4<f32>(0.0, -0.6, 0.8, 1.5));
    var radiance = vec3<f32>(0.0);
    for (var k = 0u; k < 3u && u.product_lights_meta.x == 0.0; k += 1u) {
        let light = lights[k];
        radiance += pbr_brdf(i.base, i.parameters, n, v, light.xyz) * (light.w * max(dot(n, light.xyz), 0.0));
    }
    for (var k = 0u; k < min(u32(u.product_lights_meta.y),21u) && u.product_lights_meta.x > 0.0; k += 1u) {
        let source=u.product_lights[k*4u];
        let colour=u.product_lights[k*4u+1u];
        let axis=u.product_lights[k*4u+2u];
        let falloff=u.product_lights[k*4u+3u];
        if source.w == PRODUCT_LOOK_KIND { continue; }
        if source.w == 4.0 {
            radiance += i.base*colour.xyz*(1.0-i.parameters.x)/3.141592653589793;
            continue;
        }
        let sample=product_light_sample(k,i.wp);
        radiance += pbr_brdf(i.base,i.parameters,n,v,sample[0])*sample[1]*max(dot(n,sample[0]),0.0);
    }
    if (look_on()) {
        // The product look: the studio, exposure, PBR Neutral and a dithered encode.
        let look = look_params();
        radiance += pbr_ibl(i.base, i.parameters, n, v, look.z) * look.y;
        let encoded = look_encode(look_tone(radiance * look.x), i.pos.xy);
        if (u.back.w > 0.5) { return vec4<f32>(to_linear(encoded), 1.0); }
        return vec4<f32>(encoded, 1.0);
    }
    let mapped = radiance / (vec3<f32>(1.0) + radiance);
    if (u.back.w > 0.5) { return vec4<f32>(mapped, 1.0); }
    return vec4<f32>(pbr_encode(mapped), 1.0);
}
"#;
