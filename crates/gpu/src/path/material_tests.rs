//! The path back end of the material graph: its table, its hook and its pipeline, and that a
//! constant graph renders exactly as the equivalent `pbr` look.

use std::collections::BTreeMap;
use std::sync::Arc;

use solidcore_record::Vec3;
use solidcraft_geom::{Mesh};
use solidcraft_render::job::graphs::{GraphBinding, GraphMaterial, GraphMaterials};
use solidcraft_render::job::{CancelToken, ImageRequest, ImageWork, JobOptions, TaskPoll};
use solidcraft_render::material_graph::{Constant, MaterialGraph, base_response, ir::Ir, lower, validate};
use solidcraft_render::surface::OpaqueSurface;
use solidcraft_render::{Camera, ProductMesh, ProductScene, StandardView};

use super::super::{Executor, Task, admission, open_headless};
use crate::material::textures::ResidentPolicy;

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
    assert!(Arc::ptr_eq(a.graph_pipelines.as_ref().unwrap(), b.graph_pipelines.as_ref().unwrap()), "one pipeline per shape set");
    assert!(task(None).graph_pipelines.is_none());
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
    let mut stage = scene.meshes[0].clone();
    stage.name = "__render_stage__/floor".into();
    scene.meshes.push(stage);
    let graphs = GraphMaterials::new(
        vec![material(&copper), material(&coated)],
        vec![binding(0), GraphBinding { body_index: 1, body_name: "second".into(), ..binding(1) }],
    )
    .unwrap();
    let cancel = CancelToken::default();
    let (texture, records) =
        super::super::textures::flatten_with_graphs(None, Some(&graphs), 3, Vec3::ZERO, &cancel, &ResidentPolicy::path(u64::MAX)).unwrap();
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
    let mut stage_triangles = 0;
    for t in 0..count {
        let interior = materials + t * 20 + 16;
        let z = f32::from_bits(geometry(interior + 2));
        // The body is the medium identity in optics.w (body index + 1).
        let body = f32::from_bits(geometry(materials + t * 20 + 11)) as usize - 1;
        if body == 2 {
            assert_eq!(z, 1.0, "a stage body keeps its flag, so aging still skips it");
            stage_triangles += 1;
        } else {
            assert!(z < 0.0, "every triangle of a graph-bound body names its record");
            seen.insert(z.to_bits());
        }
    }
    assert!(stage_triangles > 0);
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
    let (epoxy, _) = graph(FIXTURES[0].1);
    let graphs = GraphMaterials::new(vec![material(&copper), material(&coated), material(&epoxy)], vec![binding(0)]).unwrap();
    let source = super::super::graph_source(&super::source(&graphs).unwrap());
    let module = match naga::front::wgsl::parse_str(&source) {
        Ok(m) => m,
        Err(e) => panic!("{}", e.emit_to_string(&source)),
    };
    let validated = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module);
    assert!(validated.is_ok(), "{validated:?}");
    // Constant graphs share one shape whatever their values: one evaluator for them, one for epoxy.
    assert_eq!(graphs.shapes().len(), 2);
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

/// A body placement `[M | t]`, rows of `world = M·object + t`.
type Placement = Option<[[f64; 4]; 3]>;

