//! Measurements against the targets of docs/material-graph.md §5.4, recorded in docs/perf.md.
//! Records, not checks: every test is ignored and prints its numbers. Run optimised, one at a
//! time, on an otherwise idle GPU:
//!
//! ```sh
//! cargo test --release -p solidcraft-gpu --lib material::perf_tests -- --ignored --nocapture --test-threads=1
//! ```
//!
//! GPU times are wall time from `prepare` to the end of `device.poll(wait)` per frame (no
//! read-back), after warm-up; the camera moves a little every frame so nothing is reused.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_render::material_graph::{Ir, MaterialGraph, SurfaceSample, base_sample, lower, uniform_words, validate};

use crate::view::glass_tests::{studio, test_frame};
use crate::view::{Frame, GpuScene, GpuTarget, ProductLook, Renderer, StudioPreset, far_depth, reversed_depth};

const SHOWCASE: &str = include_str!("../../../../examples/realistic-showcase.json");

/// (min, median) of `samples`, in the samples' unit.
fn spread(mut samples: Vec<f64>) -> (f64, f64) {
    samples.sort_by(f64::total_cmp);
    let min = samples.first().copied().unwrap_or(f64::NAN);
    let median = samples.get(samples.len() / 2).copied().unwrap_or(f64::NAN);
    (min, median)
}

