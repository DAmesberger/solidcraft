//! The Realistic view of a modelling viewport: the same scenes with
//! the frame's product look. Opaque scalar faces are lit by the studio as an implicit dielectric;
//! highlights, edges and the standard picture itself stay exactly as they are.

use std::sync::{Arc, Mutex};

use super::glass_tests::{render_frame, test_frame};
use super::*;

const SIZE: usize = 64;

fn gpu() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    match crate::open_headless(crate::test_env::policy()) {
        Ok(gpu) => Some(gpu),
        Err(error) => {
            crate::test_env::skip_unless_required(crate::test_env::required(), &error);
            None
        }
    }
}

fn renderer(device: &wgpu::Device) -> Renderer {
    let depth = crate::depth_format(device, true);
    Renderer::new(
        device,
        GpuTarget { format: wgpu::TextureFormat::Rgba8Unorm, depth: Some(depth), samples: 1, tess: false, oit: false, edges: false },
        None,
    )
}

/// Two triangles covering `x0..x1` × `y0..y1` at height `z`, all with normal `n`.
fn square(scene: &mut GpuScene, [x0, y0, x1, y1]: [f32; 4], z: f32, n: [f32; 3], c: PaletteIndex) {
    for [x, y] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
        scene.tri([x, y, z], n, c);
    }
}

/// The view's pixel (`x`, `y`), RGB.
fn at(bytes: &[u8], x: usize, y: usize) -> [u8; 3] {
    let i = (y * SIZE + x) * 4;
    bytes.get(i..i + 3).map_or([0; 3], |p| [p[0], p[1], p[2]])
}

/// The view's pixels with x and y in `lo..hi`.
fn region(bytes: &[u8], lo: usize, hi: usize) -> Vec<[u8; 3]> {
    (lo..hi).flat_map(|y| (lo..hi).map(move |x| (x, y))).map(|(x, y)| at(bytes, x, y)).collect()
}

fn mean(pixels: &[[u8; 3]]) -> f64 {
    pixels.iter().map(|c| c.iter().map(|&x| f64::from(x)).sum::<f64>() / 3.0).sum::<f64>() / pixels.len().max(1) as f64
}

/// A modelling frame (the fixed test view, 20 units across 64 pixels, looking down −Z): a grey
/// scalar face over the whole view with a thick edge line along y = 0; in the highlight scene a
/// flat selection fill (zero normal) in the left half's top and a lit hover face in its bottom.
fn modelling_frame(ssaa: f32) -> Frame {
    let mut model = GpuScene::default();
    let grey = model.color([150, 150, 156, 255]);
    let edge = model.color([20, 24, 30, 255]);
    square(&mut model, [-12.0, -12.0, 12.0, 12.0], 0.0, [0.0, 0.0, 1.0], grey);
    model.line([-12.0, 0.0, 0.0], [12.0, 0.0, 0.0], edge, 8.0, false);
    let mut highlight = GpuScene::default();
    let selected = highlight.color([40, 140, 255, 255]);
    let hovered = highlight.color([250, 180, 60, 255]);
    square(&mut highlight, [-8.0, 3.0, -2.0, 8.0], 0.0, [0.0; 3], selected);
    square(&mut highlight, [-8.0, -8.0, -2.0, -3.0], 0.0, [0.0, 0.0, 1.0], hovered);
    Frame { hl_key: 1, hl_slot: Arc::new(Mutex::new(Some(highlight))), ..test_frame(model, ssaa) }
}

/// Pixels of the edge line (row 32 is y = 0), the selection fill and the hovered face, away from
/// their borders.
fn overlays(bytes: &[u8]) -> Vec<[u8; 3]> {
    let mut out: Vec<[u8; 3]> = (40..56).map(|x| at(bytes, x, 32)).collect();
    out.extend((10..16).flat_map(|y| (10..20).map(move |x| (x, y))).map(|(x, y)| at(bytes, x, y)));
    out.extend((48..54).flat_map(|y| (10..20).map(move |x| (x, y))).map(|(x, y)| at(bytes, x, y)));
    out
}

/// Plain face pixels (the right half, away from the line).
fn face(bytes: &[u8]) -> Vec<[u8; 3]> {
    (40..56).flat_map(|y| (40..56).map(move |x| (x, y))).map(|(x, y)| at(bytes, x, y)).collect()
}

