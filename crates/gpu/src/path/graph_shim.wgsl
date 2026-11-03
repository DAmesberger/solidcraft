// TEMPORARY (track PT): stand-ins for the shared library's material-graph types and helpers,
// under the names agreed with track C1, until C1 moves them into crates/gpu/src/shading/. This
// file is deleted when that lands; nothing else may depend on it.

struct sc_Texcoord {
    uv: vec2<f32>,
    du: vec3<f32>,
    dv: vec3<f32>,
    repeat: vec2<f32>,
    footprint: f32,
};

struct sc_ObjectFrame {
    to_object: mat3x3<f32>,
    to_world: mat3x3<f32>,
};

struct sc_GraphCtx {
    record: u32,
    v: vec3<f32>,
    n: vec3<f32>,
    n_object: vec3<f32>,
    p_world: vec3<f32>,
    p_object: vec3<f32>,
    fp_world: f32,
    fp_object: f32,
    frame: sc_ObjectFrame,
};

struct sc_Surface {
    base_color: vec3<f32>,
    metalness: f32,
    roughness: f32,
    ior: f32,
    coat_weight: f32,
    coat_roughness: f32,
    coat_ior: f32,
    normal: vec3<f32>,
    coat_normal: vec3<f32>,
    specular_weight: f32,
    specular_color: vec3<f32>,
    anisotropy: f32,
    rotation: f32,
    tangent: vec3<f32>,
    coat_color: vec3<f32>,
    emission_luminance: f32,
    emission_color: vec3<f32>,
    transmission_weight: f32,
    transmission_color: vec3<f32>,
    transmission_depth_mm: f32,
    opacity: f32,
};

struct sc_Image { color: vec3<f32>, value: f32, alpha: f32, };

fn sc_proj_unit(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    return select(fallback, v / l, l > 1e-20);
}

fn sc_luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn sc_image_pick(rgba: vec4<f32>, channel: u32) -> f32 {
    return select(select(select(select(sc_luminance(rgba.rgb), rgba.r, channel == 1u), rgba.g, channel == 2u), rgba.b, channel == 3u), rgba.a, channel == 4u);
}

fn sc_image_of(rgba: vec4<f32>, channel: u32) -> sc_Image {
    return sc_Image(rgba.rgb, sc_image_pick(rgba, channel), rgba.a);
}

fn sc_image_default(color: vec3<f32>, channel: u32) -> sc_Image {
    return sc_image_of(vec4<f32>(color, 0.0), channel);
}

fn sc_default_tangent(n: vec3<f32>) -> vec3<f32> {
    let a = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
    return sc_proj_unit(a - n * dot(a, n), vec3<f32>(1.0, 0.0, 0.0));
}

fn sc_clean(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    let nan = (bitcast<u32>(x) & 0x7fffffffu) > 0x7f800000u;
    return select(clamp(x, lo, hi), fallback, nan);
}

fn sc_clean3(x: vec3<f32>, lo: f32, hi: f32, fallback: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(sc_clean(x.x, lo, hi, fallback.x), sc_clean(x.y, lo, hi, fallback.y), sc_clean(x.z, lo, hi, fallback.z));
}
