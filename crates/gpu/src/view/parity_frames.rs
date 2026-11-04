// Standard-look parity frames: a fixed set of small viewport frames that must stay byte for byte
// what the renderer drew before the material-graph package (docs/material-graph.md §6.3).
//
// The set only uses API that upstream's renderer already had (no product look, no graphs), so
// `cargo xtask render-parity` can copy this file into an older revision, dump the same frames
// there and compare. It is a child of `glass_tests` so it can use that module's frame helpers
// in every revision. Explicit only:
//
// ```sh
// PARITY_OUT=<dir> cargo test -p solidcraft-gpu --lib parity_frames -- --ignored
// ```
//
// Each frame is written as `<dir>/std-<name>.rgba` (64 × 64, straight RGBA8).

use super::*;

pub(crate) const SIZE: u32 = 64;

/// Draw `frame` into a fresh `format` target with depth: `SIZE` × `SIZE` RGBA bytes.
pub(crate) fn draw(r: &mut Renderer, device: &wgpu::Device, queue: &wgpu::Queue, frame: &Frame, format: wgpu::TextureFormat) -> Vec<u8> {
    let depth = crate::depth_format(device, true);
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("parity frame"),
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let color_view = color.create_view(&Default::default());
    let depth_tex = texture(depth, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let depth_view = depth_tex.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    r.prepare(device, queue, &mut encoder, frame);
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("parity frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(far_depth(reversed_depth(Some(depth)))), store: wgpu::StoreOp::Store }),
                    stencil_ops: depth.has_stencil_aspect().then_some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        r.paint(frame, &mut pass, [0.0, 0.0, SIZE as f32, SIZE as f32]);
    }
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity read"),
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: Some(SIZE) } },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    read.slice(..).get_mapped_range().map(|b| b.to_vec()).unwrap_or_default()
}

/// The PBR surfaces of the set: a dielectric, copper, a green dielectric at IOR 1, a polished
/// white dielectric and a rough metal.
pub(crate) const SURFACES: [([f64; 3], f64, f64, f64); 5] = [
    ([0.8, 0.5, 0.2], 0.0, 0.5, 1.5),
    ([0.95, 0.64, 0.54], 1.0, 0.3, 1.5),
    ([0.1, 0.45, 0.18], 0.0, 0.25, 1.0),
    ([0.9, 0.9, 0.92], 0.0, 0.045, 1.5),
    ([0.7, 0.7, 0.75], 1.0, 0.8, 2.5),
];