/// The Realistic look lights the opaque scalar face with the studio (PBR Neutral, dithered), and
/// every overlay (the edge line, a flat selection fill, a lit hover highlight) keeps the very
/// pixels of the standard look, drawn directly and supersampled. Until the studio is installed a
/// look with no product lights draws the standard picture, and drawing Realistic leaves later
/// standard frames byte-identical.
#[test]
fn realistic_lights_scalar_faces_and_keeps_every_overlay() {
    let Some((_, device, queue)) = gpu() else { return };
    for ssaa in [1.0, 2.0] {
        let mut r = renderer(&device);
        let mut frame = modelling_frame(ssaa);
        let standard = render_frame(&mut r, &device, &queue, &frame, true);
        assert_eq!(r.pic.ss_scaled, ssaa > 1.0, "drawn as asked: ×{ssaa}");
        frame.product_look = Some(ProductLook::product());
        let waiting = render_frame(&mut r, &device, &queue, &frame, true);
        assert!(waiting == standard, "×{ssaa}: without its studio, the standard picture");
        r.set_studio_environment(&device, &queue, &studio(StudioPreset::product()));
        let realistic = render_frame(&mut r, &device, &queue, &frame, true);
        assert_eq!(overlays(&realistic), overlays(&standard), "×{ssaa}: the overlays keep their pixels");
        let (lit, plain) = (face(&realistic), face(&standard));
        assert!(mean(&lit) > 40.0, "×{ssaa}: the studio lights the face: mean {}", mean(&lit));
        assert!(lit != plain, "×{ssaa}: the face is drawn in the look");
        if ssaa == 1.0 {
            let distinct: std::collections::BTreeSet<[u8; 3]> = lit.iter().copied().collect();
            assert!(distinct.len() > 1, "the look dithers the face");
        }
        // Realistic again, from a fresh scene key: the same picture (no state carried over).
        frame.key += 1;
        let again = render_frame(&mut r, &device, &queue, &frame, true);
        assert_eq!(overlays(&again), overlays(&realistic));
        // And Standard after it is today's picture, byte for byte.
        frame.product_look = None;
        let back = render_frame(&mut r, &device, &queue, &frame, true);
        assert!(back == standard, "×{ssaa}: the standard picture after Realistic");
        // A renderer takes a scene out of its slot: a fresh one is given the scenes again.
        let mut fresh = renderer(&device);
        assert!(render_frame(&mut fresh, &device, &queue, &modelling_frame(ssaa), true) == standard, "×{ssaa}: as a fresh renderer draws it");
    }
}

/// A scalar look in the Realistic view is the implicit dielectric `standard_surface` of §6.2: its
/// sRGB colour decoded exactly, metalness 0, roughness 0.5, IOR 1.5, uncoated. It shades like a
/// `pbr` look with those parameters, within the dither.
#[test]
fn a_scalar_face_is_lit_as_a_rough_dielectric() {
    let Some((_, device, queue)) = gpu() else { return };
    let srgb = [180u8, 90, 40];
    let decode = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c > 0.04045 { ((c + 0.055) / 1.055).powf(2.4) } else { c / 12.92 }
    };
    let mut scalar = GpuScene::default();
    let colour = scalar.color([srgb[0], srgb[1], srgb[2], 255]);
    square(&mut scalar, [-12.0, -12.0, 12.0, 12.0], 0.0, [0.0, 0.0, 1.0], colour);
    let Some(pbr) = pbr_scene(Vec3::new(decode(srgb[0]), decode(srgb[1]), decode(srgb[2])), 0.0, 0.5, 1.5) else { panic!("valid surface") };
    let mut r = renderer(&device);
    r.set_studio_environment(&device, &queue, &studio(StudioPreset::product()));
    let look = Some(ProductLook::product());
    let draw = |r: &mut Renderer, scene: GpuScene, key: u64| {
        let frame = Frame { key, product_look: look, ..test_frame(scene, 1.0) };
        inner_pixels(&render_frame(r, &device, &queue, &frame, true))
    };
    let a = draw(&mut r, scalar, 1);
    let b = draw(&mut r, pbr, 2);
    assert_eq!(a.len(), b.len());
    let worst = a.iter().zip(&b).map(|(p, q)| p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)).max().unwrap_or(0);
    assert!(worst <= 2, "the scalar face shades as its dielectric: {:?} against {:?}, worst {worst}", a.first(), b.first());
    assert!(mean(&a) > 30.0, "lit: {}", mean(&a));
}

