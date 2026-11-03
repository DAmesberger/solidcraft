// Shared shading library: projection (crates/gpu/src/shading/mod.rs). Pure functions, `sc_`
// names, WebGL2-safe. Bodies have no UVs, so every texture coordinate is projected from a point.

// Planar coordinates of `p`: its offset from `origin` along `u` and `v`, the axes' directions
// divided by their repeat lengths (as the runtime planar maps store them), so one repeat is one
// unit of uv.
fn sc_planar_uv(p: vec3<f32>, origin: vec3<f32>, u: vec3<f32>, v: vec3<f32>) -> vec2<f32> {
    let delta = p - origin;
    return vec2<f32>(dot(delta, u), dot(delta, v));
}

// An axis of one repeat of length |`axis`| as `sc_planar_uv` takes it: `axis` / |`axis`|². Zero
// for a zero axis.
fn sc_repeat_axis(axis: vec3<f32>) -> vec3<f32> {
    let length2 = dot(axis, axis);
    return select(vec3<f32>(0.0), axis / length2, length2 > 0.0);
}

// The blend weights of the three axis-aligned planar projections at unit normal `n`, by
// |n|^`sharpness` normalised to sum to one. The x weight is for the (y, z) plane, the y weight
// for (x, z) and the z weight for (x, y): `q.yz`, `q.xz` and `q.xy` of the point `q` in repeats.
fn sc_triplanar_weights(n: vec3<f32>, sharpness: f32) -> vec3<f32> {
    let w = pow(max(abs(n), vec3<f32>(1e-6)), vec3<f32>(sharpness));
    return w / (w.x + w.y + w.z);
}

// Cylindrical coordinates of `p` about the line through `origin` along unit `axis`: u around the
// axis from unit `reference` (perpendicular to it), as arc length at `radius` when `arc_length`
// and in radians otherwise, in (−π, π]·scale; v along the axis.
fn sc_cylindrical_uv(p: vec3<f32>, origin: vec3<f32>, axis: vec3<f32>, reference: vec3<f32>, radius: f32, arc_length: bool) -> vec2<f32> {
    let delta = p - origin;
    let side = cross(axis, reference);
    let angle = atan2(dot(delta, side), dot(delta, reference));
    return vec2<f32>(select(angle, angle * radius, arc_length), dot(delta, axis));
}

// The unit direction of increasing u of `sc_cylindrical_uv` at `p`; `reference`'s perpendicular
// on the axis itself.
fn sc_cylindrical_tangent(p: vec3<f32>, origin: vec3<f32>, axis: vec3<f32>, reference: vec3<f32>) -> vec3<f32> {
    let delta = p - origin;
    let radial = delta - axis * dot(delta, axis);
    if (dot(radial, radial) < 1e-20) { return cross(axis, reference); }
    return normalize(cross(axis, radial));
}
