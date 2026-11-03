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

// Material-graph texcoords (docs/material-graph.md §3.2): the raster and path back ends' projections.

// A projected coordinate. `uv` is in pattern units (millimetres for an unscaled projection);
// `du` and `dv` are its gradients over space (per mm), so a pattern's slope (d/du, d/dv) turns
// into a spatial gradient as slope.x·du + slope.y·dv. `repeat` is the span of `uv` that one image
// covers, and `footprint` the pixel's size in uv units, which patterns fade their detail by.
struct sc_Texcoord {
    uv: vec2<f32>,
    du: vec3<f32>,
    dv: vec3<f32>,
    repeat: vec2<f32>,
    footprint: f32,
};

// The three planar coordinates of a triplanar projection (across X, Y and Z) and their blend
// weights, which sum to one.
struct sc_Triplanar {
    x: sc_Texcoord,
    y: sc_Texcoord,
    z: sc_Texcoord,
    w: vec3<f32>,
};

// Maps between world and object space at a fragment, rebuilt from the screen derivatives of
// both positions (no per-vertex frame is needed): `to_object` takes a world vector into object
// space and `to_world` an object vector into world space.
struct sc_ObjectFrame {
    to_object: mat3x3<f32>,
    to_world: mat3x3<f32>,
};

fn sc_proj_unit(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    return select(fallback, v / l, l > 1e-20);
}

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

// The object frame from the derivatives of the world position (`wx`, `wy`) and of the object
// position (`ox`, `oy`) along the screen's x and y. Each pair spans the triangle's plane; its
// cross product completes a basis, scaled to the pair so a uniform scale stays uniform.
fn sc_object_frame(wx: vec3<f32>, wy: vec3<f32>, ox: vec3<f32>, oy: vec3<f32>) -> sc_ObjectFrame {
    let wn = cross(wx, wy);
    let on = cross(ox, oy);
    let w = mat3x3<f32>(wx, wy, wn * inverseSqrt(max(length(wn), 1e-30)));
    let o = mat3x3<f32>(ox, oy, on * inverseSqrt(max(length(on), 1e-30)));
    let identity = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    if (!(length(wn) > 1e-30) || !(length(on) > 1e-30)) {
        return sc_ObjectFrame(identity, identity);
    }
    return sc_ObjectFrame(o * sc_inverse3(w), w * sc_inverse3(o));
}

// A texcoord computed in object space, with its gradients carried into world space (a gradient
// is a covector: it maps by the transpose of the world-to-object map).
fn sc_tc_to_world(t: sc_Texcoord, frame: sc_ObjectFrame) -> sc_Texcoord {
    let back = transpose(frame.to_object);
    return sc_Texcoord(t.uv, back * t.du, back * t.dv, t.repeat, t.footprint);
}

fn sc_tri_to_world(t: sc_Triplanar, frame: sc_ObjectFrame) -> sc_Triplanar {
    return sc_Triplanar(sc_tc_to_world(t.x, frame), sc_tc_to_world(t.y, frame), sc_tc_to_world(t.z, frame), t.w);
}

// The pixel footprint `fp` (mm) in uv units of a projection with gradients `du`, `dv`.
fn sc_tc_footprint(fp: f32, du: vec3<f32>, dv: vec3<f32>) -> f32 {
    return fp * max(length(du), length(dv));
}

// Planar: `u` and `v` span one repeat from `origin`; uv is in millimetres along their directions.
fn sc_tc_planar(p: vec3<f32>, fp: f32, origin: vec3<f32>, u: vec3<f32>, v: vec3<f32>) -> sc_Texcoord {
    let lu = max(length(u), 1e-6);
    let lv = max(length(v), 1e-6);
    let du = u / lu;
    let dv = v / lv;
    let d = p - origin;
    return sc_Texcoord(vec2<f32>(dot(d, du), dot(d, dv)), du, dv, vec2<f32>(lu, lv), sc_tc_footprint(fp, du, dv));
}

