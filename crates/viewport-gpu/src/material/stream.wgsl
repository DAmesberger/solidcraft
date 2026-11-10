// The material-graph vertex streams (`material::VERTEX_SIZE`: position, normal and the vec4
// offset of the material's record in its uniform page; `material::OBJECT_VERTEX_SIZE`: plus the
// object-space position, for shapes that read object space; `vs_mat_placed`: an instanced mesh's
// shared vertices at each placement, the record per placement). Part of every generated shape
// module.
struct MatOut {
    @invariant @builtin(position) pos: vec4<f32>,
    @location(0) n: vec3<f32>,
    @location(1) wp: vec3<f32>,
    @location(2) op: vec3<f32>,
    @location(3) @interpolate(flat) record: u32,
};

@vertex
fn vs_mat(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) record: u32) -> MatOut {
    return MatOut(u.vp * vec4<f32>(p, 1.0), n, p, p, record);
}

@vertex
fn vs_mat_object(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) record: u32, @location(3) op: vec3<f32>) -> MatOut {
    return MatOut(u.vp * vec4<f32>(p, 1.0), n, p, op, record);
}

@vertex
fn vs_mat_placed(
    @location(0) p: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(3) r0: vec4<f32>,
    @location(4) r1: vec4<f32>,
    @location(5) r2: vec4<f32>,
    @location(7) record: u32,
) -> MatOut {
    let q = vec4<f32>(p, 1.0);
    let wp = vec3<f32>(dot(r0, q), dot(r1, q), dot(r2, q));
    return MatOut(u.vp * vec4<f32>(wp, 1.0), vec3<f32>(dot(r0.xyz, n), dot(r1.xyz, n), dot(r2.xyz, n)), wp, p, record);
}
