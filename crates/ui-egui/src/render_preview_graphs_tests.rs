//! The Render preview's Realistic look draws graph and `pbr` looks through the material stream.

use serde_json::json;
use solidcore_record::Vec3;
use solidcraft_engine::Session;

use crate::{Services, SolveApp};

fn with_gpu(app: &mut SolveApp) {
    app.viewport.gpu = Some(crate::gpu::GpuTarget {
        format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm,
        depth: None,
        samples: 1,
        tess: false,
        oit: false,
        edges: false,
    });
}

fn bodies(app: &SolveApp) -> Vec<String> {
    crate::open_meshing::session_world(&app.session).bodies.iter().map(|b| b.name.clone()).collect()
}

/// A see-through graph (transmission 0.9), as the showcase's dome.
fn glass(app: &mut SolveApp) {
    app.run(
        "appearance.graph.create",
        json!({"id": "glass", "name": "Glass", "graph": {"name": "Glass", "nodes": [
            {"id": "s", "kind": "standard_surface", "inputs": {"base_color": [0.9, 0.95, 1.0], "transmission_weight": 0.9}}], "output": "s"}}),
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

/// A graph whose base colour is an 8 × 8 black-and-white checker image, planar in object space.
fn checker(app: &mut SolveApp) {
    let mut img = image::RgbaImage::new(8, 8);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = if (x / 4 + y / 4) % 2 == 0 { image::Rgba([250, 250, 250, 255]) } else { image::Rgba([10, 10, 10, 255]) };
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap_or_else(|e| panic!("{e}"));
    let added = app
        .run("appearance.texture.add", json!({"data_base64": solidcraft_engine::doc::canvas::base64_encode(&png), "name": "checker"}))
        .unwrap_or_else(|e| panic!("{e}"));
    let sha = added["texture"].as_str().map(str::to_owned).unwrap_or_else(|| panic!("a texture id: {added}"));
    app.run("appearance.graph.create", json!({"id": "checker", "name": "Checker", "graph": {"name": "Checker",
        "interface": [{"name": "print", "type": "texture"}],
        "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}, "inputs": {"u": [20.0, 0.0, 0.0], "v": [0.0, 15.0, 0.0]}},
            {"id": "img", "kind": "image", "params": {"space": "srgb", "wrap": "repeat", "channel": "rgb"}, "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "print"}}},
            {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "img.color", "fallback": [0.5, 0.5, 0.5]}, "specular_roughness": 0.6}}],
        "output": "s"}}))
    .unwrap_or_else(|e| panic!("{e}"));
    let body = bodies(app).first().cloned().unwrap_or_default();
    app.run("appearance.assign", json!({"bodies": [body], "graph": "checker", "values": {"print": {"texture": sha}}}))
        .unwrap_or_else(|e| panic!("{e}"));
}

/// Bytes of the preview scene's own triangles (its blocks and any loose streams).
fn own_bytes(gpu: &crate::gpu::GpuScene) -> usize {
    gpu.pbr.len() + gpu.tris.len() + gpu.glass.len() + gpu.blocks.iter().map(|b| b.bytes.vertices.len()).sum::<usize>()
}

