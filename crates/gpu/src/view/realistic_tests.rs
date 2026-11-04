//! The Realistic view of a modelling viewport (docs/material-graph.md §6): the same scenes with
//! the frame's product look. Opaque scalar faces are lit by the studio as an implicit dielectric;
//! highlights, edges and the standard picture itself stay exactly as they are.

use std::sync::{Arc, Mutex};

use super::glass_tests::{inner_pixels, pbr_scene, render_frame, studio, test_frame};
use super::*;

const SIZE: usize = 64;

fn renderer(device: &wgpu::Device) -> Renderer {
    let depth = crate::depth_format(device, true);
    Renderer::new(device, GpuTarget { format: wgpu::TextureFormat::Rgba8Unorm, depth: Some(depth), samples: 1, tess: false, oit: false }, None)
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
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    for ssaa in [1.0, 2.0] {
        let mut r = renderer(&device);
        let mut frame = modelling_frame(ssaa);
        let standard = render_frame(&mut r, &device, &queue, &frame, true);
        assert_eq!(r.ss_scaled, ssaa > 1.0, "drawn as asked: ×{ssaa}");
        frame.product_look = Some(ProductLook::default());
        let waiting = render_frame(&mut r, &device, &queue, &frame, true);
        assert!(waiting == standard, "×{ssaa}: without its studio, the standard picture");
        r.set_studio_environment(&device, &queue, &studio(StudioPreset::Product));
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
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
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
    r.set_studio_environment(&device, &queue, &studio(StudioPreset::Product));
    let look = Some(ProductLook::default());
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
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let mut r = renderer(&device);
    r.set_studio_environment(&device, &queue, &studio(StudioPreset::Product));
    let mut frame = modelling_frame(1.0);
    let _ = render_frame(&mut r, &device, &queue, &frame, true);
    let uploaded = r.uploads();
    for look in [Some(ProductLook::default()), None, Some(ProductLook::default())] {
        frame.product_look = look;
        let pixels = render_frame(&mut r, &device, &queue, &frame, true);
        assert!(mean(&region(&pixels, 8, 56)) > 20.0, "drawn");
        assert_eq!(r.uploads(), uploaded, "{look:?}: nothing uploaded again");
    }
}

/// The opaque faces' entry points, which read the studio in the Realistic look, translate to
/// WebGL2's GLSL ES 3.00 (naga's GL backend, as wgpu's on the web), the studio's cube map
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
    for entry in ["fs_solid", "fs_solid_cut", "fs_solid_open", "fs_solid_flat", "fs_solid_cut_flat", "fs_solid_open_flat"] {
        let pipeline = glsl::PipelineOptions { shader_stage: naga::ShaderStage::Fragment, entry_point: entry.into(), multiview: None };
        let mut out = String::new();
        let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
            .and_then(|mut w| w.write().map(|_| ()));
        assert!(written.is_ok(), "{entry}: {written:?}");
        assert!(out.starts_with("#version 300 es") && out.contains("samplerCube"), "{entry} samples the studio's cube");
    }
}