/// The look changes no geometry: the model's batches are uploaded once and kept when a frame
/// switches to Realistic and back, so a switch is a frame of drawing only.
#[test]
fn switching_the_look_uploads_nothing() {
    let Some((_, device, queue)) = gpu() else { return };
    let mut r = renderer(&device);
    r.set_studio_environment(&device, &queue, &studio(StudioPreset::product()));
    let mut frame = modelling_frame(1.0);
    let _ = render_frame(&mut r, &device, &queue, &frame, true);
    let uploaded = r.uploads();
    for look in [Some(ProductLook::product()), None, Some(ProductLook::product())] {
        frame.product_look = look;
        let pixels = render_frame(&mut r, &device, &queue, &frame, true);
        assert!(mean(&region(&pixels, 8, 56)) > 20.0, "drawn");
        assert_eq!(r.uploads(), uploaded, "{look:?}: nothing uploaded again");
    }
}

/// A Render preview may be the first frame on a new device. Its metal should reflect the
/// studio exactly as it does after a Model frame has used the neutral environment.
#[test]
fn a_first_preview_has_the_same_metal_reflection_as_after_a_model_frame() {
    let Some((_, device, queue)) = gpu() else { return };
    let env = studio(StudioPreset::product());
    let preview = |warm: bool| {
        let mut r = renderer(&device);
        if warm {
            let _ = render_frame(&mut r, &device, &queue, &modelling_frame(1.0), true);
        }
        r.set_studio_environment(&device, &queue, &env);
        let scene = pbr_scene(Vec3::new(0.7, 0.7, 0.7), 1.0, 0.25, 1.5).unwrap();
        // The preview's own scene and an empty highlight, under keys the Model frame did not use,
        // as a view's next frame publishes them: an empty slot keeps the previous upload.
        let mut frame = Frame { key: 2, hl_key: 2, hl_slot: Arc::new(Mutex::new(Some(GpuScene::default()))), ..test_frame(scene, 1.0) };
        frame.product = true;
        frame.product_lights = Some(Vec::new());
        frame.product_look = Some(ProductLook::product());
        render_frame(&mut r, &device, &queue, &frame, true)
    };
    let cold = preview(false);
    let warm = preview(true);
    assert!(mean(&face(&warm)) > 20.0, "the studio is reflected on the metal");
    assert!(cold == warm, "initial studio lighting is independent of a prior Model frame");
}

/// The opaque faces' entry points, which read the studio in the Realistic look, translate to
/// WebGL2's GLSL ES 3.00 (naga's GL backend, as wgpu's on the web), the studio's equirectangular maps
/// included: Realistic exists on WebGL2 too, within its four bind groups.
#[test]
fn the_realistic_face_shaders_translate_for_webgl2() {
    use naga::back::glsl;
    let source = format!("{}\n{}", super::SHADER, crate::pbr::SOURCE);
    let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    // The cut entries (`fs_solid_cut`, `fs_solid_cut_flat`) only write the cap's depth: they shade nothing.
    for entry in ["fs_solid", "fs_solid_open", "fs_solid_flat", "fs_solid_open_flat"] {
        let pipeline = glsl::PipelineOptions { shader_stage: naga::ShaderStage::Fragment, entry_point: entry.into(), multiview: None };
        let mut out = String::new();
        let written =
            glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default()).and_then(|mut w| w.write());
        let reflection = written.unwrap_or_else(|e| panic!("{entry}: {e}"));
        assert!(out.starts_with("#version 300 es"));
        for binding in [0, 11, 12] {
            assert!(
                reflection.texture_mapping.values().any(|mapping| {
                    module.global_variables[mapping.texture].binding.as_ref().is_some_and(|b| b.group == 3 && b.binding == binding)
                }),
                "{entry} reads studio texture {binding}"
            );
        }
    }
}

