use std::collections::BTreeMap;
use std::sync::Arc;

use solidcraft_viewport::appearance_graph::{AppearanceGraph, Ir, SurfaceSample, base_response, base_sample, lower, uniform_words, validate};

use super::GraphScene;
use crate::view::glass_tests::{inner_pixels, render_frame, studio, test_frame};
use crate::view::{Frame, GpuScene, GpuTarget, ProductLook, Renderer, StudioPreset};

/// The worked examples of docs/appearance-graph.md §3.8 and the `pbr`-equivalent graph.
const STEEL: &str = include_str!("../../../viewport/tests/fixtures/appearance_graph/pbr-steel.json");
const BRUSHED: &str = include_str!("../../../viewport/tests/fixtures/appearance_graph/brushed-stainless.json");

/// A graph, lowered, with its base sample.
struct Lowered {
    ir: Arc<Ir>,
    base: SurfaceSample,
}

fn lowered(graph: &AppearanceGraph) -> Result<Lowered, String> {
    let valid = validate(graph).map_err(|e| e.to_string())?;
    let ir = Arc::new(lower(&valid).map_err(|e| e.to_string())?);
    let base = base_sample(&valid).map_err(|e| e.to_string())?;
    Ok(Lowered { ir, base })
}

fn fixture(text: &str) -> Lowered {
    let graph: AppearanceGraph = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}"));
    lowered(&graph).unwrap_or_else(|e| panic!("{e}"))
}

fn glsl(source: &str, entries: &[(naga::ShaderStage, &str)]) -> Result<Vec<String>, String> {
    use naga::back::glsl;
    let (module, info) = super::validate(source).map_err(|e| e.0)?;
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    entries
        .iter()
        .map(|(stage, entry)| {
            let pipeline = glsl::PipelineOptions { shader_stage: *stage, entry_point: (*entry).into(), multiview: None };
            let mut out = String::new();
            glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
                .and_then(|mut w| w.write().map(|_| ()))
                .map_err(|e| format!("{entry}: {e:?}"))?;
            Ok(out)
        })
        .collect()
}

/// The shape's entry points in GLSL ES 3.00: its vertex stream and `fs_mat`. The fragment reads
/// the page (a uniform-block member array) at an index from the flat varying record offset,
#[test]
fn constant_shapes_translate_for_webgl2_and_do_not_embed_values() {
    let steel = fixture(STEEL);
    let mut edited: AppearanceGraph = serde_json::from_str(STEEL).unwrap();
    edited.name = "An edited steel".into();
    let updated = lowered(&edited).unwrap();
    let source = super::shape_source(&steel.ir).unwrap();
    assert_eq!(source, super::shape_source(&updated.ir).unwrap());
    assert!(!source.contains(&edited.name));
    let entries = [(naga::ShaderStage::Fragment, "fs_mat"), (naga::ShaderStage::Vertex, "vs_mat_placed")];
    assert_eq!(glsl(&source, &entries).unwrap().len(), 2);
    assert!(super::shape_source(&fixture(BRUSHED).ir).is_err());
}

#[test]
fn records_hold_the_base_response_then_the_slots() {
    let steel = fixture(STEEL);
    let brushed = fixture(BRUSHED);
    let mut scene = GraphScene::default();
    let words = |l: &Lowered| uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
    let a = scene.appearance(&steel.ir, &words(&steel), &steel.base).unwrap_or_else(|e| panic!("{e}"));
    let b = scene.appearance(&brushed.ir, &words(&brushed), &brushed.base).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(scene.appearance(&steel.ir, &words(&steel), &steel.base), Ok(a), "an equal material shares its record");
    assert!(scene.appearance(&steel.ir, &[0; 3], &steel.base).is_err(), "words must match the graph");
    for m in [a, b] {
        scene.vertex(m, [0.0; 3], [0.0, 0.0, 1.0], [1.0, 2.0, 3.0]);
    }
    let page = scene.pages().first().map(|p| p.words.clone()).unwrap_or_default();
    let record = super::base_record(&steel.base);
    let surface = steel.base.opaque;
    assert_eq!(record[0], surface.base_color().x as f32);
    assert_eq!(record[3], surface.metallic() as f32);
    assert_eq!(record[4], surface.roughness() as f32);
    assert_eq!(page.get(..28).map(|w| w.iter().map(|x| f32::from_bits(*x)).collect::<Vec<_>>()), Some(record.to_vec()));
    let draws = scene.draws();
    assert_eq!(draws.len(), 2, "one draw per shape on a page");
    let offset = |d: &super::GraphDraw| d.vertices.get(24..28).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    assert_eq!(offset(&draws[0]), Some(0), "the first record starts the page");
    let steel_vec4s = super::BASE_VEC4 + steel.ir.uniform_floats / 4;
    assert_eq!(offset(&draws[1]), Some(steel_vec4s), "the second follows it");
    assert_eq!(draws[1].vertices.len(), if brushed.ir.object_space { super::OBJECT_VERTEX_SIZE } else { super::VERTEX_SIZE });
    // A page holds whole records: past 1024 vec4s a new page starts at 0.
    let mut full = GraphScene::default();
    let per = super::BASE_VEC4 + brushed.ir.uniform_floats / 4;
    let mut last = None;
    for i in 0..(super::PAGE_VEC4 / per + 1) {
        let mut w = words(&brushed);
        if let Some(x) = w.first_mut() {
            *x = i;
        }
        last = full.appearance(&brushed.ir, &w, &brushed.base).ok();
    }
    assert_eq!(full.pages().len(), 2);
    if let Some(m) = last {
        full.vertex(m, [0.0; 3], [0.0; 3], [0.0; 3]);
    }
    let second = full.draws().iter().find(|d| d.page == 1).and_then(offset);
    assert_eq!(second, Some(0));
}

