//! Golden frames of a saved design and their acceptance metrics (docs/material-graph.md §5.9):
//! every saved view of `$GOLDEN_DESIGN` (e.g. the realistic showcase built with
//! `solidcraft-cli run examples/realistic-showcase.json --save …`) drawn offscreen through the
//! viewport callback as the window draws it, in the modelling view and the Render preview, in
//! the Standard and the Realistic look. Evidence, not a check (`--ignored`):
//!
//! ```sh
//! GOLDEN_DESIGN=<file> GOLDEN_OUT=<dir> [GOLDEN_TAG=product] [GOLDEN_STUDIO=product|soft|contrast] \
//!     [GOLDEN_SIZE=1600x1000] [GOLDEN_SSAA=2] \
//!     cargo test --release -p solidcraft-ui-egui --lib golden_frames -- --ignored --nocapture
//! ```
//!
//! It writes `<tag>-<view>-<surface>-<look>.png` (surface `model` or `preview`) and one JSON
//! line per frame to `<dir>/<tag>-metrics.jsonl`. A mask pass names each pixel's body and B-rep
//! face: the product scene's faces in code colours (as the lighting review's mask), drawn with
//! the same camera without supersampling. Metrics are measured on the 8-bit frames only, on face
//! pixels at least two pixels from any other face or the background, so the modelling view's
//! edge lines and silhouettes are left out:
//!
//! - per part: the clipped (luma ≥ 254) and crushed (≤ 5) shares, median, 5th and 95th
//!   percentiles (cavities are not excluded: the occlusion term is not read back);
//! - per face of at least 400 pixels: median, 5th and 95th percentiles, their span, whether the
//!   face is planar, its mean normal.
//!
//! Failures of the §5.9 bar on the Realistic frames are printed (`bar: …`); with `GOLDEN_CHECK=1`
//! the test fails on any, which makes it the acceptance test. The bar's "materials
//! distinguishable" and "WebGL2 matches native" rows need other frames and are not checked here.
//! `GOLDEN_MASKS=1` also writes each mask (`…-mask.png`). `GOLDEN_STUDIO` replaces the look's studio preset and `GOLDEN_INTENSITY` its radiance scale (for
//! the brightness comparison); the default keeps the view's own.

use solidcraft_engine::geom::prelude::*;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Session;
use solidcraft_engine::render::ProductScene;
use solidcraft_engine::render::surface::OpaqueSurface;

use super::build_scene;
use crate::gpu::{Frame, GpuScene, GpuTarget, ProductLook, Renderer, StudioEnvironment, StudioPreset, ViewportCallback};
use crate::{Services, SolveApp};
use egui_wgpu::wgpu;

/// The light theme's viewport background (sRGB), the clear colour of every beauty frame.
const BACKGROUND: [u8; 3] = [236, 240, 245];

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// One mask digit's linear level: 0.1–0.7, below PBR Neutral's shoulder (shifted by 0.04 only),
/// so each survives as a distinct 8-bit value (as the lighting review's mask).
fn level(digit: u32) -> f64 {
    0.1 + 0.1 * f64::from(digit.min(6))
}

