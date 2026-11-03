use super::*;

use std::collections::BTreeMap;
use std::io::Cursor;

use serde_json::{Value, json};
use solidcraft_render::material_graph::graph::TextureRef;
use solidcraft_render::material_graph::reference::{self, ReferenceSurface, ReferenceTexture};
use solidcraft_render::material_graph::{MaterialGraph, base_sample, lower, validate};

fn png(image: image::DynamicImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn rgba_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    png(image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)))))
}

fn key(name: &str, srgb: bool) -> TextureKey {
    let mut sha = format!("{:0<64}", name.bytes().map(|b| format!("{:x}", b % 16)).collect::<String>());
    sha.truncate(64);
    TextureKey { sha256: sha, srgb }
}

fn source(bytes: Vec<u8>, srgb: bool) -> TextureSource {
    let role = role_of(&bytes, srgb);
    TextureSource::new(bytes, role).unwrap()
}

fn prepared(name: &str, bytes: Vec<u8>, srgb: bool, policy: ResidentPolicy, caps: TextureCaps) -> PreparedTexture {
    prepare(key(name, srgb), &source(bytes, srgb), policy, caps, &|| false).unwrap()
}

/// IEEE half bits to f32 (for checking).
fn half_value(h: u16) -> f32 {
    let (exp, mant) = (i32::from((h >> 10) & 0x1f), f32::from(h & 0x3ff));
    if exp == 0 { mant * 2f32.powi(-24) } else { (1.0 + mant / 1024.0) * 2f32.powi(exp - 15) }
}

// Data preparation, CPU only.

#[test]
fn half_floats_round_to_nearest() {
    assert_eq!([half_bits(0.0), half_bits(1.0), half_bits(0.5), half_bits(65504.0)], [0, 0x3c00, 0x3800, 0x7bff]);
    assert_eq!(half_bits(2f32.powi(-24)), 1, "the smallest subnormal");
    assert_eq!([half_bits(f32::NAN), half_bits(-1.0)], [0, 0]);
    for v in (0..=65535u16).step_by(7) {
        let x = f32::from(v) / 65535.0;
        let h = half_value(half_bits(x));
        // Within half a unit in the last place of an 11-bit significand.
        let ulp = if x < 2f32.powi(-14) { 2f32.powi(-24) } else { 2f32.powi(x.log2().floor() as i32 - 10) };
        assert!((h - x).abs() <= ulp * 0.5 + 1e-9, "{v}: {x} → {h}");
    }
}

#[test]
fn mip_chains_are_counted_through_one_texel() {
    assert_eq!([mip_levels(1, 1), mip_levels(5, 3), mip_levels(16384, 1), mip_levels(2048, 2048)], [1, 3, 15, 12]);
    assert_eq!(chain_bytes(4, 4, 4), (16 + 4 + 1) * 4);
    assert_eq!(chain_bytes(5, 3, 4), (15 + 2 + 1) * 4);
    // A full chain is under 4/3 of its base.
    let base = 16384u64 * 16384 * 4;
    assert!(chain_bytes(16384, 16384, 4) < base / 3 * 4 + 4);
}

#[test]
fn the_resident_clamp_follows_the_device_and_the_budget() {
    let native = ResidentPolicy::native(16384);
    assert_eq!((native.max_edge, native.budget_bytes), (16384, 2 << 30));
    // A full-size RGBA8 image is resident at full size (4/3 GiB with mips, within 2 GiB) …
    assert_eq!(native.levels_to_drop(16384, 16384, 4, true), 0);
    // … a half-float height map of that size is not (8 B/px: 2.7 GiB), one level down it is.
    assert_eq!(native.levels_to_drop(16384, 16384, 8, true), 1);
    assert_eq!(ResidentPolicy::native(8192).levels_to_drop(16384, 4096, 4, true), 1);
    let webgl = ResidentPolicy::for_limits(&wgpu::Limits::downlevel_webgl2_defaults());
    assert_eq!((webgl.max_edge, webgl.budget_bytes), (2048, 256 << 20));
    assert_eq!(webgl.levels_to_drop(16384, 16384, 4, true), 3);
    assert_eq!(webgl.levels_to_drop(16384, 512, 4, true), 3, "the longer side decides");
    assert_eq!(webgl.levels_to_drop(2048, 2048, 4, true), 0);
    assert_eq!(ResidentPolicy::for_limits(&wgpu::Limits::default()), ResidentPolicy::native(8192));
    assert_eq!(ResidentPolicy::downlevel(16384).max_edge, 2048);
    // The path tracer: 16 B/px without mips, within its job budget and its binding size.
    let path = ResidentPolicy::path(128 << 20);
    assert_eq!(path.budget_bytes, 96 << 20);
    assert_eq!(path.max_edge, 2508);
    assert_eq!(path.levels_to_drop(4096, 4096, PATH_BYTES_PER_PIXEL, false), 1);
    assert_eq!(path.levels_to_drop(16384, 16384, PATH_BYTES_PER_PIXEL, false), 3);
    let small = ResidentPolicy::path(16 << 20);
    assert_eq!((small.budget_bytes, small.max_edge), (16 << 20, 1024));
    // Hostile sizes end at 1 × 1, never loop.
    assert_eq!(ResidentPolicy { max_edge: 1, budget_bytes: 0 }.levels_to_drop(u32::MAX, u32::MAX, 16, true), 31);
}

