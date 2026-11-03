// Shared shading library: bsdf (crates/gpu/src/shading/mod.rs). Pure functions, `sc_` names,
// WebGL2-safe. The opaque surface of `solidcraft_render::surface` and its clear coat
// (`clear_coat.rs`), extracted from the path tracer's expressions: a GGX specular lobe with
// separable Smith shadowing and Schlick Fresnel, a Fresnel-weighted Lambert diffuse layer, and an
// optional single-scattering GGX dielectric coat that attenuates the base at entry and exit by an
// upper bound of its directional reflectance. Directions are unit vectors away from the surface.
// `parameters` is (metallic, perceptual roughness, dielectric IOR); `coat` is (weight, perceptual
// roughness, IOR), where weight 0 or IOR 1 is no coat.
//
// The viewport's additions for material graphs (docs/material-graph.md §5.5, track C2): a coat
// colour that tints the base on entry and exit, a coat normal of its own, an anisotropic GGX base
// lobe (OpenPBR's roughness axes) for direct light, the bent reflection direction the studio
// looks up for it, and the see-through fallback for transmission. The path tracer has no lobe
// for anisotropy or coat colour (it approximates them, decision O10); with their defaults every
// addition reduces to the functions above exactly.

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

// What passes a coat of colour `coat_color` once, at coverage `weight`: a thin absorbing layer
// tints the light by its colour on each pass (entry and exit), and an uncoated share passes as is.
fn sc_coat_tint(weight: f32, coat_color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) - weight * (vec3<f32>(1.0) - coat_color);
}

// A coat over `under` (the base BRDF already evaluated): the single-scattering GGX dielectric
// coat about its own unit normal `n` (the coat normal), attenuating the base at entry and exit by
// the upper bound of its directional reflectance and tinting it by `coat_color` on both passes
// (`sc_coat_tint`). `coat` is (weight, perceptual roughness, IOR): weight 0 is no coat, IOR 1 a
// coat without an interface (it only tints). A white coat (`coat_color` 1) is `clear_coat.rs`'s.
fn sc_coat_layer(under: vec3<f32>, coat: vec3<f32>, coat_color: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    if (coat.x <= 0.0) { return under; }
    let tint = sc_coat_tint(coat.x, coat_color);
    if (coat.z == 1.0) { return under * (tint * tint); }
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
    return under * attenuation * (tint * tint) + coat.x * sc_fresnel(f0, dot(v, h)) * (d * 0.25 * gv * gl);
}

// The whole opaque BRDF: the base under an optional clear coat (`clear_coat.rs`), both about `n`.
fn sc_coated_brdf(base: vec3<f32>, parameters: vec3<f32>, coat: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    return sc_coat_layer(sc_base_brdf(base, parameters, n, v, l), coat, vec3<f32>(1.0), n, v, l);
}

// Anisotropic GGX (`solidcraft_render::anisotropic_surface`, its oracle; Heitz, JCGT 7(4), 2018,
// equations 1 and 2, mathematics only). Local directions are in the frame (t, b, n), b = n × t.

// The smallest α either axis takes: Solidcraft's lower perceptual roughness bound, squared.
const sc_min_alpha: f32 = 0.002025;

// OpenPBR's anisotropic roughness: (αt, αb) from perceptual roughness and anisotropy in [0, 1],
// αt = r²·√(2 / (1 + (1 − a)²)) and αb = (1 − a)·αt, so αt² + αb² stays 2·α². Each axis keeps at
// least `sc_min_alpha` (a = 1 would be a delta along b). Anisotropy 0 is (r², r²).
fn sc_aniso_alpha(roughness: f32, anisotropy: f32) -> vec2<f32> {
    let a = clamp(anisotropy, 0.0, 1.0);
    let k = 1.0 - a;
    let t = roughness * roughness * sqrt(2.0 / (1.0 + k * k));
    return max(vec2<f32>(t, k * t), vec2<f32>(sc_min_alpha));
}

