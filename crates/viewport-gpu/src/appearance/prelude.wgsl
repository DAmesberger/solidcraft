// Binding 3 adds one uniform page to the studio's bind group.
struct MatPage { m: array<vec4<f32>, 1024>, };
@group(3) @binding(3) var<uniform> mat_page: MatPage;

struct AppearanceSurface { base: vec3<f32>, parameters: vec3<f32>, };

// A surface input as the surface node admits it: clamped to [lo, hi], NaN replaced by `fallback`.
fn sc_clean(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    let nan = (bitcast<u32>(x) & 0x7fffffffu) > 0x7f800000u;
    return select(clamp(x, lo, hi), fallback, nan);
}

fn sc_clean3(x: vec3<f32>, lo: f32, hi: f32, fallback: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(sc_clean(x.x, lo, hi, fallback.x), sc_clean(x.y, lo, hi, fallback.y), sc_clean(x.z, lo, hi, fallback.z));
}

// The same shading path as a conventional PBR face, including its output encoding.
@fragment
fn fs_mat(i: MatOut) -> @location(0) vec4<f32> {
    let toward = select(u.back.xyz, u.eye.xyz - i.wp, u.eye.w > 0.5);
    let facet = cross(dpdx(i.wp), dpdy(i.wp));
    let shading_n = select(i.n, select(facet, -facet, dot(facet, toward) < 0.0), dot(i.n, i.n) < 1e-12);
    let n = pbr_unit(shading_n);
    let v = pbr_unit(toward);
    if (clipped(i.wp)) { discard; }
    let surface = appearance_eval(i.record);
    var ao = 1.0;
    if (look_on()) { ao = occlusion(TOut(i.pos, n, vec4<f32>(surface.base, 1.0), i.wp, 0u, 0.0, 0u)); }
    return vec4<f32>(pbr_lit(surface.base, surface.parameters, n, v, i.wp, i.pos.xy, ao), 1.0);
}