/// The 37 floats (WGSL `Surface` order) each material's evaluator returns at each point, read
/// back from the GPU: what both back ends compare against the test-only reference. Material `i`
/// shades its own body, placed by its placement; one compiled job holds them all.
fn probe(executor: &Executor, materials: Vec<(GraphMaterial, Placement)>, points: &[ProbePoint]) -> Vec<Vec<[f32; 37]>> {
    let (count, n) = (materials.len(), points.len());
    assert!(count * n <= 204, "the probe's readback holds 204 surfaces");
    let mut scene = sphere(white());
    let body = scene.meshes[0].clone();
    scene.meshes = (0..count).map(|i| ProductMesh { name: format!("graph sphere {i}"), ..body.clone() }).collect();
    let bindings = materials
        .iter()
        .enumerate()
        .map(|(i, (_, object_to_world))| GraphBinding {
            body_index: i,
            body_name: format!("graph sphere {i}"),
            face: None,
            material: i,
            object_to_world: *object_to_world,
        })
        .collect();
    let graphs = GraphMaterials::new(materials.into_iter().map(|(m, _)| m).collect(), bindings).unwrap();
    // The records the job writes, laid out as the job lays them out (origin: the camera target 0).
    let (_, records) =
        super::super::textures::flatten_with_graphs(None, Some(&graphs), count, Vec3::ZERO, &CancelToken::default(), &ResidentPolicy::path(u64::MAX))
            .unwrap();
    let mut cases = String::new();
    for (i, m) in graphs.materials().iter().enumerate() {
        let shape = graphs.shapes().binary_search(&m.ir().shape).unwrap();
        let record = (-records.interior(i, 0)) as u32 - 1;
        cases.push_str(&format!("        case {i}u: {{ s = graph_eval_{shape}(graph_probe_ctx({record}u, ps[i], ns[i], vs[i])); }}\n"));
    }
    let v3 = |v: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", v[0], v[1], v[2]);
    let list = |k: usize| points.iter().map(|p| v3([p.0, p.1, p.2][k])).collect::<Vec<_>>().join(", ");
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
        "// The probe's context: the job's, with |n·v| exactly as the reference's point gives it (1:
// the viewer looks along the normal), so discontinuities at facing 1 compare like for like.
fn graph_probe_ctx(record: u32, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> sc_GraphCtx {{
    var c = graph_context(record, p, n, v);
    c.facing = 1.0;
    return c;
}}
@compute @workgroup_size(64)
fn graph_probe(@builtin(global_invocation_id) gid: vec3<u32>) {{
    if gid.x >= {total}u {{ return; }}
    var ps = array<vec3<f32>, {n}>({});
    var ns = array<vec3<f32>, {n}>({});
    var vs = array<vec3<f32>, {n}>({});
    let i = gid.x % {n}u;
    var s: sc_Surface;
    switch gid.x / {n}u {{
{cases}        default: {{ return; }}
    }}
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
        fields.join(", "),
        total = count * n,
    );
    let shader = format!("{}\n{entry}", super::super::graph_source(&super::source(&graphs).unwrap()));
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
    let mut r = request(64);
    r.graphs = Some(Arc::new(graphs));
    let mut task = Task::new(context.clone(), scene, r, JobOptions::default(), CancelToken::default()).unwrap();
    let mut encoder = context.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &task.bind, &[]);
        pass.dispatch_workgroups(((count * n) as u32).div_ceil(64), 1, 1);
    }
    context.queue.submit([encoder.finish()]);
    task.sample = 1;
    let raw = task.read_linear().unwrap();
    (0..count)
        .map(|m| {
            (0..n)
                .map(|i| {
                    let mut out = [0f32; 37];
                    for (k, x) in out.iter_mut().enumerate() {
                        let pair = raw[(m * n + i) * 20 + k / 2];
                        *x = if k % 2 == 0 { pair[0] - pair[1] } else { pair[2] - pair[3] };
                    }
                    out
                })
                .collect()
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
    for (point, got) in points.iter().zip(&probe(&executor, vec![(material(&ir), None)], &points)[0]) {
        let want: [f32; 21] = [
            0.05, 0.2, 0.1, 0.0, 0.6, 1.5, 1.0, 0.08, 1.5, point.1[0], point.1[1], point.1[2], point.1[0], point.1[1], point.1[2], 1.0, 1.0, 1.0,
            1.0, 0.0, 0.0,
        ];
        assert_eq!(got[..21], want, "{got:?}");
        assert_eq!(got[24..], [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    }
}

const FIXTURES: [(&str, &str); 6] = [
    ("black-epoxy-clear-coat", include_str!("../../../render/tests/fixtures/material_graph/black-epoxy-clear-coat.json")),
    ("brushed-nickel-silver", include_str!("../../../render/tests/fixtures/material_graph/brushed-nickel-silver.json")),
    ("electrolytic-sleeve", include_str!("../../../render/tests/fixtures/material_graph/electrolytic-sleeve.json")),
    ("laser-etched-nickel-silver", include_str!("../../../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json")),
    ("pbr-copper", include_str!("../../../render/tests/fixtures/material_graph/pbr-copper.json")),
    ("solder-mask-over-copper", include_str!("../../../render/tests/fixtures/material_graph/solder-mask-over-copper.json")),
];

/// The worst difference between the path evaluator and the test-only reference for each graph
/// `text` at the reference's fixed points, every graph on a body placed by `placement`.
fn worst_against_reference(
    executor: &Executor,
    texts: &[String],
    placement: Option<([[f64; 3]; 3], [f64; 3])>,
) -> Vec<Result<(String, f64), String>> {
    use solidcraft_render::material_graph::reference::{self, ReferenceSurface};
    let Ok(points) = reference::sample_points()
        .into_iter()
        .map(|p| match placement {
            Some((m, t)) => p.placed(m, t),
            None => Ok(p),
        })
        .collect::<Result<Vec<_>, _>>()
    else {
        return vec![Err("a singular placement".to_owned()); texts.len()];
    };
    let object_to_world = placement.map(|(m, t)| [0, 1, 2].map(|i| [m[i][0], m[i][1], m[i][2], t[i]]));
    let prepared: Vec<_> =
        texts.iter().map(|text| reference::prepare_json(text, &BTreeMap::new()).map(|(ir, words)| (Arc::new(ir), words))).collect();
    let materials: Vec<_> = prepared.iter().flatten().map(|(ir, _)| (material(ir), object_to_world)).collect();
    let f32s = |v: [f64; 3]| v.map(|x| x as f32);
    // Facing 1: the viewer looks along the normal, as the reference's points do.
    let probes: Vec<ProbePoint> = points.iter().map(|p| (f32s(p.world_position), f32s(p.world_normal), f32s(p.world_normal))).collect();
    let mut surfaces = probe(executor, materials, &probes).into_iter();
    prepared
        .into_iter()
        .map(|prepared| {
            let (ir, words) = prepared.map_err(|e| e.0)?;
            let got = surfaces.next().ok_or("missing probe")?;
            let mut worst = (String::new(), 0.0);
            for (i, (point, got)) in points.iter().zip(got).enumerate() {
                let mut want = reference::evaluate(&ir, &words, &[], point).map_err(|e| e.0)?.surface;
                let mut have = ReferenceSurface::from_floats(&got).map_err(|e| e.0)?;
                // The tangent is zeroed at exactly zero anisotropy; f32 and f64 may land on either side.
                if want.anisotropy.abs() < 1e-6 && have.anisotropy.abs() < 1e-6 {
                    (want.tangent, have.tangent) = ([0.0; 3], [0.0; 3]);
                }
                let (field, d) = want.max_difference(&have);
                if d > worst.1 {
                    worst = (format!("{field} at point {i}"), d);
                }
            }
            Ok(worst)
        })
        .collect()
}

/// Every worked example evaluates in the path tracer as the reference does, on unplaced and placed
/// bodies. The viewport's back end checks the same reference at the same points
/// (`material::tests`), so the two back ends agree there (§5.7).
#[test]
fn the_fixtures_match_the_reference_in_the_path_tracer() {
    let Some(executor) = executor() else { return };
    let (c, s) = (0.6f64, 0.8f64);
    let turn = ([[2.0 * c, -2.0 * s, 0.0], [2.0 * s, 2.0 * c, 0.0], [0.0, 0.0, 2.0]], [3.0, -1.0, 2.0]);
    let texts: Vec<String> = FIXTURES.iter().map(|(_, t)| (*t).to_owned()).collect();
    for placement in [None, Some(turn)] {
        for ((name, _), worst) in FIXTURES.iter().zip(worst_against_reference(&executor, &texts, placement)) {
            let (field, d) = worst.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(d < 1e-3, "{name} (placed: {}): {field} differs by {d}", placement.is_some());
        }
    }
}

/// The viewport back end's generated corpus evaluates in the path tracer as the reference does.
#[test]
fn the_generated_corpus_matches_the_reference_in_the_path_tracer() {
    let Some(executor) = executor() else { return };
    let mut fuzz = crate::material::tests::Fuzz(0x9e37_79b9_7f4a_7c15);
    let texts: Vec<(usize, String)> = (0..96)
        .map(|i| (i, fuzz.graph().to_string()))
        .filter(|(_, t)| solidcraft_render::material_graph::reference::prepare_json(t, &BTreeMap::new()).is_ok())
        .collect();
    assert!(texts.len() >= 64, "{} compared", texts.len());
    let mut bad = Vec::new();
    // A job compiles at most MAX_GRAPH_SHAPES shapes; batches of 16 stay well inside it.
    for batch in texts.chunks(16) {
        let graphs: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        for ((i, _), worst) in batch.iter().zip(worst_against_reference(&executor, &graphs, None)) {
            match worst {
                Ok((field, d)) if d >= 1e-3 => bad.push(format!("graph {i}: {field} by {d}")),
                Ok(_) => {}
                Err(e) => bad.push(format!("graph {i}: {e}")),
            }
        }
    }
    assert!(bad.is_empty(), "{} of {} graphs differ:\n{}", bad.len(), texts.len(), bad.join("\n"));
}

/// An original 4×4 RGBA test image, every texel distinct, as PNG.
fn test_image() -> (Vec<u8>, Vec<u8>) {
    let rgba: Vec<u8> = (0..16u32).flat_map(|i| [i * 16, 255 - i * 13, (i * 37) % 251, 64 + i * 11].map(|c| c as u8)).collect();
    let image = image::RgbaImage::from_raw(4, 4, rgba.clone()).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    (rgba, png.into_inner())
}

/// `image` nodes sample the texture buffer as the reference samples the image: bilinear at the
/// top-row-first texels, sRGB decoded before filtering, under every wrap mode and channel; their
/// one-texel slopes tilt a height-driven normal as the reference's do.
#[test]
fn images_sample_as_the_reference_does() {
    use solidcraft_render::job::graphs::GraphTexture;
    use solidcraft_render::material_graph::reference::{self, ReferenceSurface, ReferenceTexture};
    let Some(executor) = executor() else { return };
    let (rgba, png) = test_image();
    let texture = ReferenceTexture::from_rgba8(4, 4, &rgba).unwrap();
    let sha = "ab".repeat(32);
    let mut texts = Vec::new();
    for (space, wrap, channel) in [("srgb", "repeat", "rgb"), ("linear", "clamp", "a"), ("srgb", "mirror", "g"), ("linear", "repeat", "rgb")] {
        let graph = serde_json::json!({"name": "image", "nodes": [
            {"id": "tc", "kind": "texcoord.planar", "inputs": {"u": [2.5, 0.5, 0.0], "v": [-0.5, 2.5, 0.5]}, "params": {"space": "world"}},
            {"id": "img", "kind": "image", "params": {"space": space, "wrap": wrap, "channel": channel},
             "inputs": {"texcoord": {"from": "tc"}, "file": {"texture": sha}}},
            {"id": "bump", "kind": "normal.from_height", "inputs": {"height": {"from": "img.value"}, "relief_mm": 0.05}},
            {"id": "surface", "kind": "standard_surface", "inputs": {"base_color": {"from": "img"}, "specular_roughness": {"from": "img.value"},
             "geometry_normal": {"from": "bump"}}}],
            "output": "surface"});
        texts.push((space == "srgb", graph.to_string()));
    }
    let points = reference::sample_points();
    let f32s = |v: [f64; 3]| v.map(|x| x as f32);
    let probes: Vec<ProbePoint> = points.iter().map(|p| (f32s(p.world_position), f32s(p.world_normal), f32s(p.world_normal))).collect();
    let mut prepared = Vec::new();
    let mut materials = Vec::new();
    for (srgb, text) in &texts {
        let (ir, words) = reference::prepare_json(text, &BTreeMap::new()).unwrap();
        let ir = Arc::new(ir);
        let m = GraphMaterial::new(ir.clone(), &BTreeMap::new(), vec![Some(GraphTexture::new(png.clone(), *srgb).unwrap())]).unwrap();
        materials.push((m, None));
        prepared.push((ir, words));
    }
    for ((ir, words), got) in prepared.iter().zip(probe(&executor, materials, &probes)) {
        for (point, got) in points.iter().zip(got) {
            let want = reference::evaluate(ir, words, &[Some(&texture)], point).unwrap().surface;
            let (field, d) = want.max_difference(&ReferenceSurface::from_floats(&got).unwrap());
            assert!(d < 1e-4, "{field} differs by {d} at {point:?}");
        }
    }
}

/// A session with the path tracer installed, one box and a saved gpu_path setup `Graph`.
fn graph_design(executor: Executor, graph: serde_json::Value) -> solidcraft_engine::Session {
    use serde_json::json;
    let host = solidcraft_render::job::ImageHost::with_executor(Arc::new(executor)).unwrap();
    let mut session = solidcraft_engine::Session::default();
    session.register_render_job_host(host).unwrap();
    session.execute("solid.box", &json!({"length": 20, "width": 15, "height": 4, "body_name": "Coupon"})).unwrap();
    session.execute("appearance.graph.create", &json!({"id": "look", "name": "Look", "graph": graph})).unwrap();
    session.execute("appearance.assign", &json!({"bodies": ["Coupon"], "graph": "look"})).unwrap();
    let camera = session.execute("render.camera.fit", &json!({})).unwrap()["camera"].clone();
    session
        .execute(
            "render.settings.set",
            &json!({"name": "Graph", "set": {"width": 48, "height": 32, "camera": camera,
                "job": {"method": "gpu_path", "options": {"samples": 4, "max_depth": 3, "seed": 20261104}}}}),
        )
        .unwrap();
    session
}

fn completed(session: &mut solidcraft_engine::Session) -> (solidcraft_render::job::JobStatus, image::RgbaImage) {
    use solidcraft_render::job::{JobId, Phase};
    let start = session.execute("render.start", &serde_json::json!({"setup": "Graph"})).unwrap();
    let id: JobId = serde_json::from_value(start["job"].clone()).unwrap();
    let handle = session.render_job_handle(id).unwrap();
    let status = handle.wait_timeout(std::time::Duration::from_secs(120)).unwrap();
    assert_eq!(status.progress.phase, Phase::Completed, "{:?}", status.error);
    let frame = handle.frame().unwrap().unwrap();
    (status, image::load_from_memory(frame.png()).unwrap().to_rgba8())
}

/// A design whose look is a material graph renders through the path tracer from its saved setup:
/// the look's checker of red and blue shows, where every other renderer draws its one base colour.
#[test]
fn a_design_with_a_graph_look_renders_through_the_path_tracer() {
    use serde_json::json;
    let Some(executor) = executor() else { return };
    let checker = json!({"name": "Checker", "nodes": [
        {"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}, "inputs": {"u": [5.0, 0.0, 0.0], "v": [0.0, 5.0, 0.0]}},
        {"id": "chk", "kind": "pattern.checker", "inputs": {"texcoord": {"from": "uv"}, "size_mm": [0.5, 0.5]}},
        {"id": "paint", "kind": "mix", "inputs": {"a": [0.8, 0.05, 0.05], "b": [0.05, 0.05, 0.8], "t": {"from": "chk"}}},
        {"id": "surface", "kind": "standard_surface", "inputs": {"base_color": {"from": "paint", "fallback": [0.5, 0.5, 0.5]}, "specular_roughness": 0.6}}],
        "output": "surface"});
    let mut session = graph_design(executor, checker);
    let check = session.execute("render.preflight", &json!({"setup": "Graph"})).unwrap();
    assert_eq!(check["resolved"]["executor"], "gpu-path");
    assert_eq!(check["graphs"]["drawn_as"], "path_tracer", "{}", check["graphs"]);
    assert_eq!(check["request"]["graphs"]["materials"], 1);
    assert_eq!(check["graphs"]["approximations"], json!([]), "a checker of base colours is exact in the path tracer");
    let (status, image) = completed(&mut session);
    assert_eq!(status.graphs.as_ref().map(|g| g.bindings), Some(1), "the job result carries the graph summary");
    let red = image.pixels().filter(|p| p.0[0] > p.0[2].saturating_add(30)).count();
    let blue = image.pixels().filter(|p| p.0[2] > p.0[0].saturating_add(30)).count();
    assert!(red > 20 && blue > 20, "both checker colours show: {red} red, {blue} blue pixels");
}

/// A see-through graph look gives its body glass in the path tracer, which refracts only through
/// captured transmission.
#[test]
fn a_see_through_graph_look_renders_as_glass() {
    use serde_json::json;
    let Some(executor) = executor() else { return };
    let glass = json!({"name": "Glass", "nodes": [{"id": "surface", "kind": "standard_surface", "inputs": {
        "base_color": [0.9, 0.9, 0.9], "specular_roughness": 0.05, "transmission_weight": 1.0,
        "transmission_color": [0.6, 0.8, 0.9], "transmission_depth_mm": 10.0}}], "output": "surface"});
    let mut session = graph_design(executor, glass);
    let check = session.execute("render.preflight", &json!({"setup": "Graph"})).unwrap();
    assert_eq!(check["refraction"], true, "{check}");
    assert_eq!(check["graphs"]["approximations"], json!([]), "weight 1 is the path tracer's glass, exactly");
    completed(&mut session);
}

/// Graph textures are resident at the device's size (`ResidentPolicy::path`), the viewport's
/// power-of-two filter, and 16-bit grey height keeps its 16 bits in every colour channel.
#[test]
fn graph_textures_are_resident_sized_and_keep_height_precision() {
    use solidcraft_render::job::graphs::GraphTexture;
    let image_graph = |srgb: bool| {
        let space = if srgb { "srgb" } else { "linear" };
        let text = serde_json::json!({"name": "image", "nodes": [
            {"id": "tc", "kind": "texcoord.planar"},
            {"id": "img", "kind": "image", "params": {"space": space}, "inputs": {"texcoord": {"from": "tc"}, "file": {"texture": "cd".repeat(32)}}},
            {"id": "surface", "kind": "standard_surface", "inputs": {"base_color": {"from": "img"}}}], "output": "surface"});
        graph(&text.to_string()).0
    };
    let encode = |image: image::DynamicImage| {
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    };
    let words = |bytes: &[u8], i: usize| f32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap());
    // 64 × 64 colour on a device whose binding holds 16 × 16 texels.
    let big =
        encode(image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, y| image::Rgba([(x * 4) as u8, (y * 4) as u8, 128, 255]))));
    let ir = image_graph(true);
    let m = GraphMaterial::new(ir, &BTreeMap::new(), vec![Some(GraphTexture::new(big, true).unwrap())]).unwrap();
    let graphs = GraphMaterials::new(vec![m], vec![binding(0)]).unwrap();
    let cancel = CancelToken::default();
    let small = ResidentPolicy::path(16 * 16 * 16);
    assert_eq!(small.max_edge, 16);
    let (texture, _) = super::super::textures::flatten_with_graphs(None, Some(&graphs), 1, Vec3::ZERO, &cancel, &small).unwrap();
    let (full, _) =
        super::super::textures::flatten_with_graphs(None, Some(&graphs), 1, Vec3::ZERO, &cancel, &ResidentPolicy::path(u64::MAX)).unwrap();
    assert_eq!(full.len() - texture.len(), (64 * 64 - 16 * 16) * 16, "two levels dropped");
    // 16-bit grey height: values between 8-bit steps survive.
    let height = encode(image::DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(2, 2, |x, y| image::Luma([1 + 300 * (x + 2 * y) as u16]))));
    let m = GraphMaterial::new(image_graph(false), &BTreeMap::new(), vec![Some(GraphTexture::new(height, false).unwrap())]).unwrap();
    let graphs = GraphMaterials::new(vec![m], vec![binding(0)]).unwrap();
    let (texture, _) =
        super::super::textures::flatten_with_graphs(None, Some(&graphs), 1, Vec3::ZERO, &cancel, &ResidentPolicy::path(u64::MAX)).unwrap();
    // The 16-word header and lookup (one body), then the pixels.
    let first = 16 / 4;
    for (texel, raw) in [1u16, 301, 601, 901].into_iter().enumerate() {
        let v = f32::from(raw) / 65535.0;
        assert_eq!([0, 1, 2, 3].map(|k| words(&texture, first + 4 * texel + k)), [v, v, v, 1.0]);
    }
}

/// What the path tracer approximates is listed by `render.preflight` and in the job result, per
/// bound body: here a brushed look's anisotropy (isotropic) and its emission (not emitted).
#[test]
fn preflight_and_the_job_result_list_the_approximations() {
    use serde_json::json;
    let Some(executor) = executor() else { return };
    let brushed = json!({"name": "Brushed", "nodes": [{"id": "surface", "kind": "standard_surface", "inputs": {
        "base_color": [0.7, 0.7, 0.72], "base_metalness": 1.0, "specular_roughness": 0.3,
        "specular_roughness_anisotropy": 0.6, "emission_luminance": 2.0}}], "output": "surface"});
    let mut session = graph_design(executor, brushed);
    let check = session.execute("render.preflight", &json!({"setup": "Graph"})).unwrap();
    let want = json!([{"body_index": 0, "body_name": "Coupon", "inputs": [
        {"input": "specular_roughness_anisotropy", "treatment": "isotropic"},
        {"input": "emission_luminance", "treatment": "not_emitted"}]}]);
    assert_eq!(check["graphs"]["approximations"], want, "{}", check["graphs"]);
    assert_eq!(check["request"]["graphs"]["approximations"], want);
    let (status, _) = completed(&mut session);
    assert_eq!(serde_json::to_value(status.graphs.unwrap().approximations).unwrap(), want, "the job result reports them too");
}

/// Diagnostics (track C1, `--ignored --nocapture`): the brushed nickel silver fixture on a sphere
/// through the path tracer, against a plain metal and a plain dielectric of its roughness; the
/// luminance's range over the sphere shows whether the metal reflects its surroundings.
#[test]
#[ignore]
fn brushed_sphere_in_the_path_tracer() {
    let Some(executor) = executor() else { return };
    let text = include_str!("../../../render/tests/fixtures/material_graph/brushed-nickel-silver.json");
    let (ir, _) = graph(text);
    let stats = |px: &[[f32; 4]]| {
        let lum: Vec<f32> = px.iter().map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).filter(|l| l.is_finite()).collect();
        let (min, max) = lum.iter().fold((f32::MAX, f32::MIN), |(a, b), l| (a.min(*l), b.max(*l)));
        let mean = lum.iter().sum::<f32>() / lum.len().max(1) as f32;
        format!("mean {mean:.3} min {min:.3} max {max:.3}")
    };
    let mut with_graph = request(48);
    with_graph.graphs = Some(Arc::new(GraphMaterials::new(vec![material(&ir)], vec![binding(0)]).unwrap()));
    let brushed = render(&executor, sphere(white()), with_graph);
    let metal = render(&executor, sphere(OpaqueSurface::new(Vec3::new(0.8, 0.77, 0.7), 1.0, 0.28, 1.5).unwrap()), request(48));
    let mirror = render(&executor, sphere(OpaqueSurface::new(Vec3::new(0.8, 0.77, 0.7), 1.0, 0.15, 1.5).unwrap()), request(48));
    let plastic = render(&executor, sphere(OpaqueSurface::new(Vec3::new(0.8, 0.77, 0.7), 0.0, 0.28, 1.5).unwrap()), request(48));
    eprintln!(
        "path: brushed {} | metal 0.28 {} | metal 0.15 {} | dielectric 0.28 {}",
        stats(&brushed),
        stats(&metal),
        stats(&mirror),
        stats(&plastic)
    );
}