// The unit tangent of an anisotropic lobe: `tangent` made perpendicular to the unit normal `n`,
// then turned by `rotation` radians counter-clockwise about `n`. Zero when `tangent` has no part
// perpendicular to `n` (or is zero): the lobe then has no direction and is drawn isotropic.
fn sc_aniso_tangent(n: vec3<f32>, tangent: vec3<f32>, rotation: f32) -> vec3<f32> {
    let p = tangent - n * dot(n, tangent);
    let size = max(max(abs(p.x), abs(p.y)), abs(p.z));
    let scale = max(max(abs(tangent.x), abs(tangent.y)), abs(tangent.z));
    if (!(size > 1e-4 * scale)) { return vec3<f32>(0.0); }
    let t = sc_unit(p);
    return sc_unit(t * cos(rotation) + cross(n, t) * sin(rotation));
}

// The anisotropic GGX distribution at the local unit half vector `h` for `alpha` = (αt, αb).
fn sc_aniso_ggx_d(h: vec3<f32>, alpha: vec2<f32>) -> f32 {
    if (h.z <= 0.0) { return 0.0; }
    let x = h.x / alpha.x;
    let y = h.y / alpha.y;
    let q = x * x + y * y + h.z * h.z;
    return 1.0 / (sc_pi * alpha.x * alpha.y * q * q);
}

// One local direction's separable Smith shadowing over 4·cos, as `sc_smith_g1` (G1 / (2·w.z)).
// `w.z` is positive.
fn sc_aniso_smith_g1(w: vec3<f32>, alpha: vec2<f32>) -> f32 {
    let x = alpha.x * w.x;
    let y = alpha.y * w.y;
    return 2.0 / (w.z + sqrt(w.z * w.z + x * x + y * y));
}

// `sc_base_brdf` with an anisotropic specular lobe: `alpha` from `sc_aniso_alpha` along the unit
// tangent `t` (perpendicular to `n`, from `sc_aniso_tangent`). The diffuse layer is the
// isotropic one's. `parameters.y` is unused (the lobe's roughness is `alpha`).
fn sc_aniso_base_brdf(base: vec3<f32>, parameters: vec3<f32>, alpha: vec2<f32>, n: vec3<f32>, t: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let nv = clamp(dot(n, v), -1.0, 1.0);
    let nl = clamp(dot(n, l), -1.0, 1.0);
    if (nv <= 0.0 || nl <= 0.0) { return vec3<f32>(0.0); }
    let b = cross(n, t);
    let h = sc_unit(v + l);
    let d = sc_aniso_ggx_d(vec3<f32>(dot(t, h), dot(b, h), clamp(dot(n, h), 0.0, 1.0)), alpha);
    let gv = sc_aniso_smith_g1(vec3<f32>(dot(t, v), dot(b, v), nv), alpha);
    let gl = sc_aniso_smith_g1(vec3<f32>(dot(t, l), dot(b, l), nl), alpha);
    let f0 = sc_base_f0(base, parameters.x, parameters.z);
    let diffuse = base * ((1.0 - parameters.x) / sc_pi);
    return sc_base_fresnel(f0, dot(v, h), parameters.x, parameters.z) * (d * 0.25 * gv * gl)
        + (vec3<f32>(1.0) - sc_base_fresnel(f0, nv, parameters.x, parameters.z)) * (vec3<f32>(1.0) - sc_base_fresnel(f0, nl, parameters.x, parameters.z)) * diffuse;
}

// The base's specular tint, OpenPBR's `specular_weight` and `specular_color` packed as
// `specular` = (weight, colour): the normal-incidence reflectance, the dielectric's scaled by the
// colour and the metal's base colour, both scaled by the weight. (1, 1, 1, 1) is `sc_base_f0`.
fn sc_specular_f0(base: vec3<f32>, metallic: f32, ior: f32, specular: vec4<f32>) -> vec3<f32> {
    let ratio = (ior - 1.0) / (ior + 1.0);
    let c = clamp(specular.yzw, vec3<f32>(0.0), vec3<f32>(1.0));
    return mix(vec3<f32>(ratio * ratio) * c, base, metallic) * clamp(specular.x, 0.0, 1.0);
}