/// In the Realistic look, the preview's scene carries the material stream: graph looks and `pbr`
/// looks alike, a see-through graph blended and left out of the preview's own triangles. The
/// Standard look's scene is the preview's as before, with no stream.
#[test]
fn the_realistic_preview_feeds_graph_and_pbr_looks_to_the_material_stream() {
    let mut app = SolveApp::new(Session::default(), Services::default());
    for x in [0, 50, 100] {
        app.run("solid.box", json!({"corner": [x, 0, 0], "length": 40, "width": 30, "height": 20})).unwrap_or_else(|e| panic!("{e}"));
    }
    glass(&mut app);
    app.run(
        "appearance.graph.create",
        json!({"id": "plain", "name": "Plain", "graph": {"name": "Plain", "nodes": [
        {"id": "s", "kind": "standard_surface", "inputs": {"base_color": [0.6, 0.3, 0.2], "base_metalness": 1}}], "output": "s"}}),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let names = bodies(&app);
    app.run("appearance.assign", json!({"bodies": [names[0]], "graph": "plain"})).unwrap_or_else(|e| panic!("{e}"));
    app.run("appearance.assign", json!({"bodies": [names[1]], "color": "#b47846", "pbr": {"metallic": 1, "roughness": 0.3, "dielectric_ior": 1.5}}))
        .unwrap_or_else(|e| panic!("{e}"));
    app.run("appearance.assign", json!({"bodies": [names[2]], "graph": "glass"})).unwrap_or_else(|e| panic!("{e}"));
    with_gpu(&mut app);
    let (scene, standard, _) = crate::render_workspace::preview_scene_for_evidence(&app).unwrap_or_else(|e| panic!("{e}"));
    assert!(standard.graphs.draws().is_empty(), "Standard: no stream");
    let triangles: Vec<usize> = scene.meshes.iter().map(|m| m.mesh.triangles.len()).collect();
    app.ui.render_preview_look = crate::view_look::REALISTIC;
    let (_, realistic, _) = crate::render_workspace::preview_scene_for_evidence(&app).unwrap_or_else(|e| panic!("{e}"));
    let draws = realistic.graphs.draws();
    let vertices = |blended: bool| -> usize {
        draws
            .iter()
            .filter(|d| d.blended == blended)
            .map(|d| d.vertices.len() / solidcraft_viewport_gpu::material::GraphScene::stride(d.object))
            .sum()
    };
    assert_eq!(vertices(false), 3 * (triangles[0] + triangles[1]), "the graph and the pbr body, opaque, over their faces");
    assert_eq!(vertices(true), 3 * triangles[2], "the see-through graph, blended");
    // The see-through body's triangles are the stream's alone; the others stay in the scene.
    let per_vertex = (own_bytes(&standard) / (3 * triangles.iter().sum::<usize>())).max(1);
    assert_eq!(own_bytes(&realistic) / per_vertex, 3 * (triangles[0] + triangles[1]));
    // The modelling view's look is a setting of its own.
    app.ui.render_preview_look = crate::view_look::STANDARD;
    app.ui.view_look = crate::view_look::REALISTIC;
    let (_, again, _) = crate::render_workspace::preview_scene_for_evidence(&app).unwrap_or_else(|e| panic!("{e}"));
    assert!(again.graphs.draws().is_empty() && own_bytes(&again) == own_bytes(&standard));
}

/// A camera on the box at the origin (40 × 30 × 20), looking down at its top, with the preview's
/// projection: its view-projection, toward-eye direction and eye.
fn top_view(app: &mut SolveApp, size: u32) -> (crate::viewport::Proj, [f32; 3], [f32; 4]) {
    let (yaw, pitch) = solidcraft_viewport::StandardView::Iso.angles();
    app.cam =
        solidcraft_viewport::Camera { target: Vec3::new(20.0, 15.0, 10.0), yaw, pitch, roll: 0.0, distance: 120.0, fov: 0.0, ..Default::default() };
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size as f32, size as f32));
    let proj = crate::viewport::projection(app, rect);
    let back = proj.cam.back().to_f32();
    let e = proj.cam.eye().to_f32();
    (proj, back, [e[0], e[1], e[2], 0.0])
}

/// The preview drawn offscreen as `paint_product` draws it, in the preview's own look.
fn draw_preview(app: &SolveApp, proj: &crate::viewport::Proj, back: [f32; 3], eye: [f32; 4], size: u32) -> Option<Vec<u8>> {
    let (scene, gpu, lights) = crate::render_workspace::preview_scene_for_evidence(app).unwrap_or_else(|e| panic!("{e}"));
    let look = crate::view_look::product_look_around(app, "render_preview", &scene.bounds);
    crate::gpu::render_look_offscreen(gpu, proj.vp.to_f32(), back, eye, size, look, Some(lights))
}

/// Pixels in a square of half side `r` around the screen point of `p`.
fn around(bytes: &[u8], proj: &crate::viewport::Proj, p: Vec3, r: i64, size: u32) -> Vec<[u8; 3]> {
    let Some(c) = proj.to_screen(p) else { return Vec::new() };
    let (cx, cy) = (c.x as i64, c.y as i64);
    let mut out = Vec::new();
    for y in cy - r..cy + r {
        for x in cx - r..cx + r {
            let i = ((y.clamp(0, i64::from(size) - 1) * i64::from(size) + x.clamp(0, i64::from(size) - 1)) * 4) as usize;
            if let Some(px) = bytes.get(i..i + 3) {
                out.push([px[0], px[1], px[2]]);
            }
        }
    }
    out
}

