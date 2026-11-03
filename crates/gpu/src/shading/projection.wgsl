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

// Material-graph texcoords (docs/material-graph.md §3.2), on the functions above. The formulas
// are the test-only reference's (`material_graph::reference`, track T), which checks both GPU
// targets against them.

// A projected coordinate: `uv` (in repeats of the projection), the world-space gradients of u and
// v (per mm) and the pixel footprint in world mm. The footprint in uv units is
// `sc_tc_footprint`; a pattern's slope (d/du, d/dv) is the spatial gradient slope.x·du + slope.y·dv.
struct sc_Texcoord {
    uv: vec2<f32>,
    du: vec3<f32>,
    dv: vec3<f32>,
    fp_mm: f32,
};

// The three planar coordinates of a triplanar projection (across X, Y and Z) and their weights.
struct sc_Triplanar {
    x: sc_Texcoord,
    y: sc_Texcoord,
    z: sc_Texcoord,
    w: vec3<f32>,
};

// The linear maps between world and object space at a point: `to_object` takes a world vector
// into object space, `to_world` an object vector into world space.
struct sc_ObjectFrame {
    to_object: mat3x3<f32>,
    to_world: mat3x3<f32>,
};

// The inverse of `m`, or the identity when `m` is (nearly) singular.
fn sc_inverse3(m: mat3x3<f32>) -> mat3x3<f32> {
    let a = m[0];
    let b = m[1];
    let c = m[2];
    let r0 = cross(b, c);
    let r1 = cross(c, a);
    let r2 = cross(a, b);
    let det = dot(a, r0);
    let scale = max(max(dot(a, a), dot(b, b)), dot(c, c));
    if (!(abs(det) > 1e-12 * scale * sqrt(scale))) {
        return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    }
    return transpose(mat3x3<f32>(r0, r1, r2)) * (1.0 / det);
}

// The object frame from the screen derivatives of the world position (`wx`, `wy`) and of the
// object position (`ox`, `oy`): each pair spans the triangle's plane, and its cross product,
// scaled to the pair, completes a basis. The identity where either pair is degenerate.
fn sc_object_frame(wx: vec3<f32>, wy: vec3<f32>, ox: vec3<f32>, oy: vec3<f32>) -> sc_ObjectFrame {
    let wn = cross(wx, wy);
    let on = cross(ox, oy);
    let identity = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    if (!(length(wn) > 1e-30) || !(length(on) > 1e-30)) {
        return sc_ObjectFrame(identity, identity);
    }
    let w = mat3x3<f32>(wx, wy, wn * inverseSqrt(length(wn)));
    let o = mat3x3<f32>(ox, oy, on * inverseSqrt(length(on)));
    return sc_ObjectFrame(o * sc_inverse3(w), w * sc_inverse3(o));
}

// A texcoord computed from object-space positions, with its gradients carried into world space
// (a gradient is a covector: it maps by the transpose of the world-to-object map).
fn sc_tc_to_world(t: sc_Texcoord, frame: sc_ObjectFrame) -> sc_Texcoord {
    let back = transpose(frame.to_object);
    return sc_Texcoord(t.uv, back * t.du, back * t.dv, t.fp_mm);
}

fn sc_tri_to_world(t: sc_Triplanar, frame: sc_ObjectFrame) -> sc_Triplanar {
    return sc_Triplanar(sc_tc_to_world(t.x, frame), sc_tc_to_world(t.y, frame), sc_tc_to_world(t.z, frame), t.w);
}

// The pixel footprint in uv units: fp_mm · max(|∇u|, |∇v|).
fn sc_tc_footprint(t: sc_Texcoord) -> f32 {
    return t.fp_mm * max(length(t.du), length(t.dv));
}

// `texcoord.planar`: uv = ((p − origin)·u/|u|², (p − origin)·v/|v|²).
fn sc_tc_planar(p: vec3<f32>, fp_mm: f32, origin: vec3<f32>, u: vec3<f32>, v: vec3<f32>) -> sc_Texcoord {
    let du = sc_repeat_axis(u);
    let dv = sc_repeat_axis(v);
    return sc_Texcoord(sc_planar_uv(p, origin, du, dv), du, dv, fp_mm);
}