fn target(device: &wgpu::Device) -> GpuTarget {
    GpuTarget {
        format: wgpu::TextureFormat::Rgba8Unorm,
        depth: Some(crate::depth_format(device, true)),
        samples: 1,
        tess: false,
        oit: false,
        edges: false,
    }
}

const SQUARE: [[f32; 3]; 6] =
    [[-12.0, -12.0, 0.0], [12.0, -12.0, 0.0], [12.0, 12.0, 0.0], [-12.0, -12.0, 0.0], [12.0, 12.0, 0.0], [-12.0, 12.0, 0.0]];

/// A square of `l` over the whole fixed view, facing the viewer; strip `i` of `n` when `n` > 1.
fn add_square(scene: &mut GpuScene, l: &Lowered, i: usize, n: usize) {
    let words = uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
    let m = scene.graphs.appearance(&l.ir, &words, &l.base).unwrap_or_else(|e| panic!("{e}"));
    let n = n.max(1) as f32;
    let (x0, x1) = (-12.0 + 24.0 * i as f32 / n, -12.0 + 24.0 * (i as f32 + 1.0) / n);
    for p in SQUARE {
        let p = [if p[0] < 0.0 { x0 } else { x1 }, p[1], p[2]];
        scene.graphs.vertex(m, p, [0.0, 0.0, 1.0], p);
    }
    // The face the graph shades, as the scene draws it.
    let face = scene.color([200, 200, 200, 255]);
    for p in SQUARE {
        let p = [if p[0] < 0.0 { x0 } else { x1 }, p[1], p[2]];
        scene.tri(p, [0.0, 0.0, 1.0], face);
    }
}

fn pbr_square(surface: solidcraft_viewport::surface::OpaqueSurface) -> GpuScene {
    let mut scene = GpuScene::default();
    for p in SQUARE {
        scene.pbr_tri(p, [0.0, 0.0, 1.0], surface);
    }
    scene
}

/// A frame in the realistic look: the product look with its studio, and occlusion on.
fn realistic(scene: GpuScene, key: u64, _units_per_mm: f32) -> Frame {
    Frame { key, product_look: Some(ProductLook::product()), ao_radius: 2.0, ..test_frame(scene, 1.0) }
}

fn renderer(device: &wgpu::Device, queue: &wgpu::Queue) -> Renderer {
    let mut r = Renderer::new(device, target(device), None);
    r.set_studio_environment(device, queue, &studio(StudioPreset::product()));
    r.appearance_pipelines_mut().no_time_budget = true;
    r
}

fn mean(p: &[[u8; 3]]) -> f64 {
    p.iter().map(|c| c.iter().map(|&x| f64::from(x)).sum::<f64>()).sum::<f64>() / (3 * p.len().max(1)) as f64
}

#[test]
fn pbr_steel_draws_as_its_pbr_look() {
    let Some((_, device, queue)) = crate::open_headless(crate::test_env::policy()).ok() else { return };
    let steel = fixture(STEEL);
    let Ok(valid) = validate(&serde_json::from_str::<AppearanceGraph>(STEEL).unwrap_or_else(|e| panic!("{e}"))) else { panic!("valid") };
    let surface = base_response(&valid).unwrap_or_else(|e| panic!("{e}"));
    let mut r = renderer(&device, &queue);
    let pbr = render_frame(&mut r, &device, &queue, &realistic(pbr_square(surface), 1, 1.0), true);
    let mut scene = GpuScene::default();
    add_square(&mut scene, &steel, 0, 1);
    let graph = render_frame(&mut r, &device, &queue, &realistic(scene, 2, 1.0), true);
    assert_eq!(r.appearance_stats().compiled, 1);
    assert_eq!(r.appearance_stats().fallback_draws, 0, "drawn by its own pipeline");
    let changed = pbr.as_chunks::<4>().0.iter().zip(graph.as_chunks::<4>().0.iter()).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 0, "the full frame matches the conventional PBR stream pixel for pixel");
    let b = inner_pixels(&graph);
    assert!(mean(&b) > 20.0, "lit: {}", mean(&b));
}

#[test]
fn pipeline_worker_finishes_the_constant_shape_without_changing_its_pixels() {
    let Some((_, device, queue)) = crate::open_headless(crate::test_env::policy()).ok() else { return };
    let steel = fixture(STEEL);
    let mut scene = GpuScene::default();
    add_square(&mut scene, &steel, 0, 1);
    let mut renderer = renderer(&device, &queue);
    renderer.appearance_pipelines_mut().threaded = true;
    let first = realistic(scene.clone(), 1, 1.0);
    render_frame(&mut renderer, &device, &queue, &first, true);
    renderer.appearance_pipelines_mut().settle();
    let frame = realistic(scene, 2, 1.0);
    let compiled = render_frame(&mut renderer, &device, &queue, &frame, true);
    assert!(renderer.appearance_pipelines_mut().is_ready(&steel.ir.shape));
    let reference = realistic(pbr_square(steel.base.opaque), 3, 1.0);
    assert_eq!(compiled, render_frame(&mut renderer, &device, &queue, &reference, true));
}