fn spread(px: &[[u8; 3]]) -> f64 {
    let l: Vec<f64> = px.iter().map(|p| p.iter().map(|&c| f64::from(c)).sum::<f64>() / 3.0).collect();
    let mean = l.iter().sum::<f64>() / l.len().max(1) as f64;
    (l.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / l.len().max(1) as f64).sqrt()
}

/// A graph look that reads an image draws the image in the Realistic preview: the checker shows as
/// dark and light squares on the box's top. The Standard preview draws its base response, flat.
#[test]
fn a_graph_image_draws_its_pattern_in_the_preview() {
    const SIZE: u32 = 512;
    let mut app = SolveApp::new(Session::default(), Services::default());
    app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap_or_else(|e| panic!("{e}"));
    checker(&mut app);
    with_gpu(&mut app);
    let (proj, back, eye) = top_view(&mut app, SIZE);
    let top = Vec3::new(20.0, 15.0, 20.0);
    let Some(standard) = draw_preview(&app, &proj, back, eye, SIZE) else { return };
    app.ui.render_preview_look = crate::view_look::REALISTIC;
    let Some(realistic) = draw_preview(&app, &proj, back, eye, SIZE) else { return };
    let (flat, pattern) = (spread(&around(&standard, &proj, top, 30, SIZE)), spread(&around(&realistic, &proj, top, 30, SIZE)));
    eprintln!("top face spread: standard {flat:.1}, realistic {pattern:.1}");
    assert!(flat < 4.0, "Standard: the base response, flat ({flat:.1})");
    assert!(pattern > 40.0, "Realistic: the checker's squares ({pattern:.1})");
}

/// The see-through dome blends in the Realistic preview: a glass box in front of a red one lets the
/// red through, where the Standard preview draws the glass's base response, opaque.
#[test]
fn a_see_through_graph_blends_in_the_preview() {
    const SIZE: u32 = 512;
    let mut app = SolveApp::new(Session::default(), Services::default());
    let (_, back, _) = top_view(&mut app, SIZE);
    // The glass box at the origin, the red one 60 mm straight behind its centre along the view.
    let behind = Vec3::new(20.0, 15.0, 10.0) - Vec3::new(f64::from(back[0]), f64::from(back[1]), f64::from(back[2])) * 60.0;
    app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap_or_else(|e| panic!("{e}"));
    app.run("solid.box", json!({"corner": [behind.x - 15.0, behind.y - 15.0, behind.z - 15.0], "length": 30, "width": 30, "height": 30}))
        .unwrap_or_else(|e| panic!("{e}"));
    glass(&mut app);
    let names = bodies(&app);
    app.run("appearance.assign", json!({"bodies": [names[0]], "graph": "glass"})).unwrap_or_else(|e| panic!("{e}"));
    app.run("appearance.assign", json!({"bodies": [names[1]], "color": "#d01010"})).unwrap_or_else(|e| panic!("{e}"));
    with_gpu(&mut app);
    app.cam.distance = 250.0;
    let (proj, back, eye) = {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE as f32, SIZE as f32));
        let proj = crate::viewport::projection(&app, rect);
        let e = proj.cam.eye().to_f32();
        (proj, proj.cam.back().to_f32(), [e[0], e[1], e[2], 0.0])
    };
    // The glass box's centre, seen in front of the red box.
    let probe = Vec3::new(20.0, 15.0, 10.0);
    let mean = |px: &[[u8; 3]]| {
        let n = px.len().max(1) as f64;
        [0, 1, 2].map(|c| px.iter().map(|p| f64::from(p[c])).sum::<f64>() / n)
    };
    let Some(standard) = draw_preview(&app, &proj, back, eye, SIZE) else { return };
    app.ui.render_preview_look = crate::view_look::REALISTIC;
    let Some(realistic) = draw_preview(&app, &proj, back, eye, SIZE) else { return };
    let (s, r) = (mean(&around(&standard, &proj, probe, 3, SIZE)), mean(&around(&realistic, &proj, probe, 3, SIZE)));
    eprintln!("through the glass: standard {s:?}, realistic {r:?}");
    assert!(s[0] - s[1] < 20.0, "Standard: the glass's base response hides the red box: {s:?}");
    assert!(r[0] - r[1] > 40.0, "Realistic: the red box shows through the glass: {r:?}");
}
