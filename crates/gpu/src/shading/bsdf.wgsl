// Shared shading library: bsdf (crates/gpu/src/shading/mod.rs). Pure functions, `sc_` names,
// WebGL2-safe. The opaque surface of `solidcraft_render::surface` and its clear coat
// (`clear_coat.rs`), extracted from the path tracer's expressions: a GGX specular lobe with
// separable Smith shadowing and Schlick Fresnel, a Fresnel-weighted Lambert diffuse layer, and an
// optional single-scattering GGX dielectric coat that attenuates the base at entry and exit by an
// upper bound of its directional reflectance. Directions are unit vectors away from the surface.
// `parameters` is (metallic, perceptual roughness, dielectric IOR); `coat` is (weight, perceptual
// roughness, IOR), where weight 0 or IOR 1 is no coat.

const sc_pi: f32 = 3.141592653589793;

// `v` at unit length, scaled first so tiny and huge vectors stay finite; zero gives +Z.
fn sc_unit(v: vec3<f32>) -> vec3<f32> {
    let scale = max(max(abs(v.x), abs(v.y)), abs(v.z));
    if (scale == 0.0) { return vec3<f32>(0.0, 0.0, 1.0); }
    return normalize(v / scale);
}

// Schlick's Fresnel from normal-incidence reflectance `f0` at cosine `c` (clamped to [0, 1]).
fn sc_fresnel(f0: vec3<f32>, c: f32) -> vec3<f32> {
    let t = 1.0 - clamp(c, 0.0, 1.0);
    return f0 + (vec3<f32>(1.0) - f0) * (t * t * t * t * t);
}

// The base's Schlick Fresnel. An index-matched dielectric (IOR 1) has no interface reflection
// at any angle; metal fractions keep their independently weighted grazing limit.
fn sc_base_fresnel(f0: vec3<f32>, cosine: f32, metallic: f32, ior: f32) -> vec3<f32> {
    let x = 1.0 - clamp(cosine, 0.0, 1.0);
    let grazing = select(1.0, metallic, ior == 1.0);
    return f0 + (vec3<f32>(grazing) - f0) * (x * x * x * x * x);
}

// The base's normal-incidence reflectance: the dielectric's from its IOR, mixed toward the base
// colour by metalness.
fn sc_base_f0(base: vec3<f32>, metallic: f32, ior: f32) -> vec3<f32> {
    let ratio = (ior - 1.0) / (ior + 1.0);
    return mix(vec3<f32>(ratio * ratio), base, metallic);
}

// The GGX (Trowbridge-Reitz) normal distribution at cos(n, h) = `nh`, for α² = `a2`
// (α = perceptual roughness²).
fn sc_ggx_d(nh: f32, a2: f32) -> f32 {
    let d = (1.0 - nh * nh) + nh * nh * a2;
    return a2 / (sc_pi * d * d);
}

// One direction's separable Smith shadowing for GGX over 4·cos (the height-uncorrelated G1
// divided by 2·`c`): the BRDF's specular is F·D·¼·G(nv)·G(nl). `c` is positive.
fn sc_smith_g1(c: f32, a2: f32) -> f32 {
    return 2.0 / (c + sqrt(a2 + (1.0 - a2) * c * c));
}

// The base BRDF (no coat): GGX specular plus the diffuse layer, weighted by what the specular
// interface passes at entry and exit. Zero below the horizon of `n` on either side.
fn sc_base_brdf(base: vec3<f32>, parameters: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let nv = clamp(dot(n, v), -1.0, 1.0);
    let nl = clamp(dot(n, l), -1.0, 1.0);
    if (nv <= 0.0 || nl <= 0.0) { return vec3<f32>(0.0); }
    let h = sc_unit(v + l);
    let a = parameters.y * parameters.y;
    let a2 = a * a;
    let d = sc_ggx_d(clamp(dot(n, h), 0.0, 1.0), a2);
    let gv = sc_smith_g1(nv, a2);
    let gl = sc_smith_g1(nl, a2);
    let f0 = sc_base_f0(base, parameters.x, parameters.z);
    let diffuse = base * ((1.0 - parameters.x) / sc_pi);
    return sc_base_fresnel(f0, dot(v, h), parameters.x, parameters.z) * (d * 0.25 * gv * gl)
        + (vec3<f32>(1.0) - sc_base_fresnel(f0, nv, parameters.x, parameters.z)) * (vec3<f32>(1.0) - sc_base_fresnel(f0, nl, parameters.x, parameters.z)) * diffuse;
}

// An upper bound of a coat's directional reflectance at `cosine`: Schlick at the smallest
// cos(v, h) any reflected direction can have.
fn sc_coat_bound(f0: vec3<f32>, cosine: f32) -> vec3<f32> {
    let minimum_vh = cosine / sqrt(2.0 * (1.0 + sqrt(max(0.0, 1.0 - cosine * cosine))));
    return sc_fresnel(f0, minimum_vh);
}

// The coat's normal-incidence reflectance from its IOR.
fn sc_coat_f0(ior: f32) -> vec3<f32> {
    let ratio = (ior - 1.0) / (ior + 1.0);
    return vec3<f32>(ratio * ratio);
}

// The whole opaque BRDF: the base under an optional clear coat (`clear_coat.rs`).
fn sc_coated_brdf(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let under = sc_base_brdf(base, parameters, n, v, l);
    if (coat.x <= 0.0 || coat.z == 1.0) { return under; }
    let nv = clamp(dot(n, v), -1.0, 1.0);
    let nl = clamp(dot(n, l), -1.0, 1.0);
    if (nv <= 0.0 || nl <= 0.0) { return under; }
    let h = sc_unit(v + l);
    let a = coat.y * coat.y;
    let a2 = a * a;
    let d = sc_ggx_d(clamp(dot(n, h), 0.0, 1.0), a2);
    let gv = sc_smith_g1(nv, a2);
    let gl = sc_smith_g1(nl, a2);
    let f0 = sc_coat_f0(coat.z);
    let attenuation = (vec3<f32>(1.0) - coat.x * sc_coat_bound(f0, nv)) * (vec3<f32>(1.0) - coat.x * sc_coat_bound(f0, nl));
    return under * attenuation + coat.x * sc_fresnel(f0, dot(v, h)) * (d * 0.25 * gv * gl);
}
