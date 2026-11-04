// Material graphs in the path tracer (crates/gpu/src/path/material.rs, docs/material-graph.md
// §5.7): the job's material table, the context a generated evaluator reads, `image` sampling over
// the texture buffer, and how an evaluated surface resolves a hit's triangle. The evaluators and
// `graph_surface` itself are generated per job; a job without graphs gets `graph_stub.wgsl`.
//
// The table shares the texture buffer: `texture_words[1]` is its first word (0: no graphs). Its
// vec4 0 is the world position of the rebased geometry's origin; records follow, addressed in
// vec4s from the table's start. A record is a head (`GRAPH_HEAD` vec4s: the shape, then the
// world-to-object rows with their shift, then the object-to-world rows), the generator's record
// (its base-response head and the material's uniform slots) and one texture reference per image
// binding (word offset of its linear RGBA pixels, width, height).

const GRAPH_HEAD: u32 = 7u;

fn graph_words(i: u32) -> vec4<u32> {
    let w = texture_words[1] + 4u * i;
    return vec4<u32>(texture_words[w], texture_words[w + 1u], texture_words[w + 2u], texture_words[w + 3u]);
}

fn graph_vec4(i: u32) -> vec4<f32> {
    return bitcast<vec4<f32>>(graph_words(i));
}

// The record of a triangle's graph material plus one, zero for none. `interior.z` keeps its stage
// meaning (1: a render-stage body, which aging skips) and holds −(record + 1) for a graph.
fn graph_record(t: Triangle) -> u32 {
    if t.interior.z >= 0.0 { return 0u; }
    return u32(-t.interior.z);
}

// What the evaluator of `record` reads at rebased world point `p` with unit normal `n` and unit
// direction `v` toward the viewer. Paths integrate over the pixel, so the footprints are zero:
// patterns are evaluated unfiltered at the hit.
fn graph_context(record: u32, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> sc_GraphCtx {
    let a = graph_vec4(record + 1u);
    let b = graph_vec4(record + 2u);
    let c = graph_vec4(record + 3u);
    let to_object = transpose(mat3x3<f32>(a.xyz, b.xyz, c.xyz));
    let to_world = transpose(mat3x3<f32>(graph_vec4(record + 4u).xyz, graph_vec4(record + 5u).xyz, graph_vec4(record + 6u).xyz));
    let p_object = to_object * p + vec3<f32>(a.w, b.w, c.w);
    let n_object = sc_unit(transpose(to_world) * n);
    // The geometric tangent sticks to the object: its default tangent carried into world space.
    let carried = to_world * sc_default_tangent(n_object);
    let tangent = sc_unit(carried - n * dot(carried, n));
    return sc_GraphCtx(record + GRAPH_HEAD, v, n, tangent, abs(dot(n, v)), n_object, p + graph_vec4(0u).xyz, p_object, 0.0, sc_ObjectFrame(to_object, to_world));
}

// A texel index under a wrap mode (0 repeat, 1 clamp to edge, 2 mirrored repeat).
fn graph_wrap(i: i32, n: i32, wrap: u32) -> u32 {
    if wrap == 1u { return u32(clamp(i, 0, n - 1)); }
    if wrap == 2u {
        let m = ((i % (2 * n)) + 2 * n) % (2 * n);
        return u32(select(m, 2 * n - 1 - m, m >= n));
    }
    return u32(((i % n) + n) % n);
}

fn graph_texel(offset: u32, size: vec2<u32>, x: i32, y: i32, wrap: u32) -> vec4<f32> {
    let w = offset + 4u * (graph_wrap(y, i32(size.y), wrap) * size.x + graph_wrap(x, i32(size.x), wrap));
    return bitcast<vec4<f32>>(vec4<u32>(texture_words[w], texture_words[w + 1u], texture_words[w + 2u], texture_words[w + 3u]));
}

// One planar sample of the image whose reference is the record's vec4 `slot`: bilinear over the
// linear (already sRGB-decoded) pixels, with v = 1 at the top row, as `texture_sample` filters
// runtime maps. An unbound image gives `fallback` with no coverage.
fn graph_image(slot: u32, tc: sc_Texcoord, fallback: vec3<f32>, wrap: u32, channel: u32) -> sc_Image {
    let r = graph_words(slot);
    if r.x == 0u || r.y == 0u || r.z == 0u { return sc_image_default(fallback, channel); }
    let size = r.yz;
    // Bounded before the integer conversion; a texel index this far out wraps the same way.
    let x = clamp(tc.uv.x * f32(size.x) - 0.5, -1e9, 1e9);
    let y = clamp((1.0 - tc.uv.y) * f32(size.y) - 0.5, -1e9, 1e9);
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let i = i32(x0);
    let j = i32(y0);
    let top = mix(graph_texel(r.x, size, i, j, wrap), graph_texel(r.x, size, i + 1, j, wrap), fx);
    let bottom = mix(graph_texel(r.x, size, i, j + 1, wrap), graph_texel(r.x, size, i + 1, j + 1, wrap), fx);
    return sc_image_of(mix(top, bottom, fy), channel);
}

// The hit's triangle as an evaluated surface shades it: base colour, metalness, roughness, IOR,
// coat and the shading normal (in every vertex normal; `graph_normal` reads it). Glass keeps its
// transmission and carries neither metal nor a coat. The integrator, its sampling and its random
// numbers are unchanged.
fn graph_resolve(t: Triangle, s: sc_Surface) -> Triangle {
    var r = t;
    let glass = t.base.w > 0.0;
    r.base = vec4<f32>(s.base_color, t.base.w);
    r.parameters = vec4<f32>(select(s.metalness, 0.0, glass), s.roughness, s.ior, t.parameters.w);
    r.coat = vec4<f32>(select(s.coat_weight, 0.0, glass), s.coat_roughness, s.coat_ior, t.coat.w);
    let n = vec4<f32>(s.normal, 0.0);
    r.n0 = n;
    r.n1 = n;
    r.n2 = n;
    return r;
}

// The shading normal of a graph-resolved triangle, kept on the geometric side like a mapped
// normal; `n` unchanged for any other triangle.
fn graph_normal(t: Triangle, n: vec3<f32>, geometric: vec3<f32>) -> vec3<f32> {
    if graph_record(t) == 0u { return n; }
    // Already unit (the generator sanitises it): an unconnected normal is `n` itself, bit for bit.
    let g = t.n0.xyz;
    return select(g, n, dot(g, geometric) <= 0.0);
}