#[test]
fn an_images_role_follows_its_use() {
    let grey16 = png(image::DynamicImage::ImageLuma16(image::ImageBuffer::new(2, 2)));
    let grey8 = png(image::DynamicImage::ImageLuma8(image::ImageBuffer::new(2, 2)));
    assert_eq!(role_of(&grey16, false), MapRole::Height);
    assert_eq!(role_of(&grey8, false), MapRole::Roughness);
    assert_eq!(role_of(&grey16, true), MapRole::BaseColor);
    assert_eq!(role_of(b"\xff\xd8junk", false), MapRole::Roughness);
}

#[test]
fn texture_keys_come_from_constants_and_look_values() {
    let text = include_str!("../../../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json");
    let graph: MaterialGraph = serde_json::from_str(text).unwrap();
    let ir = lower(&validate(&graph).unwrap()).unwrap();
    assert_eq!(texture_keys(&ir, &BTreeMap::new()), [None, None, None, None], "an interface texture with no value");
    let sha = "ab".repeat(32);
    let values = BTreeMap::from([("mark".to_owned(), Constant::Texture(TextureRef { texture: sha.clone() }))]);
    assert_eq!(texture_keys(&ir, &values)[0], Some(TextureKey { sha256: sha, srgb: false }));
    let hostile = BTreeMap::from([("mark".to_owned(), Constant::Texture(TextureRef { texture: "../../etc/passwd".into() }))]);
    assert_eq!(texture_keys(&ir, &hostile)[0], None, "only asset keys");
}

#[test]
fn preparing_downsamples_above_the_policy_and_never_leaves_one_texel() {
    let policy = ResidentPolicy { max_edge: 4, budget_bytes: 1 << 20 };
    let t = prepared("wide", rgba_png(16, 8, |x, _| [(16 * x) as u8, 0, 0, 255]), false, policy, TextureCaps::default());
    assert_eq!((t.width, t.height, t.dropped, t.source, t.format), (4, 2, 2, [16, 8], GpuFormat::Linear8));
    assert_eq!(t.pixels.len(), 4 * 2 * 4);
    // Each output averages four source columns: (16x … 16x + 48) → 24 + 64 i.
    assert_eq!(t.pixels.as_chunks::<4>().0.iter().map(|p| p[0]).collect::<Vec<_>>(), [24, 88, 152, 216, 24, 88, 152, 216]);
    // Over the budget (with mips) is also a reason to drop.
    let tight = ResidentPolicy { max_edge: 4096, budget_bytes: chain_bytes(8, 4, 4) };
    let t = prepared("budget", rgba_png(16, 8, |_, _| [1; 4]), true, tight, TextureCaps::default());
    assert_eq!((t.width, t.height, t.format), (8, 4, GpuFormat::Srgb8));
    let one = prepared("one", rgba_png(1, 1, |_, _| [9, 8, 7, 6]), true, ResidentPolicy::native(4096), TextureCaps::default());
    assert_eq!((one.width, one.height, one.pixels), (2, 2, [9, 8, 7, 6].repeat(4)));
}

