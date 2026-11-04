//! Per-frame GPU cost of the modelling view in both looks (docs/material-graph.md §5.4), on a
//! saved design (`$PERF_DESIGN`, e.g. the realistic showcase built with `solidcraft-cli run
//! examples/realistic-showcase.json --save …`). A record, not a check (`--ignored`); run
//! optimised:
//!
//! ```sh
//! PERF_DESIGN=<file> cargo test --release -p solidcraft-ui-egui --lib viewport_frame_costs -- --ignored --nocapture
//! ```
//!
//! The model scene is the window's (`build_scene`, the viewport's look, occlusion radius and
//! edges); the frames go through the renderer's `prepare` and `paint` into 1920 × 1080 targets
//! made once, timed from `prepare` to the end of the GPU wait, the camera nudged every frame so
//! no picture is reused.

use std::sync::{Arc, Mutex};

use serde_json::json;
use solidcraft_engine::Session;

use super::build_scene;
use crate::gpu::{Frame, Renderer, StudioEnvironment};
use crate::{Services, SolveApp};
use egui_wgpu::wgpu;

const SIZE: [u32; 2] = [1920, 1080];

fn spread(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v.first().copied().unwrap_or(f64::NAN), v.get(v.len() / 2).copied().unwrap_or(f64::NAN))
}

fn load() -> String {
    std::process::Command::new("uptime").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

#[test]
#[ignore = "a measurement; run optimised with --nocapture"]
fn viewport_frame_costs() {
    let Ok(design) = std::env::var("PERF_DESIGN") else { return };
    let Some((adapter, device, queue)) = crate::gpu::offscreen_device() else { panic!("no GPU") };
    println!("adapter: {:?}; load before: {}", adapter.get_info().name, load());
    let mut app = SolveApp::new(Session::default(), Services::default());
    app.run("doc.open", json!({"path": design})).unwrap_or_else(|e| panic!("{e}"));
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let depth = solidcraft_gpu::depth_format(&device, true);
    app.viewport.gpu = Some(crate::gpu::GpuTarget { format, depth: None, samples: 1, tess: false, oit: false });
    let target = crate::gpu::GpuTarget { format, depth: Some(depth), samples: 1, tess: false, oit: false };
    let texture = |format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("perf"),
                size: wgpu::Extent3d { width: SIZE[0], height: SIZE[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let color = texture(format, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let depth_view = texture(depth, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let reversed = solidcraft_gpu::view::reversed_depth(Some(depth));
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE[0] as f32, SIZE[1] as f32));
    for view in ["Showcase: isometric front-left", "Showcase: low back three-quarter"] {
        app.run("view.apply", json!({"view": view})).unwrap_or_else(|e| panic!("{e}"));
        // The window moves to the camera `view.apply` asks for on its next frame; at once here.
        app.follow_view_request(false);
        let base_cam = app.cam;
        for ssaa in [1.0f32, 2.0] {
            for (look_name, look) in [("standard", crate::view_look::STANDARD), ("realistic", crate::view_look::REALISTIC)] {
                app.ui.view_look = look;
                let product = crate::view_look::product_look_around(&app, "model", &super::shown_body_bounds(&app));
                let mut renderer = Renderer::new(&device, target, None);
                if let Some(look) = product {
                    renderer.set_studio_environment(&device, &queue, &StudioEnvironment::bake(look.studio));
                }
                let t = web_time::Instant::now();
                let scene = build_scene(&app).merged();
                let build_ms = t.elapsed().as_secs_f64() * 1e3;
                let draws = scene.graphs.draws().len();
                let slot = Arc::new(Mutex::new(Some(scene)));
                let none = || Arc::new(Mutex::new(None));
                let mut times = Vec::new();
                let mut stats = None;
                for step in 0..90u32 {
                    app.cam = crate::Camera { yaw: base_cam.yaw + 1e-4 * f64::from(step % 9), ..base_cam };
                    let proj = super::projection(&app, rect);
                    let e = proj.cam.eye().to_f32();
                    let frame = Frame {
                        product_lights: None,
                        product_look: product,
                        units_per_mm: 1.0,
                        product: false,
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
                        view_proj: proj.vp.to_f32(),
                        back: proj.cam.back().to_f32(),
                        size_px: [SIZE[0] as f32, SIZE[1] as f32],
                        clip: [0.0; 4],
                        cap: [0.0; 4],
                        images: Vec::new(),
                        clouds: Vec::new(),
                        analysis: [0.0; 8],
                        analysis_colors: [0.0; 36],
                        access: None,
                        outline: matches!(app.ui.visual_style, 0 | 3),
                        eye: [e[0], e[1], e[2], if proj.cam.fov > 0.0 { 1.0 } else { 0.0 }],
                        ao_radius: super::ao_radius(&app),
                        ao_samples: solidcraft_gpu::view::AO_SAMPLES,
                        ssaa,
                        sky: [0.0; 4],
                        hover_tint: [0.0; 3],
                        selection_tint: [0.0; 3],
                    };
                    let started = web_time::Instant::now();
                    let mut encoder = device.create_command_encoder(&Default::default());
                    renderer.prepare(&device, &queue, &mut encoder, &frame);
                    {
                        let mut pass = encoder
                            .begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("perf"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &color,
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
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
                        renderer.paint(&frame, &mut pass, [0.0, 0.0, SIZE[0] as f32, SIZE[1] as f32]);
                    }
                    queue.submit([encoder.finish()]);
                    device.poll(wgpu::PollType::wait_indefinitely()).ok();
                    let ms = started.elapsed().as_secs_f64() * 1e3;
                    // Warm-up: the first frames upload the scene and compile the shapes.
                    if step >= 30 && !renderer.wants_redraw() {
                        times.push(ms);
                    }
                    stats = Some(renderer.material_stats());
                }
                let n = times.len();
                let (min, median) = spread(times);
                let drawn = renderer.stats().lock().map(|s| (s.draw_calls, s.triangles)).unwrap_or_default();
                println!(
                    "{view} ssaa {ssaa} {look_name}: frame min {min:.2} ms, median {median:.2} ms over {n} frames; scene build {build_ms:.1} ms; \
                     {draws} graph draws, draw calls {} ({} triangles); material stats {stats:?}",
                    drawn.0, drawn.1
                );
            }
        }
        app.cam = base_cam;
    }
    println!("load after: {}", load());
}
