// The raster back end's part of every generated shape module (crates/gpu/src/material/mod.rs):
// the material page, the base-response reader and the fragment entry. The context, the surface
// and the helpers generated code calls are the shared library's (`sc_GraphCtx`, `sc_Surface`). Group 3 is "studio + material" (`ibl::material_layout`): the studio's cube,
// sampler and uniform (bindings 0–2), the uniform page (3), the material textures (4–7) and their
// samplers by wrap mode (8–10).

// One uniform page: 16 KiB, WebGL2's uniform-block limit. A material's record starts at the
// vec4 its vertices carry: the base response (`codegen::BASE_VEC4` vec4s), then its slots.
struct MatPage { m: array<vec4<f32>, 1024>, };
@group(3) @binding(3) var<uniform> mat_page: MatPage;

// The base response at record `r` (`material::base_record`), shaded at unit normal `n`.
fn mat_base_response(r: u32, n: vec3<f32>) -> sc_Surface {
    let a = mat_page.m[r];
    let b = mat_page.m[r + 1u];
    let c = mat_page.m[r + 2u];
    let d = mat_page.m[r + 3u];
    let e = mat_page.m[r + 4u];
    let f = mat_page.m[r + 5u];
    let g = mat_page.m[r + 6u];
    return sc_Surface(a.xyz, a.w, b.x, b.y, b.z, b.w, c.x, n, n, c.y, d.xyz, c.z, c.w, sc_default_tangent(n), e.xyz, d.w, f.xyz, e.w,
        g.xyz, f.w, g.w);
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
    let fw = f.footprint * mm;
    let frame = sc_object_frame(wx, wy, ox, oy);
    let n = sc_unit(i.n);
    let n_object = sc_unit(transpose(frame.to_world) * n);
    let c = sc_GraphCtx(i.record, f.v, n, sc_default_tangent(n), abs(dot(n, f.v)), n_object, i.wp * mm, i.op * mm, max(max(fw.x, fw.y), fw.z), frame);
    return pbr_shade(material_eval(c), f);
}