fn display(linear: f64) -> f64 {
    let c = linear.clamp(0.0, 1.0);
    if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn digit_bytes() -> [f64; 7] {
    std::array::from_fn(|d| 255.0 * display(level(d as u32) - 0.04))
}

fn code_colour(code: u32) -> Vec3 {
    Vec3::new(level(code % 7), level((code / 7) % 7), level((code / 49) % 7))
}

fn decode(pixel: &[u8], digits: &[f64; 7]) -> Option<u32> {
    let (mut code, mut scale) = (0, 1);
    for &v in pixel.iter().take(3) {
        let (d, error) = digits.iter().enumerate().map(|(d, b)| (d as u32, (f64::from(v) - b).abs())).min_by(|a, b| a.1.total_cmp(&b.1))?;
        if error > 3.5 {
            return None;
        }
        code += d * scale;
        scale *= 7;
    }
    Some(code)
}

/// A face of the product scene: its body's name, the face id, mean normal, whether it is planar,
/// its width (twice its area over its perimeter) over the scene's extent B (the studio's room), and the
/// metalness and base-colour luminance of the body's (or face's) PBR surface if it has one.
struct FaceInfo {
    part: String,
    face: u32,
    normal: Vec3,
    planar: bool,
    width: f64,
    metallic: Option<f64>,
    albedo: Option<f64>,
}

/// The faces of `scene`, numbered 1.. (0 is the background); at most 342 (three base-7 digits).
fn faces(scene: &ProductScene) -> Vec<FaceInfo> {
    use crate::gpu::realistic_bar::{luminance, width};
    let size = scene.bounds.size();
    let extent = size.x.max(size.y).max(size.z).max(1e-9);
    let mut out: Vec<FaceInfo> = Vec::new();
    for body in &scene.meshes {
        let mut seen: std::collections::BTreeMap<u32, (Vec<Vec3>, Vec<Vec3>)> = Default::default();
        for (index, triangle) in body.mesh.triangles.iter().enumerate() {
            let face = body.mesh.tri_face.get(index).copied().unwrap_or(0);
            let entry = seen.entry(face).or_default();
            for i in triangle {
                if let (Some(n), Some(p)) = (body.mesh.normals.get(*i as usize), body.mesh.positions.get(*i as usize)) {
                    entry.0.push(n.normalized().unwrap_or(Vec3::Z));
                    entry.1.push(*p);
                }
            }
        }
        for (face, (ns, ps)) in seen {
            let mean = ns.iter().fold(Vec3::ZERO, |a, n| a + *n).normalized().unwrap_or(Vec3::Z);
            let planar = ns.iter().all(|n| n.dot(mean) > 0.9995);
            let surface = body.faces.iter().find(|f| f.face == face).and_then(|f| f.surface).or(body.surface);
            out.push(FaceInfo {
                part: body.name.clone(),
                face,
                normal: mean,
                planar,
                width: width(&ps) / extent,
                metallic: surface.map(|s| s.metallic()),
                albedo: surface.map(|s| luminance(s.base_color())),
            });
        }
    }
    out
}

/// The mask scene: each face of `scene` as a lossless diffuse in its code colour.
fn mask_scene(scene: &ProductScene, infos: &[FaceInfo]) -> GpuScene {
    let mut gpu = GpuScene::default();
    for body in &scene.meshes {
        for (index, triangle) in body.mesh.triangles.iter().enumerate() {
            let face = body.mesh.tri_face.get(index).copied().unwrap_or(0);
            let Some(code) = infos.iter().position(|f| f.part == body.name && f.face == face) else { continue };
            let Ok(surface) = OpaqueSurface::new(code_colour(code as u32 + 1), 0.0, 1.0, 1.0) else { continue };
            for i in triangle {
                let (Some(p), Some(n)) = (body.mesh.positions.get(*i as usize), body.mesh.normals.get(*i as usize)) else { continue };
                gpu.pbr_tri(p.to_f32(), n.normalized().unwrap_or(Vec3::Z).to_f32(), surface);
            }
        }
    }
    gpu
}

struct Target {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: [u32; 2],
}

/// Draw `frame` offscreen through the viewport callback into a `format` target, frame after
/// frame until no material shape or texture is still pending (at most 120): straight RGBA.
fn draw(t: &Target, format: wgpu::TextureFormat, studio: Option<StudioPreset>, clear: wgpu::Color, frame: impl Fn() -> Frame) -> Option<Vec<u8>> {
    let (device, queue, [w, h]) = (&t.device, &t.queue, t.size);
    let depth = solidcraft_gpu::depth_format(device, true);
    let mut renderer = Renderer::new(device, GpuTarget { format, depth: Some(depth), samples: 1, tess: false, oit: false }, None);
    if let Some(preset) = studio {
        renderer.set_studio_environment(device, queue, &StudioEnvironment::bake(preset));
    }
    let mut resources = egui_wgpu::CallbackResources::default();
    resources.insert(renderer);
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("golden"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth_tex = texture(depth, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth_tex.create_view(&Default::default()));
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: 1.0 };
    let reversed = solidcraft_gpu::view::reversed_depth(Some(depth));
    for n in 0..120u32 {
        let cb = ViewportCallback(frame());
        let mut encoder = device.create_command_encoder(&Default::default());
        let extra = egui_wgpu::CallbackTrait::prepare(&cb, device, queue, &screen, &mut encoder, &mut resources);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("golden"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(if reversed { 0.0 } else { 1.0 }),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
            let info = egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [w, h] };
            egui_wgpu::CallbackTrait::paint(&cb, info, &mut pass, &resources);
        }
        let done = resources.get::<Renderer>().is_none_or(|r| !r.wants_redraw());
        if (!done || n == 0) && n < 119 {
            queue.submit(extra.into_iter().chain([encoder.finish()]));
            device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("golden read"),
            size: u64::from(w * h * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(extra.into_iter().chain([encoder.finish()]));
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let bytes = slice.get_mapped_range().ok()?.to_vec();
        if n == 119 {
            eprintln!("warning: still pending after 120 frames");
        }
        return Some(bytes);
    }
    None
}

/// A frame of `scene` with the window's fixed fields.
#[allow(clippy::too_many_arguments)]
fn frame(
    slot: &crate::gpu::SceneSlot,
    look: Option<ProductLook>,
    lights: Option<Vec<[f32; 16]>>,
    product: bool,
    view_proj: [[f32; 4]; 4],
    back: [f32; 3],
    eye: [f32; 4],
    size: [u32; 2],
    ao_radius: f32,
    ssaa: f32,
    outline: bool,
) -> Frame {
    let none = || Arc::new(Mutex::new(None));
    Frame {
        moving: None,
        preview_bounds: None,
        hatch: Default::default(),
        product_lights: lights,
        product_look: look,
        units_per_mm: 1.0,
        product,
        upload_error: None,
        key: 1,
        slot: slot.clone(),
        sketch_key: 0,
        sketch_slot: none(),
        hl_key: 0,
        hl_slot: none(),
        pv_key: 0,
        pv_slot: none(),
        pv_hide: false,
        view_proj,
        back,
        size_px: [size[0] as f32, size[1] as f32],
        clip: [0.0; 4],
        cap: [0.0; 4],
        images: Vec::new(),
        clouds: Vec::new(),
        analysis: [0.0; 8],
        analysis_colors: [0.0; 36],
        access: None,
        outline,
        eye,
        ao_radius,
        ao_samples: solidcraft_gpu::view::AO_SAMPLES,
        ssaa,
        sky: [0.0; 4],
        hover_tint: [0.0; 3],
        selection_tint: [0.0; 3],
    }
}

fn luma(p: &[u8]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[(((sorted.len() - 1) as f64 * q).round() as usize).min(sorted.len() - 1)]
}

fn summary(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let n = values.len().max(1) as f64;
    json!({
        "pixels": values.len(),
        "clipped": values.iter().filter(|v| **v >= 254.0).count() as f64 / n,
        "crushed": values.iter().filter(|v| **v <= 5.0).count() as f64 / n,
        "median": percentile(&values, 0.5), "p5": percentile(&values, 0.05), "p95": percentile(&values, 0.95), "p99": percentile(&values, 0.99),
        "span": percentile(&values, 0.95) - percentile(&values, 0.05),
    })
}

/// The frame's metrics: per part and per face, on face pixels two pixels clear of any other face
/// or the background.
fn metrics(beauty: &[u8], mask: &[u8], [w, h]: [u32; 2], infos: &[FaceInfo]) -> Value {
    let digits = digit_bytes();
    let (w, h) = (w as usize, h as usize);
    let ids: Vec<Option<u32>> = mask.chunks(4).map(|p| decode(p, &digits).filter(|c| *c > 0)).collect();
    let mut per_face: std::collections::BTreeMap<u32, Vec<f64>> = Default::default();
    let mut per_part_rgb: std::collections::BTreeMap<String, [f64; 4]> = Default::default();
    const R: usize = 2;
    for y in R..h.saturating_sub(R) {
        for x in R..w.saturating_sub(R) {
            let i = y * w + x;
            let Some(code) = ids[i] else { continue };
            let clear = (y - R..=y + R).all(|yy| (x - R..=x + R).all(|xx| ids[yy * w + xx] == Some(code)));
            if !clear {
                continue;
            }
            let p = &beauty[4 * i..4 * i + 4];
            per_face.entry(code).or_default().push(luma(p));
            if let Some(info) = infos.get(code as usize - 1) {
                let e = per_part_rgb.entry(info.part.clone()).or_default();
                for k in 0..3 {
                    e[k] += f64::from(p[k]);
                }
                e[3] += 1.0;
            }
        }
    }
    let mut parts: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
    let mut face_rows = Vec::new();
    for (code, values) in &per_face {
        let Some(info) = infos.get(*code as usize - 1) else { continue };
        parts.entry(info.part.clone()).or_default().extend(values);
        if values.len() < 400 {
            continue;
        }
        let mut row = summary(values.clone());
        let n = [info.normal.x, info.normal.y, info.normal.z].map(|v| (v * 1000.0).round() / 1000.0);
        if let Some(o) = row.as_object_mut() {
            o.insert("part".into(), json!(info.part));
            o.insert("face".into(), json!(info.face));
            o.insert("planar".into(), json!(info.planar));
            o.insert("normal".into(), json!(n));
            o.insert("metallic".into(), json!(info.metallic));
            o.insert("albedo".into(), json!(info.albedo));
            o.insert("width".into(), json!(info.width));
        }
        face_rows.push(row);
    }
    let part_rows: Vec<Value> = parts
        .into_iter()
        .map(|(part, values)| {
            let mut row = summary(values);
            let rgb = per_part_rgb.get(&part).map(|e| [e[0], e[1], e[2]].map(|c| c / e[3].max(1.0)));
            if let Some(o) = row.as_object_mut() {
                o.insert("part".into(), json!(part));
                o.insert("mean_rgb".into(), json!(rgb));
            }
            row
        })
        .collect();
    json!({"parts": part_rows, "faces": face_rows})
}

/// The acceptance bar of §5.9 on one frame's metrics (`gpu::realistic_bar`, shared with the
/// lighting review): each failure as a line.
fn bar_failures(frame: &str, m: &Value) -> Vec<String> {
    crate::gpu::realistic_bar::failures(m)
        .iter()
        .map(|f| {
            format!("{frame}: {} {} {}", f["part"].as_str().unwrap_or("?"), f["check"].as_str().unwrap_or("?"), f["detail"].as_str().unwrap_or(""))
        })
        .collect()
}

fn slug(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

fn linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[test]
#[ignore = "writes golden frames of a saved design; run explicitly"]
fn golden_frames() {
    let (Some(design), Some(out)) = (env("GOLDEN_DESIGN"), env("GOLDEN_OUT")) else { return };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    let tag = env("GOLDEN_TAG").unwrap_or_else(|| "golden".into());
    let size: [u32; 2] =
        env("GOLDEN_SIZE").and_then(|s| s.split_once('x').and_then(|(a, b)| Some([a.parse().ok()?, b.parse().ok()?]))).unwrap_or([1600, 1000]);
    let ssaa: f32 = env("GOLDEN_SSAA").and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let studio = env("GOLDEN_STUDIO").map(|s| match s.as_str() {
        "soft" => StudioPreset::Soft,
        "contrast" => StudioPreset::Contrast,
        "product" => StudioPreset::Product,
        _ => StudioPreset::default(),
    });
    let intensity: Option<f32> = env("GOLDEN_INTENSITY").and_then(|s| s.parse().ok());
    let Some((adapter, device, queue)) = crate::gpu::offscreen_device() else { panic!("no GPU adapter") };
    eprintln!(
        "golden frames on {:?} ({}), {}×{}, ssaa {ssaa}, studio {studio:?}",
        adapter.get_info().backend,
        adapter.get_info().name,
        size[0],
        size[1]
    );
    let target = Target { device, queue, size };
    let mut app = SolveApp::new(Session::default(), Services::default());
    app.run("doc.open", json!({"path": design})).unwrap_or_else(|e| panic!("{e}"));
    app.viewport.gpu = Some(GpuTarget { format: wgpu::TextureFormat::Rgba8UnormSrgb, depth: None, samples: 1, tess: false, oit: false });
    let views: Vec<String> = match env("GOLDEN_VIEWS") {
        Some(v) => v.split(',').map(str::to_string).collect(),
        None => app.session.doc.named_views.iter().map(|v| v.name.clone()).collect(),
    };
    let clear = {
        let [r, g, b] = BACKGROUND.map(linear);
        wgpu::Color { r, g, b, a: 1.0 }
    };
    let mask_clear = wgpu::Color::BLACK;
    let ambient = [0.0, 0.0, 0.0, 4.0, std::f32::consts::PI, std::f32::consts::PI, std::f32::consts::PI, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mask_look = ProductLook { intensity: 0.0, exposure: 1.0, ..ProductLook::default() };
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
    let mut lines = String::new();
    let mut failures = Vec::new();
    for view in &views {
        app.run("view.apply", json!({"view": view})).unwrap_or_else(|e| panic!("{view}: {e}"));
        // The window moves to the camera `view.apply` asks for on its next frame; at once here.
        app.follow_view_request(false);
        let view_slug = slug(view.trim_start_matches("Showcase:"));
        for (look_name, look) in [("standard", crate::view_look::STANDARD), ("realistic", crate::view_look::REALISTIC)] {
            app.ui.view_look = look;
            app.ui.render_preview_look = look;
            // The modelling view, as `viewport::show` draws it.
            let proj = super::projection(&app, rect);
            let e = proj.cam.eye().to_f32();
            let eye = [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }];
            let product = crate::view_look::product_look_around(&app, "model", &super::shown_body_bounds(&app)).map(|l| ProductLook {
                studio: studio.unwrap_or(l.studio),
                intensity: intensity.unwrap_or(l.intensity),
                ..l
            });
            let slot: crate::gpu::SceneSlot = Arc::new(Mutex::new(Some(build_scene(&app).merged())));
            let outline = matches!(app.ui.visual_style, 0 | 3);
            let (vp, back) = (proj.vp.to_f32(), proj.cam.back().to_f32());
            let ao = super::ao_radius(&app);
            let model = draw(&target, wgpu::TextureFormat::Rgba8UnormSrgb, product.map(|l| l.studio), clear, || {
                frame(&slot, product, None, false, vp, back, eye, size, ao, ssaa, outline)
            });
            // The Render preview, as `render_workspace::paint_product` draws it (no supersampling
            // and no occlusion there).
            let (scene, gpu, preview_lights) = crate::render_workspace::preview_scene_for_evidence(&app).unwrap_or_else(|e| panic!("{view}: {e}"));
            let camera = app.cam;
            let radius = scene.bounds.diagonal().max(10.0) + scene.bounds.center().dist(camera.target);
            let pvp = camera.view_proj(f64::from(size[0]) / f64::from(size[1]), radius).to_f32();
            let ce = camera.eye().to_f32();
            let peye = [ce[0], ce[1], ce[2], if camera.fov > 0.0 { 1.0 } else { 0.0 }];
            let preview_look = crate::view_look::product_look_around(&app, "render_preview", &scene.bounds).map(|l| ProductLook {
                studio: studio.unwrap_or(l.studio),
                intensity: intensity.unwrap_or(l.intensity),
                ..l
            });
            let pslot: crate::gpu::SceneSlot = Arc::new(Mutex::new(Some(gpu)));
            let preview = draw(&target, wgpu::TextureFormat::Rgba8UnormSrgb, preview_look.map(|l| l.studio), clear, || {
                frame(&pslot, preview_look, Some(preview_lights.clone()), true, pvp, camera.back().to_f32(), peye, size, 0.0, 1.0, false)
            });
            // The masks, one per camera (the modelling view's projection is the window's).
            let infos = faces(&scene);
            assert!(infos.len() < 343, "{} faces: more than the mask's three digits name", infos.len());
            // The renderer takes a scene out of its slot when it uploads it: one slot per draw.
            let mask_of = |vp, back, eye| {
                let mslot: crate::gpu::SceneSlot = Arc::new(Mutex::new(Some(mask_scene(&scene, &infos))));
                draw(&target, wgpu::TextureFormat::Rgba8Unorm, None, mask_clear, || {
                    frame(&mslot, Some(mask_look), Some(vec![ambient]), true, vp, back, eye, size, 0.0, 1.0, false)
                })
            };
            let model_mask = mask_of(vp, back, eye);
            let preview_mask = mask_of(pvp, camera.back().to_f32(), peye);
            for (surface, beauty, mask) in [("model", model, model_mask), ("preview", preview, preview_mask)] {
                let (Some(beauty), Some(mask)) = (beauty, mask) else { panic!("{view} {surface} {look_name}: frames drawn") };
                let name = format!("{tag}-{view_slug}-{surface}-{look_name}");
                let path = out.join(format!("{name}.png"));
                image::save_buffer(&path, &beauty, size[0], size[1], image::ColorType::Rgba8).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                if env("GOLDEN_MASKS").is_some() {
                    let path = out.join(format!("{name}-mask.png"));
                    image::save_buffer(&path, &mask, size[0], size[1], image::ColorType::Rgba8).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                }
                let m = metrics(&beauty, &mask, size, &infos);
                let row = json!({"tag": tag, "view": view, "surface": surface, "look": look_name, "studio": studio.map(|s| format!("{s:?}")), "intensity": intensity, "file": format!("{name}.png"), "metrics": m});
                eprintln!("{name}: {} parts", m["parts"].as_array().map_or(0, Vec::len));
                if look_name == "realistic" {
                    failures.extend(bar_failures(&name, &m));
                }
                lines.push_str(&row.to_string());
                lines.push('\n');
            }
        }
    }
    let path = out.join(format!("{tag}-metrics.jsonl"));
    std::fs::write(&path, lines).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for line in &failures {
        eprintln!("bar: {line}");
    }
    // `GOLDEN_CHECK=1`: the Realistic frames must meet the bar (the acceptance test).
    if env("GOLDEN_CHECK").is_some() {
        assert!(failures.is_empty(), "{} failures of the §5.9 bar (listed above)", failures.len());
    }
}
