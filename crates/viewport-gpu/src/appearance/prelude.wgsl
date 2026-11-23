// The raster back end's part of every generated shape module (crates/viewport-gpu/src/appearance/mod.rs):
// the material page, the base-response reader and the fragment entry. The context, the surface
// and the helpers generated code calls are the shared library's. Group 3 binds the studio's
// environment and rig (0–2, 11–12), the page (3), four images (4–7) and their samplers (8–10).

// One uniform page: 16 KiB, WebGL2's uniform-block limit. A material's record starts at the
// vec4 its vertices carry: the base response (`codegen::BASE_VEC4` vec4s), then its slots.
struct MatPage { m: array<vec4<f32>, 1024>, };
@group(3) @binding(3) var<uniform> mat_page: MatPage;

// The base response at record `r` (`appearance::base_record`), shaded at unit normal `n`.
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

// An appearance-graph fragment. Every derivative is taken here, first, in uniform control flow;
// `appearance_eval` is straight-line.
fn appearance_ao(i: MatOut, n: vec3<f32>, base: vec3<f32>) -> f32 {
    if (look_on()) { return occlusion(TOut(i.pos, n, vec4<f32>(base, 1.0), i.wp, 0u, 0.0, 0u)); }
    return 1.0;
}

@fragment
fn fs_mat(i: MatOut) -> @location(0) vec4<f32> {
    let wx = dpdx(i.wp);
    let wy = dpdy(i.wp);
    // The corners of a flat level of an instanced mesh carry no normal (`SharedTris::flat`): the
    // fragment takes its triangle's, facing the eye.
    let toward = select(u.back.xyz, u.eye.xyz - i.wp, u.eye.w > 0.5);
    let facet = cross(wx, wy);
    let shading_n = select(i.n, select(facet, -facet, dot(facet, toward) < 0.0), dot(i.n, i.n) < 1e-12);
    let v = pbr_unit(toward);
    let mm = 1.0 / max(u.units.x, 1e-30);
    let ox = dpdx(i.op) * mm;
    let oy = dpdy(i.op) * mm;
    let fw = (abs(wx) + abs(wy)) * mm;
    // Under a section the face it shades is cut away: so is its shading.
    if (clipped(i.wp)) { discard; }
    let frame = sc_object_frame(wx * mm, wy * mm, ox, oy);
    let n = pbr_unit(shading_n);
    let n_object = sc_unit(transpose(frame.to_world) * n);
    // The geometric tangent is the object's, carried into world space (the reference's placed
    // point): a rotated occurrence keeps its patterns' orientation.
    let carried = frame.to_world * sc_default_tangent(n_object);
    let tangent = sc_clean_tangent(carried, n, sc_default_tangent(n));
    let c = sc_GraphCtx(i.record, v, n, tangent, abs(dot(n, v)), n_object, i.wp * mm, i.op * mm, max(max(fw.x, fw.y), fw.z), frame);
    let s = appearance_eval(c);
    let ao = appearance_ao(i, s.normal, s.base_color);
    return vec4<f32>(pbr_lit(s.base_color, vec3<f32>(s.metalness, s.roughness, s.ior), s.normal, v, i.wp, i.pos.xy, ao), 1.0);
}
