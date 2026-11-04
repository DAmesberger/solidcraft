//! The lighting review set and its per-part luminance metrics (the acceptance bar of the material
//! graph note, §5.9): the Sample Plate and simple shapes drawn offscreen on the GPU through the
//! viewport's renderer with the product look, at least 1600 px wide, with a mask pass that names
//! every B-rep face's pixels. Explicit only:
//!
//! ```sh
//! LIGHTING_REVIEW_OUT=<dir> LIGHTING_REVIEW_TAG=after [LIGHTING_REVIEW_STUDIO=soft|contrast] [LIGHTING_REVIEW_INTENSITY=0.75] \
//!     cargo test -p solidcraft-ui-egui --release lighting_review -- --ignored --nocapture
//! ```
//!
//! It writes `<tag>-<scene>-<material>-<pose>.png` and one JSON line of metrics per frame to
//! `<dir>/<tag>-metrics.jsonl`. The metrics are measured on the frames, never on the CPU's own
//! shading: luminance is Rec. 709 luma of the 8-bit display values.

use solidcraft_engine::geom::prelude::*;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcore_record::Vec3;
use solidcraft_engine::render::{Camera, ProductScene, surface::OpaqueSurface};

use super::{Frame, ProductLook, Renderer, StudioEnvironment, StudioPreset, ViewportCallback};
use super::{GpuScene, GpuTarget, wgpu};

const SIZE: u32 = 1600;
const SSAA: f32 = 2.0;
/// The viewport's light-theme background (`capture::BACKGROUND`'s top).
const BACKGROUND: [f64; 3] = [236.0 / 255.0, 240.0 / 255.0, 245.0 / 255.0];

struct Material {
    name: &'static str,
    color: &'static str,
    metallic: f64,
    roughness: f64,
}

const PLATE_MATERIALS: [Material; 4] = [
    Material { name: "polished-metal", color: "#c9ccd1", metallic: 1.0, roughness: 0.08 },
    Material { name: "rough-metal", color: "#c9ccd1", metallic: 1.0, roughness: 0.42 },
    Material { name: "white-plastic", color: "#e8e8e6", metallic: 0.0, roughness: 0.35 },
    Material { name: "black-plastic", color: "#1c1c1e", metallic: 0.0, roughness: 0.35 },
];

const SHAPE_MATERIALS: [Material; 4] = [
    Material { name: "chrome", color: "#eceef0", metallic: 1.0, roughness: 0.05 },
    Material { name: "gold", color: "#f2c46a", metallic: 1.0, roughness: 0.18 },
    Material { name: "matte-grey", color: "#8a8a8a", metallic: 0.0, roughness: 0.85 },
    Material { name: "glossy-red", color: "#b8322a", metallic: 0.0, roughness: 0.12 },
];

/// Camera poses (yaw, pitch): isometric front-left, and a low three-quarter view from the back.
const POSES: [(&str, f64, f64); 2] = [("iso-front-left", std::f64::consts::FRAC_PI_4, 0.6154797086703874), ("low-back", -2.35, 0.26)];

fn session(scene: &str) -> Option<(Session, String)> {
    let mut s = Session::default();
    let run = |s: &mut Session, command: &str, params: Value| s.execute(command, &params).map_err(|e| eprintln!("{command}: {e}")).ok();
    let body = match scene {
        "sample-plate" => {
            s.run_script(&solidcraft_engine::sample::script()).map_err(|e| eprintln!("sample: {e}")).ok()?;
            "Plate"
        }
        "sphere" => {
            run(&mut s, "solid.sphere", json!({"radius": 15, "body_name": "Sphere"}))?;
            "Sphere"
        }
        "cylinder" => {
            run(&mut s, "solid.cylinder", json!({"base": [0, 0, 0], "axis": [0, 0, 1], "radius": 12, "height": 30, "body_name": "Cylinder"}))?;
            "Cylinder"
        }
        "chamfered-box" => {
            run(&mut s, "solid.box", json!({"length": 30, "width": 30, "height": 30, "corner": [0, 0, 0], "body_name": "Box"}))?;
            let edges = json!([
                [15, 0, 0],
                [15, 30, 0],
                [15, 0, 30],
                [15, 30, 30],
                [0, 15, 0],
                [30, 15, 0],
                [0, 15, 30],
                [30, 15, 30],
                [0, 0, 15],
                [30, 0, 15],
                [0, 30, 15],
                [30, 30, 15]
            ]);
            run(&mut s, "solid.chamfer", json!({"body": "Box", "edges": edges, "distance": 3}))?;
            "Box"
        }
        _ => return None,
    };
    let name = s
        .model
        .state()
        .bodies
        .iter()
        .map(|b| b.name.clone())
        .find(|n| n == body)
        .or_else(|| s.model.state().bodies.first().map(|b| b.name.clone()))?;
    Some((s, name))
}

