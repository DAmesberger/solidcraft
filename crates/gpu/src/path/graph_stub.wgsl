// A job without material graphs (crates/gpu/src/path/material.rs): no triangle names a record,
// so every hit keeps its captured surface.
fn graph_surface(t: Triangle, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> Triangle {
    return t;
}