#[test]
fn heights_are_half_floats_where_the_device_draws_them_and_8_bit_otherwise() {
    let bytes = png(image::DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(2, 1, |x, _| image::Luma([[0u16, 40000][x as usize]]))));
    let caps = TextureCaps { srgb_renderable: true, half_float_renderable: true };
    let t = prepared("h", bytes.clone(), false, ResidentPolicy::native(4096), caps);
    assert_eq!((t.format, t.pixels.len()), (GpuFormat::Half, 16));
    let texels: Vec<u16> = t.pixels.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
    let h = half_bits(40000.0 / 65535.0);
    assert_eq!(texels, [0, 0, 0, 0x3c00, h, h, h, 0x3c00]);
    let t = prepared("h8", bytes, false, ResidentPolicy::native(4096), TextureCaps::default());
    assert_eq!((t.format, t.pixels), (GpuFormat::Linear8, vec![0, 0, 0, 255, 156, 156, 156, 255]));
}

#[test]
fn the_decoder_refuses_hostile_input_at_once_and_decodes_the_rest_off_thread() {
    let mut decoder = TextureDecoder::new(ResidentPolicy::native(4096), TextureCaps::default());
    assert!(decoder.request(key("junk", true), b"not an image".to_vec()).is_err());
    assert!(decoder.request(key("big", true), rgba_png(MAX_TEXTURE_EDGE + 1, 1, |_, _| [0; 4])).is_err(), "over the edge");
    let mut truncated = rgba_png(4, 4, |_, _| [1; 4]);
    truncated.truncate(truncated.len() - 3);
    assert!(decoder.request(key("cut", true), truncated).is_err());
    let full = decoder.budget().remaining();
    decoder.request(key("ok", true), rgba_png(8, 4, |x, y| [x as u8, y as u8, 0, 255])).unwrap();
    decoder.request(key("ok", true), rgba_png(8, 4, |_, _| [0; 4])).unwrap();
    assert_eq!(decoder.pending(), 1, "a key decodes once");
    assert_eq!(decoder.budget().remaining(), full - 8 * 4 * 4, "charged from the header");
    let done = decoder.finish(std::time::Duration::from_secs(60));
    assert_eq!(done.len(), 1);
    let t = done.into_iter().next().unwrap().unwrap();
    assert_eq!((t.width, t.height, t.key), (8, 4, key("ok", true)));
    assert_eq!(decoder.pending(), 0);
}

#[test]
fn a_texture_that_fails_to_decode_returns_its_budget() {
    let mut lying = rgba_png(1, 1, |_, _| [0; 4]);
    lying[16..20].copy_from_slice(&64u32.to_be_bytes());
    lying[20..24].copy_from_slice(&64u32.to_be_bytes());
    let mut crc = !0u32;
    for b in &lying[12..29] {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    lying[29..33].copy_from_slice(&(!crc).to_be_bytes());
    let mut decoder = TextureDecoder::new(ResidentPolicy::native(4096), TextureCaps::default());
    let full = decoder.budget().remaining();
    decoder.request(key("liar", false), lying.clone()).unwrap();
    assert_eq!(decoder.budget().remaining(), full - 64 * 64 * 4);
    let done = decoder.finish(std::time::Duration::from_secs(60));
    assert!(matches!(done.as_slice(), [Err(_)]));
    assert_eq!(decoder.budget().remaining(), full);
    decoder.request(key("liar", false), lying).unwrap();
    assert_eq!(decoder.pending(), 0, "a failed key is not retried");
}

#[test]
fn a_designs_textures_share_the_decoders_budget() {
    let mut decoder = TextureDecoder::with_budget(ResidentPolicy::native(4096), TextureCaps::default(), DecodeBudget::with_bytes(100));
    decoder.request(key("a", false), rgba_png(4, 4, |_, _| [0; 4])).unwrap();
    assert!(decoder.request(key("b", false), rgba_png(4, 4, |_, _| [0; 4])).is_err(), "64 + 64 > 100");
    assert_eq!(decoder.budget().remaining(), 36);
}

// GPU.

fn device() -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    crate::open_headless().ok()
}

fn resident(device: &wgpu::Device, caps: TextureCaps) -> ResidentTextures {
    let mut r = ResidentTextures::new(device);
    r.caps = caps;
    r
}