// The grazing reflectance that goes with `sc_specular_f0`: the dielectric's 1 (0 for an
// index-matched base, the IOR-1 rule) and the metal's edge tint, the specular colour, both
// scaled by the weight. (1, 1, 1, 1) is `sc_base_fresnel`'s grazing limit.
fn sc_specular_f90(metallic: f32, ior: f32, specular: vec4<f32>) -> vec3<f32> {
    let c = clamp(specular.yzw, vec3<f32>(0.0), vec3<f32>(1.0));
    return mix(vec3<f32>(select(1.0, 0.0, ior == 1.0)), c, metallic) * clamp(specular.x, 0.0, 1.0);
}

// Schlick's Fresnel from `f0` toward a grazing reflectance `f90` at cosine `c`.
fn sc_tinted_fresnel(f0: vec3<f32>, f90: vec3<f32>, c: f32) -> vec3<f32> {
    let x = 1.0 - clamp(c, 0.0, 1.0);
    return f0 + (f90 - f0) * (x * x * x * x * x);
}

// The base BRDF with a specular tint (`sc_specular_f0`, `sc_specular_f90`): the same GGX lobe,
// anisotropic along the unit tangent `t` for `alpha` = (αt, αb), or isotropic at α = `alpha.x`
// when `t` is zero, and the diffuse layer weighted by what the tinted interface passes at entry
// and exit. A weaker or coloured specular leaves more to the diffuse layer.
fn sc_tinted_base_brdf(base: vec3<f32>, parameters: vec3<f32>, specular: vec4<f32>, alpha: vec2<f32>, n: vec3<f32>, t: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let nv = clamp(dot(n, v), -1.0, 1.0);
    let nl = clamp(dot(n, l), -1.0, 1.0);
    if (nv <= 0.0 || nl <= 0.0) { return vec3<f32>(0.0); }
    let h = sc_unit(v + l);
    var lobe: f32;
    if (dot(t, t) > 0.0) {
        let b = cross(n, t);
        let d = sc_aniso_ggx_d(vec3<f32>(dot(t, h), dot(b, h), clamp(dot(n, h), 0.0, 1.0)), alpha);
        lobe = d * 0.25 * sc_aniso_smith_g1(vec3<f32>(dot(t, v), dot(b, v), nv), alpha) * sc_aniso_smith_g1(vec3<f32>(dot(t, l), dot(b, l), nl), alpha);
    } else {
        let a2 = alpha.x * alpha.x;
        lobe = sc_ggx_d(clamp(dot(n, h), 0.0, 1.0), a2) * 0.25 * sc_smith_g1(nv, a2) * sc_smith_g1(nl, a2);
    }
    let f0 = sc_specular_f0(base, parameters.x, parameters.z, specular);
    let f90 = sc_specular_f90(parameters.x, parameters.z, specular);
    let diffuse = base * ((1.0 - parameters.x) / sc_pi);
    return sc_tinted_fresnel(f0, f90, dot(v, h)) * lobe
        + (vec3<f32>(1.0) - sc_tinted_fresnel(f0, f90, nv)) * (vec3<f32>(1.0) - sc_tinted_fresnel(f0, f90, nl)) * diffuse;
}