/// One linear channel level of the mask's face code: digits 0–6 at 0.1–0.7, which PBR Neutral
/// leaves below its shoulder and shifts by 0.04 only, so each survives as a distinct 8-bit value.
fn level(digit: u32) -> f64 {
    0.1 + 0.1 * f64::from(digit.min(6))
}

fn display(linear: f64) -> f64 {
    let c = linear.clamp(0.0, 1.0);
    if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// The 8-bit value each digit's level reads back as through the look (−0.04, sRGB).
fn digit_bytes() -> [f64; 7] {
    std::array::from_fn(|d| 255.0 * display(level(d as u32) - 0.04))
}

fn face_code(face: u32) -> Vec3 {
    let (a, b, c) = (face % 7, (face / 7) % 7, (face / 49) % 7);
    Vec3::new(level(a), level(b), level(c))
}

/// The face a mask pixel names (`None`: background, or an edge pixel blended between faces).
fn decode(pixel: &[u8], digits: &[f64; 7]) -> Option<u32> {
    let mut face = 0;
    let mut scale = 1;
    for &v in pixel.iter().take(3) {
        let (d, error) = digits.iter().enumerate().map(|(d, b)| (d as u32, (f64::from(v) - b).abs())).min_by(|a, b| a.1.total_cmp(&b.1))?;
        if error > 3.5 {
            return None;
        }
        face += d * scale;
        scale *= 7;
    }
    Some(face)
}

/// The product scene's PBR triangles: as they are, or (`mask`) each face in its code colour, a
/// lossless diffuse with no interface lit by a uniform ambient light of radiance π.
fn gpu_scene(scene: &ProductScene, mask: bool) -> GpuScene {
    let mut gpu = GpuScene::default();
    for body in &scene.meshes {
        for (index, triangle) in body.mesh.triangles.iter().enumerate() {
            let face = body.mesh.tri_face.get(index).copied().unwrap_or(0);
            let look = body.faces.iter().find(|f| f.face == face).and_then(|f| f.surface).or(body.surface);
            let surface = if mask { OpaqueSurface::new(face_code(face), 0.0, 1.0, 1.0).ok() } else { look };
            let Some(surface) = surface else { continue };
            for i in triangle {
                let (Some(p), Some(n)) = (body.mesh.positions.get(*i as usize), body.mesh.normals.get(*i as usize)) else { continue };
                let n = n.normalized().unwrap_or(Vec3::Z);
                gpu.pbr_tri(p.to_f32(), n.to_f32(), surface);
            }
        }
    }
    if !mask {
        ground_shadow(&mut gpu, scene);
    }
    gpu
}

/// The viewport's ground shadow (`viewport::ground_shadow`, light theme): a soft ellipse under the
/// bounds.
fn ground_shadow(sc: &mut GpuScene, scene: &ProductScene) {
    let b = scene.bounds;
    let (size, c) = (b.size(), b.center());
    let z = b.min.z - 1e-3 * (1.0 + b.diagonal());
    let (rx, ry) = (size.x * 0.62 + size.z * 0.15, size.y * 0.62 + size.z * 0.15);
    let dark = 80;
    let rings = [(0.0, dark), (0.55, dark * 3 / 4), (0.85, dark / 3), (1.0, 0)];
    let n = 48;
    let at = |r: f64, k: usize| {
        let a = k as f64 / n as f64 * std::f64::consts::TAU;
        [(c.x + rx * r * a.cos()) as f32, (c.y + ry * r * a.sin()) as f32, z as f32]
    };
    for w in rings.windows(2) {
        let ((r0, a0), (r1, a1)) = (w[0], w[1]);
        let (c0, c1) = (sc.color([0, 0, 0, a0 as u8]), sc.color([0, 0, 0, a1 as u8]));
        for k in 0..n {
            let (p00, p01, p10, p11) = (at(r0, k), at(r0, k + 1), at(r1, k), at(r1, k + 1));
            sc.shadow_tri(p00, c0);
            sc.shadow_tri(p10, c1);
            sc.shadow_tri(p11, c1);
            if r0 > 0.0 {
                sc.shadow_tri(p00, c0);
                sc.shadow_tri(p11, c1);
                sc.shadow_tri(p01, c0);
            }
        }
    }
}

// BEFORE-PATCH-START: the renderer branch's look has no room; the before build returns `look`.
fn placed(look: ProductLook, scene: &ProductScene) -> ProductLook {
    let (min, max) = (scene.bounds.min.to_f32(), scene.bounds.max.to_f32());
    ProductLook { room: solidcraft_gpu::view::StudioRoom::around(min, max), ..look }
}
// BEFORE-PATCH-END

struct Shot {
    scene: GpuScene,
    look: ProductLook,
    lights: Option<Vec<[f32; 16]>>,
    ao_radius: f32,
    clear: [f64; 3],
    studio: Option<Arc<StudioEnvironment>>,
}

/// Draw `shot` offscreen through the viewport's frame callback: straight RGBA, `SIZE` × `SIZE`.
fn draw(device: &wgpu::Device, queue: &wgpu::Queue, camera: &Camera, radius: f64, shot: Shot) -> Option<Vec<u8>> {
    let depth = solidcraft_gpu::depth_format(device, true);
    let target = GpuTarget { format: wgpu::TextureFormat::Rgba8Unorm, depth: Some(depth), samples: 1, tess: false, oit: false };
    let mut resources = egui_wgpu::CallbackResources::default();
    let mut renderer = Renderer::new(device, target, None);
    if let Some(env) = &shot.studio {
        renderer.set_studio_environment(device, queue, env);
    }
    resources.insert(renderer);
    let none = || Arc::new(Mutex::new(None));
    let e = camera.eye().to_f32();
    let cb = ViewportCallback(Frame {
        hatch: Default::default(),
        product_lights: shot.lights,
        product_look: Some(shot.look),
        units_per_mm: 1.0,
        product: true,
        upload_error: None,
        key: 1,
        slot: Arc::new(Mutex::new(Some(shot.scene))),
        sketch_key: 0,
        sketch_slot: none(),
        hl_key: 0,
        hl_slot: none(),
        pv_key: 0,
        pv_slot: none(),
        pv_hide: false,
        view_proj: camera.view_proj(1.0, radius).to_f32(),
        back: camera.back().to_f32(),
        size_px: [SIZE as f32; 2],
        clip: [0.0; 4],
        cap: [0.0; 4],
        images: Vec::new(),
        clouds: Vec::new(),
        analysis: [0.0; 8],
        analysis_colors: [0.0; 36],
        access: None,
        outline: false,
        eye: [e[0], e[1], e[2], 1.0],
        ao_radius: shot.ao_radius,
        ao_samples: solidcraft_gpu::view::AO_SAMPLES,
        ssaa: SSAA,
        sky: [0.0; 4],
        hover_tint: [0.0; 3],
        selection_tint: [0.0; 3],
    });
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lighting review"),
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth_tex = texture(depth, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth_tex.create_view(&Default::default()));
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [SIZE, SIZE], pixels_per_point: 1.0 };
    let extra = egui_wgpu::CallbackTrait::prepare(&cb, device, queue, &screen, &mut encoder, &mut resources);
    {
        let [r, g, b] = shot.clear;
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lighting review"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE as f32, SIZE as f32));
        let info = egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [SIZE, SIZE] };
        egui_wgpu::CallbackTrait::paint(&cb, info, &mut pass, &resources);
    }
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lighting review read"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &read,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: Some(SIZE) },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit(extra.into_iter().chain([encoder.finish()]));
    let slice = read.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let bytes = slice.get_mapped_range().ok()?.to_vec();
    Some(bytes)
}