fn level(r: &ResidentTextures, device: &wgpu::Device, queue: &wgpu::Queue, k: &TextureKey, i: u32) -> (u32, u32, Vec<u8>) {
    r.read_level(device, queue, k, i).unwrap()
}

fn close(got: &[u8], want: &[u8], tolerance: u8, what: &str) {
    assert_eq!(got.len(), want.len(), "{what}");
    let worst = got.iter().zip(want).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(worst <= tolerance, "{what}: {got:?} vs {want:?}");
}

/// The GPU's chain against the analytic one: an sRGB checker averages to linear grey (sRGB 188),
/// data and alpha to their means, levels by level through 1 × 1.
#[test]
fn the_gpu_mip_chain_matches_its_analytic_levels() {
    let Some((_, device, queue)) = device() else { return };
    for caps in [TextureCaps::default(), TextureCaps { srgb_renderable: false, half_float_renderable: false }] {
        let mut r = resident(&device, caps);
        let checker = |x: u32, y: u32| if (x + y).is_multiple_of(2) { [0, 0, 0, 0] } else { [255, 255, 255, 255] };
        let colour = prepared("checker", rgba_png(8, 4, checker), true, r.policy, caps);
        let k = colour.key.clone();
        r.install(&device, &queue, colour).unwrap();
        assert_eq!(r.size(&k), Some([8, 4, 4]));
        let grey = solidcraft_geom::srgb::encode8(0.5);
        for (i, (w, h)) in [(4, 2), (2, 1), (1, 1)].into_iter().enumerate() {
            let (gw, gh, bytes) = level(&r, &device, &queue, &k, i as u32 + 1);
            assert_eq!((gw, gh), (w, h));
            close(&bytes, &[grey, grey, grey, 128].repeat((w * h) as usize), 1, &format!("sRGB level {} ({caps:?})", i + 1));
        }
        assert_eq!(level(&r, &device, &queue, &k, 0).2, rgba_png_pixels(8, 4, checker), "level 0 as uploaded");
    }
}

fn rgba_png_pixels(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let pixel = &pixel;
    (0..height).flat_map(|y| (0..width).flat_map(move |x| pixel(x, y))).collect()
}

/// Odd sides: the GPU's area average equals the CPU's (`DecodedTexture::halve`, the same rule as
/// `texture_sampling::mips`) at every level.
#[test]
fn odd_sides_average_by_area_on_the_gpu_as_on_the_cpu() {
    let Some((_, device, queue)) = device() else { return };
    let mut r = resident(&device, TextureCaps::default());
    for (srgb, name) in [(false, "odd-data"), (true, "odd-colour")] {
        let pixel = |x: u32, y: u32| [(37 * x + 11 * y) as u8, (90 + 13 * x * y) as u8, (200 - 17 * y) as u8, (255 - 23 * x) as u8];
        let bytes = rgba_png(7, 5, pixel);
        let t = prepared(name, bytes.clone(), srgb, r.policy, TextureCaps::default());
        let k = t.key.clone();
        r.install(&device, &queue, t).unwrap();
        let mut cpu = source(bytes, srgb).decode(|| false).unwrap();
        for i in 1..3 {
            cpu = cpu.halve(|| false).unwrap().unwrap();
            let (w, h, gpu) = level(&r, &device, &queue, &k, i);
            assert_eq!((w, h), (cpu.info().width, cpu.info().height));
            close(&gpu, cpu.rgba(), 1, &format!("{name} level {i}"));
        }
    }
}

#[test]
fn half_float_heights_average_on_the_gpu() {
    let Some((adapter, device, queue)) = device() else { return };
    let caps = TextureCaps::of(&adapter);
    if !caps.half_float_renderable {
        return;
    }
    let mut r = resident(&device, caps);
    let values = [0u16, 65535, 30000, 50000];
    let t = prepared(
        "height",
        png(image::DynamicImage::ImageLuma16(image::ImageBuffer::from_fn(2, 2, |x, y| image::Luma([values[(y * 2 + x) as usize]])))),
        false,
        r.policy,
        caps,
    );
    assert_eq!(t.format, GpuFormat::Half);
    let k = t.key.clone();
    r.install(&device, &queue, t).unwrap();
    let (_, _, texel) = level(&r, &device, &queue, &k, 1);
    let mean = values.iter().map(|v| f32::from(*v) / 65535.0).sum::<f32>() / 4.0;
    let got: Vec<f32> = texel.as_chunks::<2>().0.iter().map(|b| half_value(u16::from_le_bytes(*b))).collect();
    for c in &got[..3] {
        assert!((c - mean).abs() < 1e-3, "{got:?} vs {mean}");
    }
    assert_eq!(got[3], 1.0);
}