fn studio(preset: StudioPreset) -> StudioEnvironment {
    static BAKED: std::sync::Mutex<Vec<StudioEnvironment>> = std::sync::Mutex::new(Vec::new());
    let mut baked = BAKED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(env) = baked.iter().find(|env| env.preset() == preset) {
        return env.clone();
    }
    let env = StudioEnvironment::bake(preset);
    baked.push(env.clone());
    env
}

fn pbr_scene(base: Vec3, metallic: f64, roughness: f64, ior: f64) -> Option<GpuScene> {
    swatch(base, metallic, roughness, ior, [0.0, 0.0, 1.0])
}

fn swatch(base: Vec3, metallic: f64, roughness: f64, ior: f64, normal: [f32; 3]) -> Option<GpuScene> {
    let surface = solidcraft_viewport::surface::OpaqueSurface::new(base, metallic, roughness, ior).ok()?;
    let mut scene = GpuScene::default();
    for p in [[-12.0, -12.0, 0.0], [12.0, -12.0, 0.0], [12.0, 12.0, 0.0], [-12.0, -12.0, 0.0], [12.0, 12.0, 0.0], [-12.0, 12.0, 0.0]] {
        scene.pbr_tri(p, normal, surface);
    }
    Some(scene)
}

fn inner_pixels(bytes: &[u8]) -> Vec<[u8; 3]> {
    const SIZE: usize = 64;
    let mut out = Vec::new();
    for y in 8..SIZE - 8 {
        for x in 8..SIZE - 8 {
            let i = (y * SIZE + x) * 4;
            if let Some(p) = bytes.get(i..i + 3) {
                out.push([p[0], p[1], p[2]]);
            }
        }
    }
    out
}

#[test]
fn installing_a_studio_invalidates_parked_tiles_waiting_for_it() {
    let Some((_, device, queue)) = gpu() else { return };
    let mut renderer = renderer(&device);
    let preset = StudioPreset::soft();
    let mut frame = modelling_frame(1.0);
    frame.product_look = Some(ProductLook { studio: preset, ..Default::default() });
    renderer.prepare_view(1, &device, &queue, &frame);
    let mut other = modelling_frame(1.0);
    other.product_look = frame.product_look;
    renderer.prepare_view(2, &device, &queue, &other);
    assert!(renderer.picture(1).is_some_and(|picture| picture.ss_key.is_some()));
    assert!(renderer.picture(2).is_some_and(|picture| picture.ss_key.is_some()));
    renderer.set_studio_environment(&device, &queue, &studio(preset));
    assert!(renderer.stats().lock().unwrap().installed_studios.contains(&Some(preset)));
    assert!(renderer.picture(1).is_some_and(|picture| picture.ss_key.is_none()));
    assert!(renderer.picture(2).is_some_and(|picture| picture.ss_key.is_none()));
}

/// A lossless white surface in unit radiance returns unit radiance at every
/// angle. Measure the final GPU pixels, including the lookup, tone curve and dither.
#[test]
fn a_white_furnace_keeps_the_same_display_value_across_roughness_and_angle() {
    let Some((_, device, queue)) = gpu() else { return };
    let mut renderer = renderer(&device);
    let furnace = StudioEnvironment::furnace(1.0);
    renderer.set_studio_environment(&device, &queue, &furnace);
    let mapped = f64::from(ibl::pbr_neutral([1.0; 3])[0]);
    let expected = (1.055 * mapped.powf(1.0 / 2.4) - 0.055) * 255.0;
    let mut key = 0;
    for (metallic, ior) in [(0.0, 1.0), (0.0, 1.5), (1.0, 1.5)] {
        for roughness in [0.05, 0.4, 1.0] {
            for cosine in [0.05_f32, 0.4, 1.0] {
                let normal = [(1.0 - cosine * cosine).sqrt(), 0.0, cosine];
                let scene = swatch(Vec3::new(1.0, 1.0, 1.0), metallic, roughness, ior, normal).unwrap();
                key += 1;
                let frame =
                    Frame { key, product_look: Some(ProductLook { studio: furnace.preset(), ..Default::default() }), ..test_frame(scene, 1.0) };
                let pixels = render_frame(&mut renderer, &device, &queue, &frame, true);
                let got = mean(&inner_pixels(&pixels));
                assert!((got - expected).abs() <= 2.0, "{metallic} {ior} {roughness} {cosine}: {got} vs {expected}");
            }
        }
    }
}