/// `f` timed `reps` times, in milliseconds: (min, median).
fn time_ms(reps: usize, mut f: impl FnMut()) -> (f64, f64) {
    f();
    spread(
        (0..reps)
            .map(|_| {
                let t = web_time::Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect(),
    )
}

fn load() -> String {
    std::process::Command::new("uptime").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn graph_of(value: Value) -> MaterialGraph {
    serde_json::from_value(value).unwrap_or_else(|e| panic!("{e}"))
}

/// The showcase's graphs, by name.
fn showcase_graphs() -> Vec<(String, MaterialGraph)> {
    let design: Value = serde_json::from_str(SHOWCASE).unwrap_or_else(|e| panic!("{e}"));
    design["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["command"] == "appearance.graph.create")
        .map(|c| (c["params"]["id"].as_str().unwrap_or("?").to_string(), graph_of(c["params"]["graph"].clone())))
        .collect()
}

/// 127 maths nodes in a chain into the surface: the node limit (as `validate_tests`).
fn chain_128() -> MaterialGraph {
    let mut nodes = vec![json!({"id": "c0", "kind": "constant", "inputs": {"value": 0.25}})];
    for i in 1..127 {
        nodes.push(json!({"id": format!("c{i}"), "kind": if i % 2 == 0 { "add" } else { "multiply" }, "inputs": {"a": {"from": format!("c{}", i - 1)}, "b": 1}}));
    }
    nodes.push(json!({"id": "s", "kind": "standard_surface", "inputs": {"base_metalness": {"from": "c126", "fallback": 0.5}}}));
    graph_of(json!({"name": "chain", "nodes": nodes, "output": "s"}))
}

/// 128 nodes of the kinds a real look uses: a projection, noise, brushed lines, a weave and a
/// checker, then 119 maths nodes mixing them into colour, roughness and a bump.
fn mixed_128() -> MaterialGraph {
    let mut nodes = vec![
        json!({"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}}),
        json!({"id": "n", "kind": "pattern.noise", "params": {"octaves": 4, "kind": "gradient", "dimensions": 2}, "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 3}}),
        json!({"id": "b", "kind": "pattern.brushed", "inputs": {"texcoord": {"from": "uv"}}}),
        json!({"id": "w", "kind": "pattern.weave", "inputs": {"texcoord": {"from": "uv"}}}),
        json!({"id": "k", "kind": "pattern.checker", "inputs": {"texcoord": {"from": "uv"}}}),
    ];
    let sources = ["n", "b", "w", "k"];
    let kinds = ["mix", "multiply", "add", "smoothstep", "sin", "clamp", "subtract", "max"];
    let mut last = "n".to_string();
    for i in 0..119 {
        let id = format!("m{i}");
        let other = sources[i % 4];
        let inputs = match kinds[i % kinds.len()] {
            "mix" => json!({"a": {"from": last}, "b": {"from": other}, "t": 0.5}),
            "smoothstep" => json!({"edge0": 0.1, "edge1": 0.9, "in": {"from": last}}),
            "sin" | "clamp" => json!({"in": {"from": last}}),
            _ => json!({"a": {"from": last}, "b": {"from": other}}),
        };
        nodes.push(json!({"id": id, "kind": kinds[i % kinds.len()], "inputs": inputs}));
        last = id;
    }
    nodes.push(json!({"id": "rough", "kind": "remap", "inputs": {"in": {"from": last}, "out_min": 0.2, "out_max": 0.6}}));
    nodes.push(json!({"id": "col", "kind": "convert.float_to_color3", "inputs": {"in": {"from": last}}}));
    nodes.push(json!({"id": "bump", "kind": "normal.from_height", "inputs": {"height": {"from": "w"}}}));
    nodes.push(json!({"id": "s", "kind": "standard_surface", "inputs": {
        "base_color": {"from": "col"}, "specular_roughness": {"from": "rough"}, "geometry_normal": {"from": "bump"}}}));
    graph_of(json!({"name": "mixed", "nodes": nodes, "output": "s"}))
}

/// The most expensive graph of `count` 3-D gradient noises of 8 octaves summed into the colour
/// that still validates: the one nearest the 2000-unit budget.
fn budget_graph() -> (MaterialGraph, u32) {
    let make = |count: usize| {
        let mut nodes = vec![json!({"id": "p", "kind": "geometry.position", "params": {"space": "object"}})];
        let mut last: Option<String> = None;
        for i in 0..count {
            nodes.push(json!({"id": format!("n{i}"), "kind": "pattern.noise", "params": {"octaves": 8, "kind": "gradient", "dimensions": 3},
                "inputs": {"position": {"from": "p"}, "scale_mm": 1.0 + i as f64, "seed": i}}));
            let sum = format!("a{i}");
            let a = last.clone().map_or(json!(0.0), |l| json!({"from": l}));
            nodes.push(json!({"id": sum, "kind": "add", "inputs": {"a": a, "b": {"from": format!("n{i}")}}}));
            last = Some(sum);
        }
        let last = last.unwrap_or_default();
        nodes.push(json!({"id": "c", "kind": "convert.float_to_color3", "inputs": {"in": {"from": last}}}));
        nodes.push(json!({"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "c"}}}));
        graph_of(json!({"name": "budget", "nodes": nodes, "output": "s"}))
    };
    let mut best = None;
    for count in 1..64 {
        let g = make(count);
        match validate(&g).ok().and_then(|v| lower(&v).ok()) {
            Some(ir) => best = Some((g, ir.cost)),
            None => break,
        }
    }
    best.unwrap_or_else(|| panic!("no budget graph validates"))
}

struct Lowered {
    ir: Arc<Ir>,
    base: SurfaceSample,
}

fn lowered(g: &MaterialGraph) -> Lowered {
    let valid = validate(g).unwrap_or_else(|e| panic!("{e}"));
    Lowered { ir: Arc::new(lower(&valid).unwrap_or_else(|e| panic!("{e}"))), base: base_sample(&valid).unwrap_or_else(|e| panic!("{e}")) }
}

fn fixtures() -> Vec<(String, MaterialGraph)> {
    let mut all: Vec<(String, MaterialGraph)> =
        super::tests::FIXTURES.iter().map(|(n, t)| (n.to_string(), serde_json::from_str(t).unwrap_or_else(|e| panic!("{e}")))).collect();
    all.extend(showcase_graphs());
    all.push(("chain-128".into(), chain_128()));
    all.push(("mixed-128".into(), mixed_128()));
    all.push(("budget".into(), budget_graph().0));
    all
}

/// §5.4: validation + IR + base response (≤ 1 ms for 128 nodes), WGSL generation + naga
/// validation per shape (≤ 5 ms), and each material's uniform record (≤ 1 KiB).
#[test]
#[ignore = "a measurement; run optimised with --nocapture"]
fn graph_preparation_times() {
    println!("load before: {}", load());
    for (name, g) in fixtures() {
        let nodes = g.nodes.len();
        let prep = time_ms(300, || {
            let v = validate(&g).unwrap_or_else(|e| panic!("{e}"));
            let _ = lower(&v).unwrap_or_else(|e| panic!("{e}"));
            let _ = base_sample(&v).unwrap_or_else(|e| panic!("{e}"));
        });
        let l = lowered(&g);
        let generate = time_ms(30, || {
            let _ = super::shape_source(&l.ir).unwrap_or_else(|e| panic!("{e}"));
        });
        let source = super::shape_source(&l.ir).unwrap_or_default();
        let generate_validate = time_ms(30, || {
            let s = super::shape_source(&l.ir).unwrap_or_else(|e| panic!("{e}"));
            super::validate(&s).unwrap_or_else(|e| panic!("{e}"));
        });
        let record = 16 * super::BASE_VEC4 as usize + 4 * l.ir.uniform_floats as usize;
        println!(
            "{name}: {nodes} nodes, cost {} units; validate+lower+base min {:.3} / median {:.3} ms; generate {:.3}/{:.3} ms; \
             generate+naga {:.3}/{:.3} ms ({} KiB WGSL); record {record} B",
            l.ir.cost,
            prep.0,
            prep.1,
            generate.0,
            generate.1,
            generate_validate.0,
            generate_validate.1,
            source.len() / 1024
        );
    }
    println!("load after: {}", load());
}

// GPU.

/// Offscreen targets of one size, made once.
struct Target {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    depth_format: wgpu::TextureFormat,
    size: [u32; 2],
}

impl Target {
    fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let depth_format = crate::depth_format(device, true);
        let texture = |format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("perf"),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Target {
            color: texture(wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC),
            depth: texture(depth_format, wgpu::TextureUsages::RENDER_ATTACHMENT),
            depth_format,
            size,
        }
    }

    /// Draw `frame` and wait for the GPU: (ms from `prepare` to the end of the wait, ms of
    /// `prepare` alone).
    fn draw(&self, r: &mut Renderer, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame) -> (f64, f64) {
        let t = web_time::Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        r.prepare(device, queue, &mut encoder, frame);
        let prepared = t.elapsed().as_secs_f64() * 1e3;
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("perf"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(far_depth(reversed_depth(Some(self.depth_format)))),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: self
                            .depth_format
                            .has_stencil_aspect()
                            .then_some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            r.paint(frame, &mut pass, [0.0, 0.0, self.size[0] as f32, self.size[1] as f32]);
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        (t.elapsed().as_secs_f64() * 1e3, prepared)
    }
}

const SIZE: [u32; 2] = [1920, 1080];
/// Full-screen layers drawn far to near, so each one is shaded (overdraw makes the fragment
/// cost dominate the frame).
const LAYERS: usize = 8;

fn renderer(device: &wgpu::Device, queue: &wgpu::Queue) -> Renderer {
    let target =
        GpuTarget { format: wgpu::TextureFormat::Rgba8Unorm, depth: Some(crate::depth_format(device, true)), samples: 1, tess: false, oit: false };
    let mut r = Renderer::new(device, target, None);
    r.set_studio_environment(device, queue, &studio(StudioPreset::default()));
    r.materials_mut().no_time_budget = true;
    r
}

/// A frame of `scene` at [`SIZE`]: the fixed view of `test_frame` widened to the aspect, nudged
/// by `step` so no picture is reused; `look` on or off.
fn frame(scene: &GpuScene, key: u64, step: u32, look: bool) -> Frame {
    let mut f = test_frame(scene.clone(), 1.0);
    let aspect = SIZE[1] as f32 / SIZE[0] as f32;
    f.view_proj[0][0] *= aspect;
    f.view_proj[3][0] = 1e-4 * (step % 7) as f32;
    f.size_px = [SIZE[0] as f32, SIZE[1] as f32];
    f.key = key;
    f.product_look = look.then(ProductLook::default);
    f
}

/// One quad of the layer at height `z` covering the widened view.
fn layer(z: f32) -> [[f32; 3]; 6] {
    let (x, y) = (24.0, 12.0);
    [[-x, -y, z], [x, -y, z], [x, y, z], [-x, -y, z], [x, y, z], [-x, y, z]]
}

fn pbr_layers(surface: solidcraft_render::surface::OpaqueSurface) -> GpuScene {
    let mut scene = GpuScene::default();
    for i in 0..LAYERS {
        for p in layer(i as f32) {
            scene.pbr_tri(p, [0.0, 0.0, 1.0], surface);
        }
    }
    scene
}

/// `l` on [`LAYERS`] layers over the scalar faces the viewport draws under every graph face.
fn graph_layers(l: &Lowered, values: &BTreeMap<String, solidcraft_render::material_graph::Constant>) -> GpuScene {
    graph_layers_with(l, values, true)
}

fn graph_layers_with(l: &Lowered, values: &BTreeMap<String, solidcraft_render::material_graph::Constant>, underlay: bool) -> GpuScene {
    let mut scene = GpuScene::default();
    let words = uniform_words(&l.ir, values).unwrap_or_else(|e| panic!("{e}"));
    let m = scene.graphs.material(&l.ir, &words, &l.base).unwrap_or_else(|e| panic!("{e}"));
    let face = scene.color([200, 200, 200, 255]);
    for i in 0..LAYERS {
        for p in layer(i as f32) {
            scene.graphs.vertex(m, p, [0.0, 0.0, 1.0], p);
            if underlay {
                scene.tri(p, [0.0, 0.0, 1.0], face);
            }
        }
    }
    scene
}

/// Median and min frame time of `scene` over `frames` frames after warm-up.
fn frame_times(r: &mut Renderer, device: &wgpu::Device, queue: &wgpu::Queue, t: &Target, scene: &GpuScene, key: u64, look: bool) -> (f64, f64) {
    for step in 0..8 {
        t.draw(r, device, queue, &frame(scene, key, step, look));
    }
    let s = r.material_stats();
    assert_eq!((s.pending, s.fallback_draws), (0, 0), "every shape compiled before timing: {s:?}");
    spread((0..60).map(|step| t.draw(r, device, queue, &frame(scene, key, 100 + step, look)).0).collect())
}

/// §5.4 fragment cost: a constant graph against today's `fs_pbr`, a graph at the 2000-unit budget
/// against the look, the brushed (anisotropic) graph against the same graph without anisotropy,
/// and the Standard view's scalar faces; [`LAYERS`] full-screen layers at 1920 × 1080.
#[test]
#[ignore = "a measurement; run optimised with --nocapture"]
fn fragment_costs() {
    let Some((adapter, device, queue)) = crate::open_headless().ok() else { panic!("no GPU") };
    println!("adapter: {:?}; load before: {}", adapter.get_info().name, load());
    let t = Target::new(&device, SIZE);
    let copper_graph: MaterialGraph = serde_json::from_str(super::tests::FIXTURES[4].1).unwrap_or_else(|e| panic!("{e}"));
    let copper = lowered(&copper_graph);
    let surface = solidcraft_render::material_graph::base_response(&validate(&copper_graph).unwrap_or_else(|e| panic!("{e}")))
        .unwrap_or_else(|e| panic!("{e}"));
    let (budget, budget_cost) = budget_graph();
    let budget = lowered(&budget);
    let graphs = showcase_graphs();
    let brushed_graph = graphs.iter().find(|(n, _)| n == "brushed-aluminium").map(|(_, g)| g.clone()).unwrap_or_else(|| panic!("brushed"));
    let mut iso_json = serde_json::to_value(&brushed_graph).unwrap_or_else(|e| panic!("{e}"));
    for node in iso_json["nodes"].as_array_mut().into_iter().flatten() {
        if node["kind"] == "standard_surface"
            && let Some(inputs) = node["inputs"].as_object_mut()
        {
            inputs.remove("specular_roughness_anisotropy");
            inputs.remove("geometry_tangent");
        }
    }
    let brushed = lowered(&brushed_graph);
    let iso = lowered(&graph_of(iso_json));
    println!(
        "brushed shape {:02x?} cost {}, isotropic shape {:02x?} cost {}",
        &brushed.ir.shape.0[..4],
        brushed.ir.cost,
        &iso.ir.shape.0[..4],
        iso.ir.cost
    );
    let none = BTreeMap::new();
    let mut scalar = GpuScene::default();
    let grey = scalar.color([180, 180, 186, 255]);
    for i in 0..LAYERS {
        for p in layer(i as f32) {
            scalar.tri(p, [0.0, 0.0, 1.0], grey);
        }
    }
    let cases: Vec<(String, GpuScene, bool)> = vec![
        ("standard, scalar faces".into(), scalar.clone(), false),
        ("realistic, scalar faces".into(), scalar, true),
        ("realistic, pbr look (fs_pbr)".into(), pbr_layers(surface), true),
        ("realistic, constant graph (pbr-copper)".into(), graph_layers(&copper, &none), true),
        ("realistic, constant graph without the scalar underlay".into(), graph_layers_with(&copper, &none, false), true),
        (format!("realistic, budget graph ({budget_cost} units)"), graph_layers(&budget, &none), true),
        ("realistic, brushed aluminium (anisotropic)".into(), graph_layers(&brushed, &none), true),
        ("realistic, brushed aluminium without anisotropy".into(), graph_layers(&iso, &none), true),
    ];
    for (round, order) in [(0, false), (1, true), (2, false)] {
        let list: Vec<_> = if order { cases.iter().rev().collect() } else { cases.iter().collect() };
        for (i, (name, scene, look)) in list.into_iter().enumerate() {
            let mut r = renderer(&device, &queue);
            let (min, median) = frame_times(&mut r, &device, &queue, &t, scene, 10 + i as u64, *look);
            println!("round {round}: {name}: frame min {min:.3} ms, median {median:.3} ms");
        }
    }
    println!("load after: {}", load());
}

/// 48 distinct shapes (noise kind, dimensions and octaves), each followed by `salt` additions so
/// that a new salt gives shapes no earlier run compiled (the system keeps compiled Metal shaders
/// across processes; a shape seen before compiles from that cache).
fn distinct_shapes(salt: usize) -> Vec<Lowered> {
    let mut out = Vec::new();
    for kind in ["value", "gradient", "cellular"] {
        for dimensions in [2, 3] {
            for octaves in 1..=8 {
                let at = if dimensions == 2 {
                    json!({"texcoord": {"from": "uv"}, "scale_mm": 2})
                } else {
                    json!({"position": {"from": "p"}, "scale_mm": 2})
                };
                let mut nodes = vec![
                    json!({"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}}),
                    json!({"id": "p", "kind": "geometry.position", "params": {"space": "world"}}),
                    json!({"id": "a0", "kind": "pattern.noise", "params": {"octaves": octaves, "kind": kind, "dimensions": dimensions}, "inputs": at}),
                ];
                for i in 1..=salt {
                    nodes.push(json!({"id": format!("a{i}"), "kind": "add", "inputs": {"a": {"from": format!("a{}", i - 1)}, "b": 0.001}}));
                }
                nodes.push(json!({"id": "c", "kind": "convert.float_to_color3", "inputs": {"in": {"from": format!("a{salt}")}}}));
                nodes.push(json!({"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "c"}}}));
                out.push(lowered(&graph_of(json!({"name": "o", "nodes": nodes, "output": "s"}))));
            }
        }
    }
    out
}

/// §5.4 pipeline creation per new shape (≤ 30 ms native, off the critical frame): a small frame
/// that makes one new shape's pipeline, minus the same frame once it exists, minus the shape's
/// generation and naga validation measured alone. Also: a value edit (only the page rewritten)
/// and a camera change (nothing uploaded, nothing compiled).
#[test]
#[ignore = "a measurement; run optimised with --nocapture"]
fn pipeline_and_edit_costs() {
    let Some((adapter, device, queue)) = crate::open_headless().ok() else { panic!("no GPU") };
    println!("adapter: {:?}; load before: {}", adapter.get_info().name, load());
    let small = Target::new(&device, [64, 64]);
    // `PERF_SALT` (0–100) picks the shapes; a salt not used before on this machine gives cold
    // compiles. The same shapes in a second renderer are then compiled warm.
    let salt = std::env::var("PERF_SALT").ok().and_then(|s| s.parse().ok()).unwrap_or(0usize).min(100);
    let shapes = distinct_shapes(salt);
    let keys: std::collections::BTreeSet<_> = shapes.iter().map(|l| l.ir.shape).collect();
    let mut r = renderer(&device, &queue);
    let none = BTreeMap::new();
    let small_frame =
        |scene: &GpuScene, key: u64| crate::view::Frame { key, product_look: Some(ProductLook::default()), ..test_frame(scene.clone(), 1.0) };
    // The fallback and the frame's other pipelines first.
    let warm = graph_layers(&lowered(&serde_json::from_str(super::tests::FIXTURES[4].1).unwrap_or_else(|e| panic!("{e}"))), &none);
    for k in 0..4 {
        small.draw(&mut r, &device, &queue, &small_frame(&warm, 1 + k));
    }
    for (pass, label) in
        [(0u64, "first compile in this process (cold unless the salt was used before)"), (1, "same shapes, new renderer (system shader cache warm)")]
    {
        if pass == 1 {
            r = renderer(&device, &queue);
            for k in 0..4 {
                small.draw(&mut r, &device, &queue, &small_frame(&warm, 50 + k));
            }
        }
        let mut compile = Vec::new();
        for (i, l) in shapes.iter().enumerate() {
            let scene = graph_layers(l, &none);
            let key = 1000 + 2 * i as u64 + 10_000 * pass;
            let (first, _) = small.draw(&mut r, &device, &queue, &small_frame(&scene, key));
            assert_eq!(r.material_stats().compiled, 1, "one new pipeline");
            let steady = spread((0..5).map(|j| small.draw(&mut r, &device, &queue, &small_frame(&scene, key + 1 + 100_000 * (j + 1))).0).collect()).1;
            let codegen = time_ms(5, || {
                let s = super::shape_source(&l.ir).unwrap_or_else(|e| panic!("{e}"));
                super::validate(&s).unwrap_or_else(|e| panic!("{e}"));
            })
            .1;
            compile.push((first - steady - codegen).max(0.0));
        }
        let (min, median) = spread(compile.clone());
        let max = compile.iter().copied().fold(0.0, f64::max);
        println!(
            "salt {salt}, {} distinct shapes, {label}: pipeline creation (frame − steady frame − codegen) min {min:.2} ms, median {median:.2} ms, max {max:.2} ms",
            keys.len()
        );
    }

    // A value edit: the same shape with another value.
    let l = &shapes[3];
    let scene = graph_layers(l, &none);
    let t = Target::new(&device, SIZE);
    for step in 0..6 {
        t.draw(&mut r, &device, &queue, &frame(&scene, 5000, step, true));
    }
    let mut edits = Vec::new();
    let mut prepares = Vec::new();
    for e in 0..40u32 {
        let mut words = uniform_words(&l.ir, &none).unwrap_or_else(|e| panic!("{e}"));
        for w in &mut words {
            if f32::from_bits(*w) == 2.0 {
                *w = (2.0 + 0.01 * (e + 1) as f32).to_bits();
            }
        }
        let mut edited = GpuScene::default();
        let m = edited.graphs.material(&l.ir, &words, &l.base).unwrap_or_else(|e| panic!("{e}"));
        let face = edited.color([200, 200, 200, 255]);
        for i in 0..LAYERS {
            for p in layer(i as f32) {
                edited.graphs.vertex(m, p, [0.0, 0.0, 1.0], p);
                edited.tri(p, [0.0, 0.0, 1.0], face);
            }
        }
        let (whole, prepared) = t.draw(&mut r, &device, &queue, &frame(&edited, 6000 + u64::from(e), 0, true));
        let s = r.material_stats();
        assert_eq!(s.compiled, 0, "a value edit compiles nothing");
        let u = r.uploads();
        if e == 0 {
            println!("value edit uploads: {u:?}");
        }
        edits.push(whole);
        prepares.push(prepared);
    }
    let (emin, emed) = spread(edits);
    let (pmin, pmed) = spread(prepares);
    println!("value edit (new scene key, same shape): prepare min {pmin:.3} / median {pmed:.3} ms; whole frame min {emin:.3} / median {emed:.3} ms");
    // A camera change: same scene key, other view.
    let before = r.uploads();
    for step in 0..6 {
        t.draw(&mut r, &device, &queue, &frame(&scene, 7000, step, true));
    }
    let mut camera_prepare = Vec::new();
    for step in 0..40 {
        let (_, p) = t.draw(&mut r, &device, &queue, &frame(&scene, 7000, 200 + step, true));
        assert_eq!(r.material_stats().compiled, 0, "a camera change compiles nothing");
        camera_prepare.push(p);
    }
    let (cmin, cmed) = spread(camera_prepare);
    println!("camera change: prepare min {cmin:.3} / median {cmed:.3} ms; uploads before {before:?} after {:?}", r.uploads());
    println!("load after: {}", load());
}

/// §4.3/§5.4 texture costs: decode + resident preparation on the CPU (the decode worker's work)
/// and the GPU upload with its mip chain by render passes, for the showcase label and an
/// original 4096² test image.
#[test]
#[ignore = "a measurement; run optimised with --nocapture"]
fn texture_costs() {
    use super::textures::{ResidentTextures, TextureCaps, TextureKey, prepare, role_of};
    use solidcraft_render::texture_input::TextureSource;
    let Some((adapter, device, queue)) = crate::open_headless().ok() else { panic!("no GPU") };
    println!("adapter: {:?}; load before: {}", adapter.get_info().name, load());
    let label = include_bytes!("../../../../assets/examples/realistic-showcase-label.png").to_vec();
    let big = {
        let img = image::RgbaImage::from_fn(4096, 4096, |x, y| {
            let v = ((x ^ y) & 0xff) as u8;
            image::Rgba([v, (x >> 4) as u8, (y >> 4) as u8, 255])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap_or_else(|e| panic!("{e}"));
        out.into_inner()
    };
    for (name, bytes) in [("showcase label", label), ("4096² generated RGBA PNG", big)] {
        let key = TextureKey { sha256: format!("{:0>64}", name.len()), srgb: true };
        let caps = TextureCaps::of(&adapter);
        let mut resident = ResidentTextures::new(&device);
        resident.caps = caps;
        let policy = resident.policy;
        let source = || TextureSource::new(bytes.clone(), role_of(&bytes, true)).unwrap_or_else(|e| panic!("{e}"));
        let reps = if bytes.len() > 1 << 20 { 5 } else { 30 };
        let decode = time_ms(reps, || {
            let _ = prepare(key.clone(), &source(), policy, caps, &|| false).unwrap_or_else(|e| panic!("{e}"));
        });
        let mut uploads = Vec::new();
        let mut info = String::new();
        for i in 0..reps {
            let t =
                prepare(TextureKey { sha256: format!("{i:0>64}"), srgb: true }, &source(), policy, caps, &|| false).unwrap_or_else(|e| panic!("{e}"));
            info = format!("{}×{} {:?}, {} levels dropped", t.width, t.height, t.format, t.dropped);
            let start = web_time::Instant::now();
            resident.install(&device, &queue, t).unwrap_or_else(|e| panic!("{e}"));
            device.poll(wgpu::PollType::wait_indefinitely()).ok();
            uploads.push(start.elapsed().as_secs_f64() * 1e3);
        }
        let (umin, umed) = spread(uploads);
        println!(
            "{name} ({} KiB file, resident {info}): decode+prepare min {:.2} / median {:.2} ms; upload + GPU mips min {umin:.2} / median {umed:.2} ms",
            bytes.len() / 1024,
            decode.0,
            decode.1
        );
    }
    println!("load after: {}", load());
}