/// Over the budget, the least recently drawn texture drops a level first, on the GPU: its new
/// level 0 is its old level 1.
#[test]
fn the_budget_drops_a_level_of_the_least_recently_drawn() {
    let Some((_, device, queue)) = device() else { return };
    let mut r = resident(&device, TextureCaps::default());
    r.policy = ResidentPolicy { max_edge: 4096, budget_bytes: chain_bytes(64, 64, 4) * 2 + chain_bytes(32, 32, 4) };
    let make = |name: &str, shade: u8| prepared(name, rgba_png(64, 64, |x, y| [shade, (x * 4) as u8, (y * 4) as u8, 255]), false, r.policy, r.caps);
    let (a, b, c) = (make("a", 10), make("b", 20), make("c", 30));
    let (ka, kb, kc) = (a.key.clone(), b.key.clone(), c.key.clone());
    r.install(&device, &queue, a).unwrap();
    r.install(&device, &queue, b).unwrap();
    let before = level(&r, &device, &queue, &ka, 1);
    r.next_frame();
    r.touch(&kb);
    let generation = r.generation();
    r.install(&device, &queue, c).unwrap();
    assert!(r.generation() > generation);
    assert_eq!(r.size(&ka), Some([32, 32, 6]), "`a` was drawn least recently");
    assert_eq!(r.size(&kb), Some([64, 64, 7]));
    assert_eq!(r.size(&kc), Some([64, 64, 7]));
    assert_eq!(level(&r, &device, &queue, &ka, 0), before, "its chain moved up a level");
    let stats = r.stats();
    assert_eq!((stats.resident, stats.dropped_levels), (3, 1));
    assert!(stats.bytes <= r.policy.budget_bytes);
    r.remove(&kc);
    assert_eq!(r.stats().resident, 2);
}

#[test]
fn a_texture_larger_than_the_device_is_refused() {
    let Some((_, device, queue)) = device() else { return };
    let mut r = resident(&device, TextureCaps::default());
    let edge = device.limits().max_texture_dimension_2d + 1;
    let t = PreparedTexture {
        key: key("x", false),
        width: edge,
        height: 1,
        format: GpuFormat::Linear8,
        pixels: vec![0; edge as usize * 4],
        dropped: 0,
        source: [edge, 1],
    };
    assert!(r.install(&device, &queue, t).is_err());
    let t = PreparedTexture { key: key("y", false), width: 2, height: 2, format: GpuFormat::Linear8, pixels: vec![0; 3], dropped: 0, source: [2, 2] };
    assert!(r.install(&device, &queue, t).is_err(), "pixels must match the size");
    assert_eq!(r.stats().resident, 0);
}

// Sampling, against track T's reference.

fn graph(space: &str, wrap: &str, channel: &str) -> Value {
    json!({
        "name": "Sampled",
        "interface": [{"name": "map", "type": "texture"}],
        "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}, "inputs": {"origin": [-2.5, -1.75, 0.0], "u": [3.0, 0.0, 0.0], "v": [0.0, 2.5, 0.0]}},
            {"id": "img", "kind": "image", "params": {"space": space, "wrap": wrap, "channel": channel},
             "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "map"}, "default": [0.2, 0.4, 0.6]}},
            {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "img"}, "specular_roughness": {"from": "img.value"}, "coat_weight": {"from": "img.alpha"}}}
        ],
        "output": "s"
    })
}

fn height_graph(relief: f64) -> Value {
    json!({
        "name": "Height",
        "interface": [{"name": "map", "type": "texture"}],
        "nodes": [
            {"id": "uv", "kind": "texcoord.triplanar", "params": {"space": "world"}, "inputs": {"scale_mm": [4.0, 4.0, 4.0]}},
            {"id": "img", "kind": "image", "params": {"space": "linear", "wrap": "repeat", "channel": "r"},
             "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "map"}}},
            {"id": "n", "kind": "normal.from_height", "inputs": {"height": {"from": "img.value"}, "relief_mm": relief}},
            {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "img"}, "geometry_normal": {"from": "n"}}}
        ],
        "output": "s"
    })
}

