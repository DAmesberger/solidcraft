//! The path back end of the material graph: its table, its hook and its pipeline, and that a
//! constant graph renders exactly as the equivalent `pbr` look.

use std::collections::BTreeMap;
use std::sync::Arc;

use solidcraft_geom::{Mesh, Vec3};
use solidcraft_render::job::graphs::{GraphBinding, GraphMaterial, GraphMaterials};
use solidcraft_render::job::{CancelToken, ImageRequest, ImageWork, JobOptions, TaskPoll};
use solidcraft_render::material_graph::{Constant, MaterialGraph, base_response, ir::Ir, lower, validate};
use solidcraft_render::surface::OpaqueSurface;
use solidcraft_render::{Camera, ProductMesh, ProductScene, StandardView};

use super::super::{Executor, PATH_SOURCE, Task, admission, open_headless};

const COPPER: &str = include_str!("../../../render/tests/fixtures/material_graph/pbr-copper.json");
/// A constant coated dielectric (an original fixture for this test).
const COATED: &str = r#"{"name":"Coated","nodes":[{"id":"surface","kind":"standard_surface","inputs":{
    "base_color":[0.05,0.2,0.1],"specular_roughness":0.6,"coat_weight":1,"coat_roughness":0.08,"coat_ior":1.5}}],"output":"surface"}"#;

fn executor() -> Option<Executor> {
    match open_headless() {
        Ok((_, device, queue)) => Executor::new(device, queue).ok(),
        Err(e) => {
            assert!(
                std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none() || crate::select::path_backends().is_empty(),
                "required GPU unavailable: {e}"
            );
            eprintln!("gpu-path graph test skipped: {e}");
            None
        }
    }
}

fn graph(text: &str) -> (Arc<Ir>, OpaqueSurface) {
    let graph: MaterialGraph = serde_json::from_str(text).unwrap();
    let valid = validate(&graph).unwrap();
    (Arc::new(lower(&valid).unwrap()), base_response(&valid).unwrap())
}

fn material(ir: &Arc<Ir>) -> GraphMaterial {
    GraphMaterial::new(ir.clone(), &BTreeMap::new(), vec![None; ir.textures.len()]).unwrap()
}

fn binding(material: usize) -> GraphBinding {
    GraphBinding { body_index: 0, body_name: "graph sphere".into(), face: None, material, object_to_world: None }
}

fn request(size: u32) -> ImageRequest {
    let mut camera = Camera { distance: 4.0, ..Default::default() };
    camera.set_view(StandardView::Top);
    ImageRequest {
        studio_weights: None,
        integrator: Default::default(),
        raytrace: None,
        textures: None,
        environment: None,
        graphs: None,
        camera,
        width: size,
        height: size,
        background: [10, 20, 30],
        transparent_background: false,
    }
}

/// An original unit sphere (the path tests' construction, smaller).
fn sphere(surface: OpaqueSurface) -> ProductScene {
    let (rows, columns) = (24u32, 48u32);
    let mut mesh = Mesh::default();
    for i in 0..=rows {
        let theta = f64::from(i) * std::f64::consts::PI / f64::from(rows);
        for j in 0..=columns {
            let phi = f64::from(j) * std::f64::consts::TAU / f64::from(columns);
            let n = Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
            mesh.positions.push(n);
            mesh.normals.push(n);
        }
    }
    for i in 0..rows {
        for j in 0..columns {
            let a = i * (columns + 1) + j;
            let b = a + columns + 1;
            mesh.triangles.extend([[a, b, a + 1], [a + 1, b, b + 1]]);
            mesh.tri_face.extend([0, 0]);
        }
    }
    let bounds = mesh.bounds();
    ProductScene {
        meshes: vec![ProductMesh {
            name: "graph sphere".into(),
            mesh: Arc::new(mesh),
            color: [255; 3],
            opacity: 1.0,
            surface: Some(surface),
            transmission: None,
            faces: Vec::new(),
        }],
        bounds,
    }
}

fn white() -> OpaqueSurface {
    OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), 0.0, 1.0, 1.0).unwrap()
}

