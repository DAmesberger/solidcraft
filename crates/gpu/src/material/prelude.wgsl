// The raster back end's part of every generated shape module (crates/gpu/src/material/mod.rs):
// the material page, the context `material_eval` reads, the helpers generated code calls, and the
// fragment entry. Group 3 is "studio + material" (`ibl::material_layout`): the studio's cube,
// sampler and uniform (bindings 0–2), the uniform page (3), the material textures (4–7) and their
// samplers by wrap mode (8–10).

// One uniform page: 16 KiB, WebGL2's uniform-block limit. A material's record starts at the
// vec4 its vertices carry: the base response (`codegen::BASE_VEC4` vec4s), then its slots.
struct MatPage { m: array<vec4<f32>, 1024>, };
@group(3) @binding(3) var<uniform> mat_page: MatPage;

// What a material is evaluated at: the fragment, the record, the unit normal in world and object
// space, the position (mm) and pixel footprint (mm) in both spaces, and the object frame.
struct GraphCtx {
    frag: PbrFragment,
    record: u32,
    n: vec3<f32>,
    n_object: vec3<f32>,
    p_world: vec3<f32>,
    p_object: vec3<f32>,
    fp_world: f32,
    fp_object: f32,
    frame: sc_ObjectFrame,
};

// An `image` node's outputs.
struct MatImage { color: vec3<f32>, value: f32, alpha: f32, };

// An image that is not sampled (no texture yet, §5.6): its `default` colour with no coverage
// (alpha 0), so a mask read through its alpha shows no mark until its texture arrives; `value`
// is the chosen `channel` (0 luminance of rgb, 1–3 r, g, b, 4 alpha).
fn mat_image_default(color: vec3<f32>, channel: u32) -> MatImage {
    let picked = select(select(select(sc_luminance(color), color.r, channel == 1u), color.g, channel == 2u), color.b, channel == 3u);
    return MatImage(color, select(picked, 0.0, channel == 4u), 0.0);
}

// An unconnected texcoord: the origin, with no gradient.
fn mat_texcoord_none() -> sc_Texcoord {
    return sc_Texcoord(vec2<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec2<f32>(1.0), 0.0);
}

// The base response at record `r` (`material::base_record`), shaded at unit normal `n`.
fn mat_base_response(r: u32, n: vec3<f32>) -> Surface {
    let a = mat_page.m[r];
    let b = mat_page.m[r + 1u];
    let c = mat_page.m[r + 2u];
    let d = mat_page.m[r + 3u];
    let e = mat_page.m[r + 4u];
    let f = mat_page.m[r + 5u];
    let g = mat_page.m[r + 6u];
    return Surface(a.xyz, a.w, b.x, b.y, b.z, b.w, c.x, n, n, c.y, d.xyz, c.z, c.w, sc_default_tangent(n), e.xyz, d.w, f.xyz, e.w,
        g.xyz, f.w, g.w);
}

// `surface.mix` in v1 (decision O3): the two surfaces' parameters blended, `t` = 1 being `fg`;
// normals and the tangent by normalised lerp.
fn mat_surface_mix(bg: Surface, fg: Surface, t: f32) -> Surface {
    return Surface(
        mix(bg.base_color, fg.base_color, vec3<f32>(t)),
        mix(bg.metalness, fg.metalness, t),
        mix(bg.roughness, fg.roughness, t),
        mix(bg.ior, fg.ior, t),
        mix(bg.coat_weight, fg.coat_weight, t),
        mix(bg.coat_roughness, fg.coat_roughness, t),
        mix(bg.coat_ior, fg.coat_ior, t),
        sc_proj_unit(mix(bg.normal, fg.normal, vec3<f32>(t)), bg.normal),
        sc_proj_unit(mix(bg.coat_normal, fg.coat_normal, vec3<f32>(t)), bg.coat_normal),
        mix(bg.specular_weight, fg.specular_weight, t),
        mix(bg.specular_color, fg.specular_color, vec3<f32>(t)),
        mix(bg.anisotropy, fg.anisotropy, t),
        mix(bg.rotation, fg.rotation, t),
        sc_proj_unit(mix(bg.tangent, fg.tangent, vec3<f32>(t)), bg.tangent),
        mix(bg.coat_color, fg.coat_color, vec3<f32>(t)),
        mix(bg.emission_luminance, fg.emission_luminance, t),
        mix(bg.emission_color, fg.emission_color, vec3<f32>(t)),
        mix(bg.transmission_weight, fg.transmission_weight, t),
        mix(bg.transmission_color, fg.transmission_color, vec3<f32>(t)),
        mix(bg.transmission_depth_mm, fg.transmission_depth_mm, t),
        mix(bg.opacity, fg.opacity, t));
}

// A material-graph fragment. Every derivative is taken here, first, in uniform control flow;
// `material_eval` is straight-line.
@fragment
fn fs_mat(i: MatOut) -> @location(0) vec4<f32> {
    let f = pbr_fragment(i.pos, i.wp, i.n);
    let mm = 1.0 / max(u.units.x, 1e-30);
    let wx = dpdx(i.wp) * mm;
    let wy = dpdy(i.wp) * mm;
    let ox = dpdx(i.op) * mm;
    let oy = dpdy(i.op) * mm;
    let fo = fwidth(i.op) * mm;
    let fw = f.footprint * mm;
    let frame = sc_object_frame(wx, wy, ox, oy);
    let n = pbr_unit(i.n);
    let n_object = sc_proj_unit(transpose(frame.to_world) * n, n);
    let c = GraphCtx(f, i.record, n, n_object, i.wp * mm, i.op * mm, max(max(fw.x, fw.y), fw.z), max(max(fo.x, fo.y), fo.z), frame);
    return pbr_shade(material_eval(c), f);
}