/// A smooth 8 × 8 texture with every channel and alpha varying.
fn smooth(x: u32, y: u32) -> [u8; 4] {
    let f = |a: f32, b: f32| ((0.5 + 0.45 * (a * x as f32 + b * y as f32).sin()) * 255.0).round() as u8;
    [f(0.7, 0.3), f(0.2, 0.9), f(1.1, -0.4), f(-0.5, 0.6)]
}

/// The worst surface difference of `graph` sampled on the GPU from `texture` (or the stand-in)
/// against the reference, at T's points, unfiltered and at a small footprint.
fn worst(device: &wgpu::Device, queue: &wgpu::Queue, graph: &Value, texture: Option<(&[u8], u32, u32)>) -> (String, f64) {
    let text = graph.to_string();
    let parsed: MaterialGraph = serde_json::from_str(&text).unwrap();
    let valid = validate(&parsed).unwrap_or_else(|e| panic!("{e}"));
    let base = base_sample(&valid).unwrap();
    let sha = "5e".repeat(32);
    let values = BTreeMap::from([("map".to_owned(), Constant::Texture(TextureRef { texture: sha.clone() }))]);
    let (ir, words) = reference::prepare_json(&text, &values).unwrap();
    let keys = texture_keys(&ir, &values);
    let mut r = resident(device, TextureCaps::default());
    let reference_texture = texture.map(|(bytes, w, h)| {
        let k = keys[0].clone().unwrap();
        let t = prepare(
            k.clone(),
            &source(
                rgba_png(w, h, |x, y| {
                    let i = ((y * w + x) * 4) as usize;
                    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
                }),
                k.srgb,
            ),
            r.policy,
            r.caps,
            &|| false,
        )
        .unwrap();
        r.install(device, queue, t).unwrap();
        ReferenceTexture::from_rgba8(w, h, bytes).unwrap()
    });
    let view = keys[0].as_ref().and_then(|k| r.view(k));
    let mut points = reference::sample_points();
    points.extend(reference::sample_points().into_iter().map(|p| p.with_footprint(0.01)));
    let views = view.map(|v| [v, v, v, v]);
    let got = crate::material::tests::gpu_surfaces_with(device, queue, &ir, &words, &base, &points, views).unwrap_or_else(|e| panic!("{e}"));
    let textures = [reference_texture.as_ref()];
    let mut worst = (String::new(), 0.0);
    for (i, (p, g)) in points.iter().zip(&got).enumerate() {
        let mut want = reference::evaluate(&ir, &words, &textures, p).unwrap().surface;
        let mut have = ReferenceSurface::from_floats(g).unwrap();
        if want.anisotropy.abs() < 1e-6 && have.anisotropy.abs() < 1e-6 {
            (want.tangent, have.tangent) = ([0.0; 3], [0.0; 3]);
        }
        let (field, d) = want.max_difference(&have);
        if d > worst.1 {
            worst = (format!("{field} at point {i}"), d);
        }
    }
    worst
}

/// Sampled colour, value and alpha equal the reference's bilinear mip-0 sample in every colour
/// space, wrap mode and channel.
#[test]
fn sampled_images_match_the_reference() {
    let Some((_, device, queue)) = device() else { return };
    let pixels: Vec<u8> = (0..8).flat_map(|y| (0..8).flat_map(move |x| smooth(x, y))).collect();
    for space in ["srgb", "linear"] {
        for wrap in ["repeat", "clamp", "mirror"] {
            for channel in ["rgb", "r", "g", "b", "a"] {
                let (field, d) = worst(&device, &queue, &graph(space, wrap, channel), Some((&pixels, 8, 8)));
                assert!(d < 1e-2, "{space} {wrap} {channel}: {field} differs by {d}");
            }
        }
    }
}

/// A height map's slope reaches the normal as the reference's one-texel differences do, through
/// triplanar planes.
#[test]
fn image_slopes_match_the_reference() {
    let Some((_, device, queue)) = device() else { return };
    let pixels: Vec<u8> = (0..8).flat_map(|y| (0..8).flat_map(move |x| smooth(x, y))).collect();
    let (field, d) = worst(&device, &queue, &height_graph(0.05), Some((&pixels, 8, 8)));
    assert!(d < 2e-2, "{field} differs by {d}");
    let flat = worst(&device, &queue, &height_graph(0.0), Some((&pixels, 8, 8)));
    assert!(flat.1 < 1e-2, "{flat:?}");
}