fn render(executor: &Executor, scene: ProductScene, request: ImageRequest) -> Vec<[f32; 4]> {
    let options = JobOptions { samples: 8, max_depth: 4, seed: 11, ..Default::default() };
    let mut task = Task::new(executor.context.clone(), scene, request, options, CancelToken::default()).unwrap();
    for _ in 0..options.samples {
        task.complete_sample().unwrap();
    }
    task.read_linear().unwrap()
}

/// A constant graph is its `pbr` look: the same triangle reaches the integrator, so the frame is
/// bit-identical, while the scene's own surface (here white) is replaced.
#[test]
fn a_constant_graph_renders_exactly_as_its_pbr_look() {
    let Some(executor) = executor() else { return };
    for fixture in [COPPER, COATED] {
        let (ir, base) = graph(fixture);
        let pbr = render(&executor, sphere(base), request(16));
        let mut with_graph = request(16);
        with_graph.graphs = Some(Arc::new(GraphMaterials::new(vec![material(&ir)], vec![binding(0)]).unwrap()));
        assert_eq!(render(&executor, sphere(white()), with_graph), pbr, "a constant graph equals its pbr look");
        assert_ne!(render(&executor, sphere(white()), request(16)), pbr, "the graph, not the scene's surface, shaded the sphere");
    }
}

/// Jobs with the same shape set share one compiled pipeline; jobs without graphs use the device's.
#[test]
fn graph_pipelines_are_shared_by_shape_set() {
    let Some(executor) = executor() else { return };
    let (ir, _) = graph(COPPER);
    let graphs = |values: BTreeMap<String, Constant>| {
        let m = GraphMaterial::new(ir.clone(), &values, Vec::new()).unwrap();
        Some(Arc::new(GraphMaterials::new(vec![m], vec![binding(0)]).unwrap()))
    };
    let task = |graphs| {
        let mut r = request(4);
        r.graphs = graphs;
        Task::new(executor.context.clone(), sphere(white()), r, JobOptions::default(), CancelToken::default()).unwrap()
    };
    let a = task(graphs(BTreeMap::new()));
    let b = task(graphs(BTreeMap::new()));
    assert!(Arc::ptr_eq(a.pipeline.as_ref().unwrap(), b.pipeline.as_ref().unwrap()), "one pipeline per shape set");
    assert!(task(None).pipeline.is_none());
    assert_eq!(executor.context.graph_pipelines.lock().unwrap().len(), 1);
}

/// A triangle names its record as −(record + 1) in `interior.z`, and the record starts with its
/// shape; the stage flag keeps its meaning.
#[test]
fn triangles_name_their_records_in_the_texture_buffer() {
    let (copper, _) = graph(COPPER);
    let (coated, _) = graph(COATED);
    let mut scene = sphere(white());
    let mut second = scene.meshes[0].clone();
    second.name = "second".into();
    scene.meshes.push(second);
    let graphs = GraphMaterials::new(
        vec![material(&copper), material(&coated)],
        vec![binding(0), GraphBinding { body_index: 1, body_name: "second".into(), ..binding(1) }],
    )
    .unwrap();
    let cancel = CancelToken::default();
    let (texture, records) = super::super::textures::flatten_with_graphs(None, Some(&graphs), 2, Vec3::ZERO, &cancel).unwrap();
    let word = |i: usize| u32::from_le_bytes(texture[4 * i..4 * i + 4].try_into().unwrap());
    let table = word(1) as usize;
    assert!(table > 0 && table.is_multiple_of(4));
    for body in 0..2 {
        let r = -records.interior(body, 0);
        assert!(r >= 1.0 && r.fract() == 0.0);
        let record = table + 4 * (r as usize - 1);
        let shape = graphs.shapes().binary_search(&graphs.materials()[body].ir().shape).unwrap();
        assert_eq!(word(record) as usize, shape);
        // The world-to-object rows of an unplaced body are the identity.
        assert_eq!([word(record + 4), word(record + 9), word(record + 14)].map(f32::from_bits), [1.0; 3]);
    }
    let upload = super::super::scene::flatten_with_graphs(&scene, Vec3::ZERO, &cancel, false, &records).unwrap();
    let geometry = |i: usize| u32::from_le_bytes(upload.geometry[4 * i..4 * i + 4].try_into().unwrap());
    let (count, materials) = (geometry(0) as usize, geometry(2) as usize);
    let mut seen = std::collections::BTreeSet::new();
    for t in 0..count {
        let z = f32::from_bits(geometry(materials + t * 20 + 18));
        assert!(z < 0.0, "every triangle of a graph-bound body names its record");
        seen.insert(z.to_bits());
    }
    assert_eq!(seen.len(), 2, "two bodies with two materials have two records");
}