/// Product-light records: a key, an ambient, a point and a spot.
pub(crate) const KEY: [f32; 16] = [0.3, -0.2, 0.93, 0.0, 2.0, 1.8, 1.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
pub(crate) const AMBIENT: [f32; 16] = [0.0, 0.0, 0.0, 4.0, 0.2, 0.25, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
pub(crate) const POINT: [f32; 16] = [6.0, 4.0, 14.0, 1.0, 300.0, 280.0, 260.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.1, 0.0, 0.0];
pub(crate) const SPOT: [f32; 16] = [-6.0, 0.0, 20.0, 2.0, 400.0, 400.0, 400.0, 0.9, 0.3, 0.0, -1.0, 0.97, 1.0, 0.1, 0.0, 0.0];

pub(crate) fn lightings() -> [(&'static str, Option<Vec<[f32; 16]>>); 3] {
    [("fixed", None), ("key", Some(vec![KEY])), ("mixed", Some(vec![KEY, AMBIENT, POINT, SPOT]))]
}

/// Occlusion, supersampling and eye variants: plain, occlusion in perspective, supersampled.
pub(crate) const VARIANTS: [(&str, f32, f32, [f32; 4]); 3] =
    [("o", 0.0, 1.0, [0.0, 0.0, 100.0, 0.0]), ("ao", 4.0, 1.0, [3.0, -2.0, 40.0, 1.0]), ("ss", 4.0, 2.0, [0.0, 0.0, 100.0, 0.0])];

pub(crate) const FORMATS: [wgpu::TextureFormat; 2] = [wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureFormat::Rgba8UnormSrgb];

/// The PBR scenes of `surface`: a square over the view, a dome on a floor with a scalar
/// triangle, and a block on a floor.
pub(crate) fn pbr_scenes(surface: solidcraft_render::surface::OpaqueSurface) -> [(&'static str, GpuScene); 3] {
    let mut square = GpuScene::default();
    let quad = |scene: &mut GpuScene, z: f32, h: f32| {
        for [x, y] in [[-h, -h], [h, -h], [h, h], [-h, -h], [h, h], [-h, h]] {
            scene.pbr_tri([x, y, z], [0.0, 0.0, 1.0], surface);
        }
    };
    quad(&mut square, 0.0, 12.0);
    let mut dome = GpuScene::default();
    let (rings, segs) = (12usize, 24usize);
    let at = |i: usize, j: usize| {
        let theta = (i as f32 / rings as f32) * std::f32::consts::FRAC_PI_2;
        let phi = (j as f32 / segs as f32) * std::f32::consts::TAU;
        let n = [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()];
        ([n[0] * 9.0, n[1] * 9.0, n[2] * 9.0], n)
    };
    for i in 0..rings {
        for j in 0..segs {
            for (p, n) in [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j), at(i + 1, j + 1), at(i, j + 1)] {
                dome.pbr_tri(p, n, surface);
            }
        }
    }
    quad(&mut dome, 0.0, 12.0);
    let orange = dome.color([180, 90, 40, 255]);
    for p in [[-11.0, -11.0, 0.5], [-6.0, -11.0, 0.5], [-6.0, -6.0, 0.5]] {
        dome.tri(p, [0.0, 0.3, 0.95], orange);
    }
    let mut block = GpuScene::default();
    quad(&mut block, 0.0, 12.0);
    quad(&mut block, 3.0, 2.0);
    [("square", square), ("dome", dome), ("block", block)]
}

/// A scalar scene: a floor, three tilted squares (one near black), edges, a hidden line and a
/// line drawn on top.
pub(crate) fn scalar_scene(tilt: f32) -> GpuScene {
    let mut scene = GpuScene::default();
    let colours = [[150u8, 150, 156, 255], [180, 90, 40, 255], [30, 200, 90, 255], [12, 12, 14, 255]];
    let idx: Vec<_> = colours.iter().map(|c| scene.color(*c)).collect();
    let n = |x: f32, y: f32| {
        let l = (x * x + y * y + 1.0).sqrt();
        [x / l, y / l, 1.0 / l]
    };
    let quad = |scene: &mut GpuScene, [x0, y0, x1, y1]: [f32; 4], z: f32, nn: [f32; 3], c| {
        for [x, y] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
            scene.tri([x, y, z], nn, c);
        }
    };
    quad(&mut scene, [-12.0, -12.0, 12.0, 12.0], 0.0, [0.0, 0.0, 1.0], idx[0]);
    quad(&mut scene, [-6.0, -6.0, 0.0, 0.0], 2.0, n(tilt, 0.3), idx[1]);
    quad(&mut scene, [1.0, 1.0, 7.0, 7.0], 3.0, n(-0.5, tilt), idx[2]);
    quad(&mut scene, [1.0, -7.0, 7.0, -1.0], 1.0, n(0.2, -0.2), idx[3]);
    let e = scene.color([20, 24, 30, 255]);
    scene.line([-12.0, 0.5, 0.1], [12.0, 0.5, 0.1], e, 2.0, false);
    scene.hidden_line([-3.0, -12.0, 0.1], [-3.0, 12.0, 0.1], e, 1.5);
    scene.line([0.0, -12.0, 4.0], [0.0, 12.0, 4.0], e, 1.0, true);
    scene
}

/// The highlight overlay: a flat selection fill (zero normal) and a lit, half-transparent hover
/// face.
pub(crate) fn highlight() -> GpuScene {
    let mut scene = GpuScene::default();
    let a = scene.color([40, 140, 255, 255]);
    let b = scene.color([250, 180, 60, 200]);
    for [x, y] in [[-8.0f32, 3.0], [-2.0, 3.0], [-2.0, 8.0], [-8.0, 3.0], [-2.0, 8.0], [-8.0, 8.0]] {
        scene.tri([x, y, 0.0], [0.0; 3], a);
    }
    for [x, y] in [[-8.0f32, -8.0], [-2.0, -8.0], [-2.0, -3.0], [-8.0, -8.0], [-2.0, -3.0], [-8.0, -3.0]] {
        scene.tri([x, y, 0.0], [0.0, 0.0, 1.0], b);
    }
    scene
}

/// A see-through scene: a scalar floor under a glass square and a faded square.
fn see_through_scene() -> GpuScene {
    let mut scene = GpuScene::default();
    let floor = scene.color([150, 150, 156, 255]);
    let glass = scene.color([90, 160, 230, 110]);
    let faded = scene.color([230, 120, 60, 90]);
    for [x, y] in [[-12.0f32, -12.0], [12.0, -12.0], [12.0, 12.0], [-12.0, -12.0], [12.0, 12.0], [-12.0, 12.0]] {
        scene.tri([x, y, 0.0], [0.0, 0.0, 1.0], floor);
    }
    for [x, y] in [[-8.0f32, -8.0], [4.0, -8.0], [4.0, 4.0], [-8.0, -8.0], [4.0, 4.0], [-8.0, 4.0]] {
        scene.glass_tri([x, y, 3.0], [0.0, 0.0, 1.0], glass);
    }
    for [x, y] in [[-2.0f32, -2.0], [9.0, -2.0], [9.0, 9.0], [-2.0, -2.0], [9.0, 9.0], [-2.0, 9.0]] {
        scene.trans_tri([x, y, 5.0], [0.3, 0.0, 0.95], faded);
    }
    scene
}

/// Write `bytes` as `<out>/<name>.rgba`.
pub(crate) fn write(out: &std::path::Path, name: &str, bytes: &[u8]) {
    let path = out.join(format!("{name}.rgba"));
    if let Err(e) = std::fs::write(&path, bytes) {
        panic!("{}: {e}", path.display());
    }
}

/// Every Standard frame of the set (`std-…`): PBR surfaces in the default preview under the
/// fixed studio and product lights; scalar scenes through every solid, line, occlusion, sky,
/// section, analysis, outline, highlight and product-preview path; see-through surfaces.
#[test]
#[ignore = "writes the parity frames; run through `cargo xtask render-parity`"]
fn standard_parity_frames() {
    let Some(out) = std::env::var_os("PARITY_OUT").map(std::path::PathBuf::from) else { return };
    if let Err(e) = std::fs::create_dir_all(&out) {
        panic!("{}: {e}", out.display());
    }
    let Some((_, device, queue)) = crate::open_headless().ok() else { panic!("no GPU adapter") };
    let depth = crate::depth_format(&device, true);
    let mut written = 0usize;
    for format in FORMATS {
        let mut r = Renderer::new(&device, GpuTarget { format, depth: Some(depth), samples: 1, tess: false, oit: false }, None);
        let mut number = 1000u64;
        for (si, (base, metallic, roughness, ior)) in SURFACES.into_iter().enumerate() {
            let Ok(surface) = solidcraft_render::surface::OpaqueSurface::new(Vec3::new(base[0], base[1], base[2]), metallic, roughness, ior) else {
                panic!("surface {si}")
            };
            for (scene_name, scene) in pbr_scenes(surface) {
                for (light_name, lights) in lightings() {
                    for (variant, ao, ssaa, eye) in VARIANTS {
                        number += 1;
                        let frame = Frame { key: number, ao_radius: ao, eye, product_lights: lights.clone(), ..test_frame(scene.clone(), ssaa) };
                        write(&out, &format!("std-{format:?}-s{si}-{scene_name}-{light_name}-{variant}"), &draw(&mut r, &device, &queue, &frame, format));
                        written += 1;
                    }
                }
            }
        }
        let mut number = 50_000u64;
        for tilt in [0.0f32, 0.6] {
            for (variant, ao, ssaa, eye, sky, clip, outline, analysis, product, lights) in [
                ("o", 0.0, 1.0, [0.0, 0.0, 100.0, 0.0], [0.0; 4], [0.0; 4], false, [0.0; 8], false, None),
                ("ao", 4.0, 1.0, [3.0, -2.0, 40.0, 1.0], [0.0; 4], [0.0; 4], true, [0.0; 8], false, None),
                ("ss", 4.0, 2.0, [0.0, 0.0, 100.0, 0.0], [0.0; 4], [0.0; 4], true, [0.0; 8], false, None),
                ("sky", 0.0, 1.0, [0.0, 0.0, 100.0, 0.0], [0.0, 0.0, 1.0, 1.0], [0.0; 4], false, [0.0; 8], false, None),
                ("skyao", 4.0, 2.0, [3.0, -2.0, 40.0, 1.0], [0.0, 0.0, 1.0, 1.0], [0.0; 4], true, [0.0; 8], false, None),
                ("cut", 0.0, 1.0, [0.0, 0.0, 100.0, 0.0], [0.0; 4], [1.0, 0.0, 0.0, 2.0], false, [0.0; 8], false, None),
                ("zebra", 0.0, 1.0, [0.0, 0.0, 100.0, 0.0], [0.0; 4], [0.0; 4], false, [1.0, 8.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], false, None),
                ("prod", 0.0, 1.0, [0.0, 0.0, 100.0, 0.0], [0.0; 4], [0.0; 4], false, [0.0; 8], true, Some(vec![KEY, AMBIENT, POINT])),
                ("prodss", 0.0, 2.0, [0.0, 0.0, 100.0, 0.0], [0.0; 4], [0.0; 4], false, [0.0; 8], true, Some(vec![KEY])),
            ] {
                number += 1;
                let frame = Frame {
                    key: number,
                    ao_radius: ao,
                    eye,
                    sky,
                    clip,
                    cap: [0.8, 0.2, 0.2, 1.0],
                    outline,
                    analysis,
                    product,
                    product_lights: lights,
                    hl_key: number,
                    hl_slot: Arc::new(Mutex::new(Some(highlight()))),
                    hover_tint: [0.1, 0.1, 0.1],
                    selection_tint: [0.2, 0.5, 0.9],
                    ..test_frame(scalar_scene(tilt), ssaa)
                };
                write(&out, &format!("std-{format:?}-scalar-t{tilt}-{variant}"), &draw(&mut r, &device, &queue, &frame, format));
                written += 1;
            }
        }
        for (number, (variant, ao, ssaa, eye)) in (70_001u64..).zip(VARIANTS) {
            let frame = Frame { key: number, ao_radius: ao, eye, ..test_frame(see_through_scene(), ssaa) };
            write(&out, &format!("std-{format:?}-see-through-{variant}"), &draw(&mut r, &device, &queue, &frame, format));
            written += 1;
        }
    }
    eprintln!("parity: wrote {written} standard frames");
}