/// Before its texture is resident an `image` reads its `default` colour with alpha 0, as the
/// reference's unbound image does.
#[test]
fn an_image_reads_its_default_until_its_texture_is_resident() {
    let Some((_, device, queue)) = device() else { return };
    for g in [graph("srgb", "repeat", "rgb"), graph("linear", "clamp", "a"), height_graph(0.05)] {
        let (field, d) = worst(&device, &queue, &g, None);
        assert!(d < 1e-4, "{field} differs by {d}");
    }
}

// WebGL2.

fn glsl(source: &str, entries: &[(naga::ShaderStage, &str)]) -> Vec<String> {
    use naga::back::glsl;
    let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    entries
        .iter()
        .map(|(stage, entry)| {
            let mut out = String::new();
            let pipeline = glsl::PipelineOptions { shader_stage: *stage, entry_point: (*entry).into(), multiview: None };
            glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
                .and_then(|mut w| w.write())
                .unwrap_or_else(|e| panic!("{entry}: {e}"));
            out
        })
        .collect()
}

/// The mip passes and a textured shape translate to WebGL2's GLSL ES 3.00: texel fetches from one
/// level (`texelFetch`), explicit-level samples (`textureLod`) and sizes (`textureSize`).
#[test]
fn the_texture_shaders_translate_for_webgl2() {
    let mips = glsl(
        MIP_SOURCE,
        &[(naga::ShaderStage::Vertex, "vs_mip"), (naga::ShaderStage::Fragment, "fs_mip"), (naga::ShaderStage::Fragment, "fs_mip_srgb")],
    );
    assert!(mips[1].contains("texelFetch") && mips[2].contains("texelFetch"), "{}", mips[1]);
    for g in [graph("srgb", "mirror", "a"), height_graph(0.05)] {
        let parsed: MaterialGraph = serde_json::from_value(g).unwrap();
        let ir = lower(&validate(&parsed).unwrap()).unwrap();
        let source = crate::material::shape_source(&ir).unwrap();
        let out = glsl(&source, &[(naga::ShaderStage::Fragment, "fs_mat")]);
        assert!(out[0].contains("textureLod") && out[0].contains("textureSize"), "samples at an explicit level");
    }
}

/// One interface texture read with two wrap modes is two bindings, so each binding has one
/// sampler: GLSL ES's combined samplers cannot sample one texture with two.
#[test]
fn each_binding_has_one_wrap_mode() {
    let g = json!({
        "name": "Two wraps",
        "interface": [{"name": "map", "type": "texture"}],
        "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "inputs": {}},
            {"id": "a", "kind": "image", "params": {"wrap": "repeat"}, "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "map"}}},
            {"id": "b", "kind": "image", "params": {"wrap": "clamp"}, "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "map"}}},
            {"id": "c", "kind": "image", "params": {"wrap": "repeat"}, "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "map"}}},
            {"id": "m", "kind": "mix", "inputs": {"a": {"from": "a"}, "b": {"from": "b"}, "t": {"from": "c.value"}}},
            {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "m"}}}
        ],
        "output": "s"
    });
    let parsed: MaterialGraph = serde_json::from_value(g).unwrap();
    let ir = lower(&validate(&parsed).unwrap_or_else(|e| panic!("{e}"))).unwrap();
    assert_eq!(ir.textures.len(), 2, "repeat and clamp; the second repeat shares the first");
    let sha = "7c".repeat(32);
    let values = BTreeMap::from([("map".to_owned(), Constant::Texture(TextureRef { texture: sha.clone() }))]);
    let keys = texture_keys(&ir, &values);
    assert_eq!(
        (keys[0].as_ref(), keys[1].as_ref()),
        (Some(&TextureKey { sha256: sha.clone(), srgb: true }), Some(&TextureKey { sha256: sha, srgb: true }))
    );
    let source = crate::material::shape_source(&ir).unwrap();
    glsl(&source, &[(naga::ShaderStage::Fragment, "fs_mat")]);
}