// `texcoord.triplanar`: r = (p − origin) / scale_mm; the x plane reads (r.y, r.z), the y plane
// (r.x, r.z), the z plane (r.x, r.y), weighted by `sc_triplanar_weights` of `n`.
fn sc_tc_triplanar(p: vec3<f32>, n: vec3<f32>, fp_mm: f32, origin: vec3<f32>, scale_mm: vec3<f32>, sharpness: f32) -> sc_Triplanar {
    let r = (p - origin) / scale_mm;
    let gx = vec3<f32>(1.0 / scale_mm.x, 0.0, 0.0);
    let gy = vec3<f32>(0.0, 1.0 / scale_mm.y, 0.0);
    let gz = vec3<f32>(0.0, 0.0, 1.0 / scale_mm.z);
    return sc_Triplanar(sc_Texcoord(r.yz, gy, gz, fp_mm), sc_Texcoord(r.xz, gx, gz, fp_mm), sc_Texcoord(r.xy, gx, gy, fp_mm),
        sc_triplanar_weights(n, sharpness));
}

// `texcoord.cylindrical`: `sc_cylindrical_uv`, with the gradient of its angle.
fn sc_tc_cylindrical(p: vec3<f32>, fp_mm: f32, origin: vec3<f32>, axis: vec3<f32>, reference: vec3<f32>, radius_mm: f32, arc_length: bool) -> sc_Texcoord {
    let d = p - origin;
    let side = cross(axis, reference);
    let x = dot(d, reference);
    let y = dot(d, side);
    let angle_gradient = (side * x - reference * y) / max(x * x + y * y, 1e-30);
    let du = angle_gradient * select(1.0, radius_mm, arc_length);
    return sc_Texcoord(sc_cylindrical_uv(p, origin, axis, reference, radius_mm, arc_length), du, axis, fp_mm);
}

// `texcoord.transform`: uv' = R(rotation)·(uv ⊙ scale) + offset, with its gradients.
fn sc_tc_transform(t: sc_Texcoord, offset: vec2<f32>, scale: vec2<f32>, rotation: f32) -> sc_Texcoord {
    let c = cos(rotation);
    let s = sin(rotation);
    let u = t.uv.x * scale.x;
    let v = t.uv.y * scale.y;
    let du = t.du * scale.x;
    let dv = t.dv * scale.y;
    return sc_Texcoord(vec2<f32>(u * c - v * s + offset.x, u * s + v * c + offset.y), du * c - dv * s, du * s + dv * c, t.fp_mm);
}

fn sc_tri_transform(t: sc_Triplanar, offset: vec2<f32>, scale: vec2<f32>, rotation: f32) -> sc_Triplanar {
    return sc_Triplanar(sc_tc_transform(t.x, offset, scale, rotation), sc_tc_transform(t.y, offset, scale, rotation), sc_tc_transform(t.z, offset, scale, rotation), t.w);
}

// A unit tangent perpendicular to unit `n`, from the world axes: the geometric tangent (an
// unconnected `geometry_tangent`, and a projection's where u does not change along the surface).
fn sc_default_tangent(n: vec3<f32>) -> vec3<f32> {
    let axis = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
    return sc_unit(axis - n * dot(axis, n));
}

// The tangent a projection defines at unit normal `n`: ∇u made tangential and normalised, or
// `fallback` where that vanishes.
fn sc_tc_tangent(t: sc_Texcoord, n: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let g = t.du - n * dot(t.du, n);
    return select(sc_unit(g), fallback, length(g) < 1e-12);
}

fn sc_tri_tangent(t: sc_Triplanar, n: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    return sc_unit(t.w.x * sc_tc_tangent(t.x, n, fallback) + t.w.y * sc_tc_tangent(t.y, n, fallback) + t.w.z * sc_tc_tangent(t.z, n, fallback));
}

fn sc_tri_uv(t: sc_Triplanar) -> vec2<f32> {
    return t.w.x * t.x.uv + t.w.y * t.y.uv + t.w.z * t.z.uv;
}