fn luma(p: &[u8]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// What each B-rep face is: its mean normal, whether it is planar, and its triangle count.
fn faces(scene: &ProductScene) -> std::collections::BTreeMap<u32, (Vec3, bool)> {
    let mut normals: std::collections::BTreeMap<u32, Vec<Vec3>> = Default::default();
    for body in &scene.meshes {
        for (index, triangle) in body.mesh.triangles.iter().enumerate() {
            let face = body.mesh.tri_face.get(index).copied().unwrap_or(0);
            let entry = normals.entry(face).or_default();
            for i in triangle {
                if let Some(n) = body.mesh.normals.get(*i as usize) {
                    entry.push(n.normalized().unwrap_or(Vec3::Z));
                }
            }
        }
    }
    normals
        .into_iter()
        .map(|(face, ns)| {
            let sum = ns.iter().fold(Vec3::ZERO, |a, n| a + *n);
            let mean = sum.normalized().unwrap_or(Vec3::Z);
            let planar = ns.iter().all(|n| n.dot(mean) > 0.9995);
            (face, (mean, planar))
        })
        .collect()
}

/// The frame's metrics against the acceptance bar: per part (the whole body) the clipped and
/// crushed shares, and per face (eroded by a pixel, at least 400 pixels) the median, 5th and 95th
/// percentiles and their span.
fn metrics(beauty: &[u8], mask: &[u8], scene: &ProductScene) -> Value {
    let digits = digit_bytes();
    let n = SIZE as usize;
    let ids: Vec<Option<u32>> = mask.chunks(4).map(|p| decode(p, &digits)).collect();
    let mut part = Vec::new();
    let mut per_face: std::collections::BTreeMap<u32, Vec<f64>> = Default::default();
    for y in 1..n - 1 {
        for x in 1..n - 1 {
            let i = y * n + x;
            let Some(face) = ids[i] else { continue };
            let y_luma = luma(&beauty[4 * i..4 * i + 4]);
            part.push(y_luma);
            if [i - 1, i + 1, i - n, i + n].iter().all(|j| ids[*j] == Some(face)) {
                per_face.entry(face).or_default().push(y_luma);
            }
        }
    }
    let total = part.len().max(1) as f64;
    let clipped = part.iter().filter(|v| **v >= 254.0).count() as f64 / total;
    let crushed = part.iter().filter(|v| **v <= 5.0).count() as f64 / total;
    part.sort_by(f64::total_cmp);
    let kinds = faces(scene);
    let mut face_rows = Vec::new();
    for (face, mut values) in per_face {
        if values.len() < 400 {
            continue;
        }
        values.sort_by(f64::total_cmp);
        let (normal, planar) = kinds.get(&face).copied().unwrap_or((Vec3::Z, false));
        let (p5, p50, p95) = (percentile(&values, 0.05), percentile(&values, 0.5), percentile(&values, 0.95));
        let rounded = [normal.x, normal.y, normal.z].map(|v| (v * 1000.0).round() / 1000.0);
        face_rows.push(json!({
            "face": face, "pixels": values.len(), "planar": planar,
            "normal": rounded,
            "median": p50, "p5": p5, "p95": p95, "span": p95 - p5,
        }));
    }
    json!({
        "pixels": part.len(), "clipped": clipped, "crushed": crushed,
        "median": percentile(&part, 0.5), "p5": percentile(&part, 0.05), "p95": percentile(&part, 0.95),
        "faces": face_rows,
    })
}

#[test]
#[ignore = "writes the lighting review set; run explicitly"]
fn lighting_review() {
    let out = std::path::PathBuf::from(std::env::var("LIGHTING_REVIEW_OUT").unwrap_or_else(|_| "target/lighting-review".into()));
    let tag = std::env::var("LIGHTING_REVIEW_TAG").unwrap_or_else(|_| "after".into());
    let only = std::env::var("LIGHTING_REVIEW_ONLY").ok();
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    let Some((adapter, device, queue)) = super::offscreen_device() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    eprintln!("lighting review on {:?} ({})", adapter.get_info().backend, adapter.get_info().name);
    let started = std::time::Instant::now();
    // `LIGHTING_REVIEW_STUDIO=soft|contrast` draws the set in another preset (the brightness
    // comparison); the default studio otherwise.
    let preset = match std::env::var("LIGHTING_REVIEW_STUDIO").as_deref() {
        Ok("soft") => StudioPreset::Soft,
        Ok("contrast") => StudioPreset::Contrast,
        _ => StudioPreset::Product,
    };
    let studio = Arc::new(StudioEnvironment::bake(preset));
    // `LIGHTING_REVIEW_INTENSITY=<scale>`: the studio's radiance scaled (a moodier variant).
    let intensity = std::env::var("LIGHTING_REVIEW_INTENSITY").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
    eprintln!("studio baked in {:?}", started.elapsed());
    let mut lines = String::new();
    // Each frame's part pixels (straight RGB where the mask names a face), for the pairs.
    let mut parts: Vec<((&str, &str, &str), Vec<Option<[u8; 3]>>)> = Vec::new();
    let sets: [(&str, &[Material]); 4] =
        [("sample-plate", &PLATE_MATERIALS), ("sphere", &SHAPE_MATERIALS), ("cylinder", &SHAPE_MATERIALS), ("chamfered-box", &SHAPE_MATERIALS)];
    for (scene_name, materials) in sets {
        if only.as_deref().is_some_and(|o| !scene_name.contains(o)) {
            continue;
        }
        let Some((mut s, body)) = session(scene_name) else { panic!("{scene_name}: scene builds") };
        for material in materials {
            let pbr = json!({"metallic": material.metallic, "roughness": material.roughness, "dielectric_ior": 1.5});
            s.execute("appearance.assign", &json!({"bodies": [body], "color": material.color, "pbr": pbr})).unwrap_or_else(|e| panic!("{e}"));
            let Ok(scene) = solidcraft_engine::product_scene::extract(&s) else { panic!("{scene_name}: product scene") };
            let extent = scene.bounds.size().x.max(scene.bounds.size().y).max(scene.bounds.size().z);
            for (pose, yaw, pitch) in POSES {
                let mut camera = Camera { fov: 0.5, yaw, pitch, ..Default::default() };
                camera.fit(&scene.bounds);
                camera.distance *= 0.82;
                let radius = scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(camera.target) + camera.distance;
                let look = placed(ProductLook { studio: preset, intensity, ..ProductLook::default() }, &scene);
                let beauty = draw(
                    &device,
                    &queue,
                    &camera,
                    radius,
                    Shot {
                        scene: gpu_scene(&scene, false),
                        look,
                        lights: None,
                        ao_radius: (0.08 * extent) as f32,
                        clear: BACKGROUND,
                        studio: Some(studio.clone()),
                    },
                );
                let ambient = [
                    0.0,
                    0.0,
                    0.0,
                    4.0,
                    std::f32::consts::PI,
                    std::f32::consts::PI,
                    std::f32::consts::PI,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ];
                let mask = draw(
                    &device,
                    &queue,
                    &camera,
                    radius,
                    Shot {
                        scene: gpu_scene(&scene, true),
                        look: ProductLook { intensity: 0.0, exposure: 1.0, ..ProductLook::default() },
                        lights: Some(vec![ambient]),
                        ao_radius: 0.0,
                        clear: [0.0; 3],
                        studio: None,
                    },
                );
                let (Some(beauty), Some(mask)) = (beauty, mask) else { panic!("{scene_name}: frames drawn") };
                let name = format!("{tag}-{scene_name}-{}-{pose}", material.name);
                let path = out.join(format!("{name}.png"));
                image::save_buffer(&path, &beauty, SIZE, SIZE, image::ColorType::Rgba8).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                let m = metrics(&beauty, &mask, &scene);
                let digits = digit_bytes();
                let part = beauty.chunks(4).zip(mask.chunks(4)).map(|(b, k)| decode(k, &digits).map(|_| [b[0], b[1], b[2]])).collect();
                parts.push(((scene_name, material.name, pose), part));
                eprintln!("{name}: clipped {:.4} crushed {:.4} median {}", m["clipped"], m["crushed"], m["median"]);
                let row =
                    json!({"tag": tag, "scene": scene_name, "material": material.name, "pose": pose, "file": format!("{name}.png"), "metrics": m});
                lines.push_str(&row.to_string());
                lines.push('\n');
            }
        }
    }
    let path = out.join(format!("{tag}-metrics.jsonl"));
    std::fs::write(&path, lines).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // Materials distinguishable (§5.9): the mean colour difference over the pixels both frames'
    // parts cover, for materials the bar compares.
    let mut pairs = String::new();
    for (scene_name, a, b) in [
        ("sample-plate", "polished-metal", "rough-metal"),
        ("sample-plate", "white-plastic", "black-plastic"),
        ("sphere", "chrome", "gold"),
        ("sphere", "matte-grey", "glossy-red"),
    ] {
        for (pose, _, _) in POSES {
            let find = |m: &str| parts.iter().find(|((s, mm, p), _)| *s == scene_name && *mm == m && *p == pose).map(|(_, px)| px);
            let (Some(pa), Some(pb)) = (find(a), find(b)) else { continue };
            let (mut sum, mut count) = (0.0, 0usize);
            for (x, y) in pa.iter().zip(pb) {
                if let (Some(x), Some(y)) = (x, y) {
                    sum += x.iter().zip(y).map(|(p, q)| f64::from(p.abs_diff(*q))).sum::<f64>() / 3.0;
                    count += 1;
                }
            }
            let row = json!({"scene": scene_name, "pose": pose, "a": a, "b": b, "pixels": count, "mean_difference": sum / count.max(1) as f64});
            pairs.push_str(&row.to_string());
            pairs.push('\n');
        }
    }
    let path = out.join(format!("{tag}-pairs.jsonl"));
    std::fs::write(&path, pairs).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