// Triplanar: one planar coordinate per axis, (p − origin) / scale_mm on the other two axes, so
// one repeat is `scale_mm`; blended by |n|^sharpness.
fn sc_tc_triplanar(p: vec3<f32>, n: vec3<f32>, fp: f32, origin: vec3<f32>, scale_mm: vec3<f32>, sharpness: f32) -> sc_Triplanar {
    let s = max(abs(scale_mm), vec3<f32>(1e-6));
    let q = (p - origin) / s;
    let ex = vec3<f32>(1.0, 0.0, 0.0);
    let ey = vec3<f32>(0.0, 1.0, 0.0);
    let ez = vec3<f32>(0.0, 0.0, 1.0);
    let one = vec2<f32>(1.0);
    let tx = sc_Texcoord(q.yz, ey / s.y, ez / s.z, one, sc_tc_footprint(fp, ey / s.y, ez / s.z));
    let ty = sc_Texcoord(q.xz, ex / s.x, ez / s.z, one, sc_tc_footprint(fp, ex / s.x, ez / s.z));
    let tz = sc_Texcoord(q.xy, ex / s.x, ey / s.y, one, sc_tc_footprint(fp, ex / s.x, ey / s.y));
    let a = pow(abs(n) + vec3<f32>(1e-5), vec3<f32>(clamp(sharpness, 1.0, 64.0)));
    let w = a / max(a.x + a.y + a.z, 1e-20);
    return sc_Triplanar(tx, ty, tz, w);
}

// Cylindrical: u around `axis` from `reference` (mm of arc at `radius_mm`, or radians when
// `angle` is true) and v along the axis (mm). One repeat is one turn around and one turn's arc
// length along.
fn sc_tc_cylindrical(p: vec3<f32>, fp: f32, origin: vec3<f32>, axis: vec3<f32>, reference: vec3<f32>, radius_mm: f32, angle: bool) -> sc_Texcoord {
    let a = sc_proj_unit(axis, vec3<f32>(0.0, 0.0, 1.0));
    let r0 = sc_proj_unit(reference - a * dot(reference, a), sc_proj_unit(cross(a, vec3<f32>(0.0, 1.0, 0.0)), vec3<f32>(1.0, 0.0, 0.0)));
    let r1 = cross(a, r0);
    let d = p - origin;
    let radial = d - a * dot(d, a);
    let x = dot(radial, r0);
    let y = dot(radial, r1);
    let theta = atan2(y, x);
    let dist = max(length(radial), 1e-6);
    let around = (r1 * x - r0 * y) / (dist * dist);
    let radius = max(radius_mm, 1e-6);
    let turn = 6.283185307179586;
    let scale = select(radius, 1.0, angle);
    let du = around * scale;
    return sc_Texcoord(vec2<f32>(theta * scale, dot(d, a)), du, a, vec2<f32>(turn * scale, turn * radius), sc_tc_footprint(fp, du, a));
}

// uv' = rotate(uv, rotation) · scale + offset, with its gradients and footprint.
fn sc_tc_transform(t: sc_Texcoord, offset: vec2<f32>, scale: vec2<f32>, rotation: f32) -> sc_Texcoord {
    let c = cos(rotation);
    let s = sin(rotation);
    let uv = vec2<f32>(c * t.uv.x - s * t.uv.y, s * t.uv.x + c * t.uv.y) * scale + offset;
    let du = (t.du * c - t.dv * s) * scale.x;
    let dv = (t.du * s + t.dv * c) * scale.y;
    let k = max(abs(scale.x), abs(scale.y));
    return sc_Texcoord(uv, du, dv, t.repeat * abs(scale), t.footprint * k);
}

fn sc_tri_transform(t: sc_Triplanar, offset: vec2<f32>, scale: vec2<f32>, rotation: f32) -> sc_Triplanar {
    return sc_Triplanar(sc_tc_transform(t.x, offset, scale, rotation), sc_tc_transform(t.y, offset, scale, rotation), sc_tc_transform(t.z, offset, scale, rotation), t.w);
}

// The direction of increasing u (the tangent a projection defines), unit.
fn sc_tc_tangent(t: sc_Texcoord) -> vec3<f32> {
    return sc_proj_unit(t.du, vec3<f32>(1.0, 0.0, 0.0));
}

fn sc_tri_tangent(t: sc_Triplanar) -> vec3<f32> {
    return sc_proj_unit(t.w.x * sc_tc_tangent(t.x) + t.w.y * sc_tc_tangent(t.y) + t.w.z * sc_tc_tangent(t.z), vec3<f32>(1.0, 0.0, 0.0));
}

fn sc_tri_uv(t: sc_Triplanar) -> vec2<f32> {
    return t.w.x * t.x.uv + t.w.y * t.y.uv + t.w.z * t.z.uv;
}

// A unit tangent perpendicular to unit `n`, from the world axes (for an unconnected tangent).
fn sc_default_tangent(n: vec3<f32>) -> vec3<f32> {
    let axis = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
    return sc_proj_unit(axis - n * dot(axis, n), vec3<f32>(1.0, 0.0, 0.0));
}