/// Admission refuses what the path tracer cannot shade and checks bindings against the scene.
#[test]
fn admission_checks_graph_bindings() {
    let (copper, base) = graph(COPPER);
    let (coated, _) = graph(COATED);
    let with = |m: &Arc<Ir>, b: GraphBinding| {
        let mut r = request(4);
        r.graphs = Some(Arc::new(GraphMaterials::new(vec![material(m)], vec![b]).unwrap()));
        r
    };
    assert!(admission(&sphere(base), &with(&copper, binding(0)), JobOptions::default()).is_ok());
    let renamed = GraphBinding { body_name: "another body".into(), ..binding(0) };
    assert!(admission(&sphere(base), &with(&copper, renamed), JobOptions::default()).unwrap_err().message.contains("name"));
    let missing_face = GraphBinding { face: Some(7), ..binding(0) };
    assert!(admission(&sphere(base), &with(&copper, missing_face), JobOptions::default()).is_err());
    let mut glass = sphere(OpaqueSurface::new(Vec3::ZERO, 0.0, 0.1, 1.5).unwrap());
    glass.meshes[0].transmission = Some(solidcraft_render::transmission::Transmission::new(Vec3::ZERO, None).unwrap());
    let refused = admission(&glass, &with(&coated, binding(0)), JobOptions::default()).unwrap_err();
    assert!(refused.message.contains("transmissive"), "{refused:?}");
}

/// The generated module joins the path shader: it parses and validates with every shape of the job.
#[test]
fn the_generated_module_validates() {
    let (copper, _) = graph(COPPER);
    let (coated, _) = graph(COATED);
    let graphs = GraphMaterials::new(vec![material(&copper), material(&coated)], vec![binding(0)]).unwrap();
    let source = format!("{PATH_SOURCE}\n{}", super::source(&graphs).unwrap());
    let module = match naga::front::wgsl::parse_str(&source) {
        Ok(m) => m,
        Err(e) => panic!("{}", e.emit_to_string(&source)),
    };
    let validated = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module);
    assert!(validated.is_ok(), "{validated:?}");
    // Constant graphs share one shape whatever their values: one evaluator.
    assert_eq!(graphs.shapes().len(), 1);
    assert_eq!(source.matches("fn graph_eval_").count(), graphs.shapes().len());
}

/// A complete graph job encodes, like any other.
#[test]
fn a_graph_job_completes() {
    let Some(executor) = executor() else { return };
    let (ir, _) = graph(COATED);
    let mut r = request(8);
    r.graphs = Some(Arc::new(GraphMaterials::new(vec![material(&ir)], vec![binding(0)]).unwrap()));
    let mut task =
        Task::new(executor.context.clone(), sphere(white()), r, JobOptions { samples: 2, ..Default::default() }, CancelToken::default()).unwrap();
    for _ in 0..4096 {
        if let TaskPoll::Complete(_) = task.step(4096).unwrap() {
            return;
        }
    }
    panic!("bounded graph job did not complete");
}

/// One shading point of a probe: rebased world position, unit normal, unit direction to the viewer.
type ProbePoint = ([f32; 3], [f32; 3], [f32; 3]);

