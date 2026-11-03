// The material-graph vertex streams (`material::VERTEX_SIZE`: position, normal and the vec4
// offset of the material's record in its uniform page; `material::OBJECT_VERTEX_SIZE`: plus the
// object-space position, for shapes that read object space). Part of the viewport module (its AO
// passes draw these streams) and of every generated shape module.
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

/// The AO prepass's depth of a material-graph surface (drawn only with the product look).
@fragment
fn fs_ao_depth_mat(i: MatOut, @builtin(front_facing) front: bool) -> AoDepth {
    return opaque_depth(TOut(i.pos, i.n, vec4<f32>(1.0), i.wp), front);
}

/// The AO term of a material-graph surface, on the prepass's depth.
@fragment
fn fs_ao_term_mat(i: MatOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return ao_term(TOut(i.pos, i.n, vec4<f32>(1.0), i.wp), front);
}