// The viewport's whole surface BRDF: the base (anisotropic along `t` when `anisotropy` > 0 and `t`
// is nonzero; tinted by `specular` = (weight, colour) off its default (1, 1, 1, 1); otherwise
// exactly `sc_base_brdf`) about the base normal `n`, under a tinted coat about the coat normal
// `coat_n` (`sc_coat_layer`). With anisotropy 0, the default specular, a white coat and
// `coat_n` = `n` it is `sc_coated_brdf`.
fn sc_layered_brdf(base: vec3<f32>, parameters: vec3<f32>, specular: vec4<f32>, anisotropy: f32, t: vec3<f32>, coat: vec3<f32>, coat_color: vec3<f32>, n: vec3<f32>, coat_n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    var under: vec3<f32>;
    if (any(specular != vec4<f32>(1.0))) {
        var alpha = vec2<f32>(parameters.y * parameters.y);
        var axis = vec3<f32>(0.0);
        if (anisotropy > 0.0 && dot(t, t) > 0.0) {
            alpha = sc_aniso_alpha(parameters.y, anisotropy);
            axis = t;
        }
        under = sc_tinted_base_brdf(base, parameters, specular, alpha, n, axis, v, l);
    } else if (anisotropy > 0.0 && dot(t, t) > 0.0) {
        under = sc_aniso_base_brdf(base, parameters, sc_aniso_alpha(parameters.y, anisotropy), n, t, v, l);
    } else {
        under = sc_base_brdf(base, parameters, n, v, l);
    }
    return sc_coat_layer(under, coat, coat_color, coat_n, v, l);
}

// The direction an anisotropic surface reflects a prefiltered environment toward `v` from: the
// reflection about a normal bent toward the plane across the lobe's long axis (a public
// real-time approximation, mathematics only, as described in the Filament documentation's
// "anisotropic specular" image-based lighting). The lobe is stretched along `t` (OpenPBR's αt is
// the larger), so the reflection streaks along `t`. The bend grows with anisotropy and fades out
// on smooth surfaces (below perceptual roughness 0.2). Unit `n`, `t` and `v`; without a tangent
// or anisotropy it is the mirror reflection 2(n·v)n − v.
fn sc_aniso_reflect(n: vec3<f32>, t: vec3<f32>, v: vec3<f32>, roughness: f32, anisotropy: f32) -> vec3<f32> {
    let nv = max(dot(n, v), 0.0001);
    if (!(anisotropy > 0.0) || dot(t, t) == 0.0) { return 2.0 * nv * n - v; }
    let b = cross(n, t);
    let across = cross(cross(b, v), b);
    let bend = clamp(anisotropy, 0.0, 1.0) * clamp(5.0 * roughness, 0.0, 1.0);
    let bent = sc_unit(mix(n, across, bend));
    return 2.0 * dot(bent, v) * bent - v;
}

// The viewport's see-through fallback for transmission (no refraction; docs/material-graph.md
// §5.6): a share `weight` of the light behind passes, tinted by `color` (the transmittance at the
// input's depth). The result's `a` is the coverage for a premultiplied "over" blend, 1 − w·t̄
// with t̄ the mean of `color`; its `rgb` is the colour of the diffuse body that is drawn in place
// of the base: what of the base the transmission leaves, (1 − w)·`base`, plus the tint the scalar
// blend cannot apply to what is behind, w·max(`color` − t̄, 0). Weight 0 is (`base`, 1).
fn sc_see_through(base: vec3<f32>, weight: f32, color: vec3<f32>) -> vec4<f32> {
    let w = clamp(weight, 0.0, 1.0);
    if (w == 0.0) { return vec4<f32>(base, 1.0); }
    let c = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    let mean = (c.x + c.y + c.z) / 3.0;
    return vec4<f32>(base * (1.0 - w) + w * max(c - vec3<f32>(mean), vec3<f32>(0.0)), 1.0 - w * mean);
}

// A cut-out's coverage (`geometry_opacity`, not glass) on a shaded colour `c` that is already
// premultiplied by its own coverage (1 when opaque, `sc_see_through`'s for glass): colour and
// alpha both scaled by `opacity` in [0, 1], for the same premultiplied "over" blend. Opacity 1
// (or NaN) is `c` unchanged.
fn sc_cover(c: vec4<f32>, opacity: f32) -> vec4<f32> {
    if (!(opacity < 1.0)) { return c; }
    let o = max(opacity, 0.0);
    return vec4<f32>(c.rgb * o, c.a * o);
}