/// The 37 floats (WGSL `Surface` order) the job's evaluator of material 0 returns at each point,
/// read back from the GPU: what both back ends compare against the test-only reference.
fn probe(executor: &Executor, graphs: GraphMaterials, points: &[ProbePoint]) -> Vec<[f32; 37]> {
    assert!(points.len() <= 51);
    let shape = graphs.shapes().binary_search(&graphs.materials()[0].ir().shape).unwrap();
    let v3 = |v: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", v[0], v[1], v[2]);
    let list = |k: usize| points.iter().map(|p| v3([p.0, p.1, p.2][k])).collect::<Vec<_>>().join(", ");
    let n = points.len();
    let fields = [
        "s.base_color.x",
        "s.base_color.y",
        "s.base_color.z",
        "s.metalness",
        "s.roughness",
        "s.ior",
        "s.coat_weight",
        "s.coat_roughness",
        "s.coat_ior",
        "s.normal.x",
        "s.normal.y",
        "s.normal.z",
        "s.coat_normal.x",
        "s.coat_normal.y",
        "s.coat_normal.z",
        "s.specular_weight",
        "s.specular_color.x",
        "s.specular_color.y",
        "s.specular_color.z",
        "s.anisotropy",
        "s.rotation",
        "s.tangent.x",
        "s.tangent.y",
        "s.tangent.z",
        "s.coat_color.x",
        "s.coat_color.y",
        "s.coat_color.z",
        "s.emission_luminance",
        "s.emission_color.x",
        "s.emission_color.y",
        "s.emission_color.z",
        "s.transmission_weight",
        "s.transmission_color.x",
        "s.transmission_color.y",
        "s.transmission_color.z",
        "s.transmission_depth_mm",
        "s.opacity",
        "0.0",
        "0.0",
        "0.0",
    ];
    let entry = format!(
        "@compute @workgroup_size(64)
fn graph_probe(@builtin(global_invocation_id) gid: vec3<u32>) {{
    if gid.x >= {n}u {{ return; }}
    var ps = array<vec3<f32>, {n}>({});
    var ns = array<vec3<f32>, {n}>({});
    var vs = array<vec3<f32>, {n}>({});
    let r = graph_record(triangle(0u)) - 1u;
    let s = graph_eval_{shape}(graph_context(r, ps[gid.x], ns[gid.x], vs[gid.x]));
    var f = array<f32, 40>({});
    // Each float as an exact non-negative pair, which the checked readback accepts.
    for (var k = 0u; k < 20u; k += 1u) {{
        let a = f[2u * k];
        let b = f[2u * k + 1u];
        accumulation[gid.x * 20u + k] = vec4<f32>(max(a, 0.0), max(-a, 0.0), max(b, 0.0), max(-b, 0.0));
    }}
}}",
        list(0),
        list(1),
        list(2),
        fields.join(", ")
    );
    let shader = format!("{PATH_SOURCE}\n{}\n{entry}", super::source(&graphs).unwrap());
    let context = &executor.context;
    let module = context.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(shader.into()) });
    let pipeline = context.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&context.pipeline_layout),
        module: &module,
        entry_point: Some("graph_probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut r = request(32);
    r.graphs = Some(Arc::new(graphs));
    let mut task = Task::new(context.clone(), sphere(white()), r, JobOptions::default(), CancelToken::default()).unwrap();
    let mut encoder = context.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &task.bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    context.queue.submit([encoder.finish()]);
    task.sample = 1;
    let raw = task.read_linear().unwrap();
    (0..n)
        .map(|i| {
            let mut out = [0f32; 37];
            for (k, x) in out.iter_mut().enumerate() {
                let pair = raw[i * 20 + k / 2];
                *x = if k % 2 == 0 { pair[0] - pair[1] } else { pair[2] - pair[3] };
            }
            out
        })
        .collect()
}

/// At fixed points the path evaluator returns the graph's constants, sanitised as the surface
/// node defines, and the context's normal where nothing drives one.
#[test]
fn the_path_evaluator_reads_its_record_at_fixed_points() {
    let Some(executor) = executor() else { return };
    let (ir, _) = graph(COATED);
    let s = 1.0 / 3f32.sqrt();
    let points = [([0.3, 0.6, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]), ([-0.5, 0.1, 0.8], [s, s, s], [0.0, 0.0, 1.0])];
    let graphs = GraphMaterials::new(vec![material(&ir)], vec![binding(0)]).unwrap();
    for (point, got) in points.iter().zip(probe(&executor, graphs, &points)) {
        let want: [f32; 21] = [
            0.05, 0.2, 0.1, 0.0, 0.6, 1.5, 1.0, 0.08, 1.5, point.1[0], point.1[1], point.1[2], point.1[0], point.1[1], point.1[2], 1.0, 1.0, 1.0,
            1.0, 0.0, 0.0,
        ];
        assert_eq!(got[..21], want, "{got:?}");
        assert_eq!(got[24..], [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    }
}
