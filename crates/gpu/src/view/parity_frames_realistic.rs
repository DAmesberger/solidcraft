// Realistic-look parity frames: the product look's PBR, scalar and material-graph paths
// (coats, coat colour, anisotropy, patterns) in a fixed set of small frames, for comparing two
// revisions that both have the Realistic view within a tolerance (`cargo xtask render-parity
// --realistic-base REV`). Explicit only:
//
// ```sh
// PARITY_OUT=<dir> cargo test -p solidcraft-gpu --lib parity_frames -- --ignored
// ```
//
// Each frame is written as `<dir>/real-<name>.rgba` (64 × 64, straight RGBA8).

use std::collections::BTreeMap;

use super::parity_frames::{FORMATS, SURFACES, VARIANTS, draw, highlight, lightings, pbr_scenes, scalar_scene, write};
use super::*;

/// The graphs of the set: the worked examples (a constant `pbr` graph, a clear-coated black
/// epoxy, brushed and etched nickel silver, solder mask over copper) and the built-in coat
/// colour, weave and orange-peel graphs.
const GRAPHS: [(&str, &str); 8] = [
    ("pbr-copper", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../render/tests/fixtures/material_graph/pbr-copper.json"))),
    ("black-epoxy-clear-coat", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../render/tests/fixtures/material_graph/black-epoxy-clear-coat.json"))),
    ("brushed-nickel-silver", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../render/tests/fixtures/material_graph/brushed-nickel-silver.json"))),
    (
        "laser-etched-nickel-silver",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json")),
    ),
    ("solder-mask-over-copper", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../render/tests/fixtures/material_graph/solder-mask-over-copper.json"))),
    ("tinted-coat-over-copper", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/appearances/graphs/tinted-coat-over-copper.json"))),
    ("woven-laminate", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/appearances/graphs/woven-laminate.json"))),
    ("orange-peel-gloss", include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/appearances/graphs/orange-peel-gloss.json"))),
];

/// `graph` over a square filling the view, turned by `slope` (z = slope·y), in a scene of its
/// own; `None` when the graph does not validate here.
fn graph_scene(json: &str, slope: f32) -> Option<GpuScene> {
    use solidcraft_render::material_graph::{MaterialGraph, base_sample, lower, uniform_words, validate};
    let graph: MaterialGraph = serde_json::from_str(json).ok()?;
    let valid = validate(&graph).ok()?;
    let ir = Arc::new(lower(&valid).ok()?);
    let base = base_sample(&valid).ok()?;
    let words = uniform_words(&ir, &BTreeMap::new()).ok()?;
    let mut scene = GpuScene::default();
    let m = scene.graphs.material(&ir, &words, &base).ok()?;
    let length = (1.0 + slope * slope).sqrt();
    let normal = [0.0, -slope / length, 1.0 / length];
    for [x, y] in [[-12.0f32, -12.0], [12.0, -12.0], [12.0, 12.0], [-12.0, -12.0], [12.0, 12.0], [-12.0, 12.0]] {
        let p = [x, y, slope * y];
        scene.graphs.vertex(m, p, normal, p);
    }
    Some(scene)
}

/// The looks of the set: the default studio at infinity and in a room around the view, the soft
/// and contrast studios, and the product lights alone.
fn looks() -> [(&'static str, ProductLook); 5] {
    let room = StudioRoom::around([-12.0, -12.0, 0.0], [12.0, 12.0, 9.0]);
    [
        ("product", ProductLook::default()),
        ("room", ProductLook { room, ..ProductLook::default() }),
        ("soft", ProductLook { studio: StudioPreset::Soft, exposure: 1.7, rotation: 1.0, ..ProductLook::default() }),
        ("contrast", ProductLook { studio: StudioPreset::Contrast, exposure: 1.4, room, ..ProductLook::default() }),
        ("nostudio", ProductLook { intensity: 0.0, ..ProductLook::default() }),
    ]
}

/// Every Realistic frame of the set (`real-…`): PBR surfaces in each look; product lights with
/// the default look; scalar scenes lit as dielectrics with their overlays; every graph tilted and
/// flat, with and without a key light, at infinity and in a room.
#[test]
#[ignore = "writes the parity frames; run through `cargo xtask render-parity`"]
fn realistic_parity_frames() {
    let Some(out) = std::env::var_os("PARITY_OUT").map(std::path::PathBuf::from) else { return };
    if let Err(e) = std::fs::create_dir_all(&out) {
        panic!("{}: {e}", out.display());
    }
    let Some((_, device, queue)) = crate::open_headless().ok() else { panic!("no GPU adapter") };
    let depth = crate::depth_format(&device, true);
    let mut written = 0usize;
    let mut fallbacks = Vec::new();
    for format in FORMATS {
        let mut r = Renderer::new(&device, GpuTarget { format, depth: Some(depth), samples: 1, tess: false, oit: false }, None);
        r.materials_mut().no_time_budget = true;
        let mut installed = None;
        let mut install = |r: &mut Renderer, preset: StudioPreset| {
            if installed != Some(preset) {
                r.set_studio_environment(&device, &queue, &studio(preset));
                installed = Some(preset);
            }
        };
        let mut number = 100_000u64;
        for (si, (base, metallic, roughness, ior)) in SURFACES.into_iter().enumerate() {
            let Ok(surface) = solidcraft_render::surface::OpaqueSurface::new(Vec3::new(base[0], base[1], base[2]), metallic, roughness, ior) else {
                panic!("surface {si}")
            };
            for (scene_name, scene) in pbr_scenes(surface) {
                for (look_name, look) in looks() {
                    for (light_name, lights) in lightings() {
                        // Product lights only with the default look; every look with the fixed rig.
                        if look_name != "product" && light_name != "fixed" {
                            continue;
                        }
                        install(&mut r, look.studio);
                        for (variant, ao, ssaa, eye) in VARIANTS {
                            number += 1;
                            let frame = Frame {
                                key: number,
                                ao_radius: ao,
                                eye,
                                product_lights: lights.clone(),
                                product_look: Some(look),
                                ..test_frame(scene.clone(), ssaa)
                            };
                            let name = format!("real-{format:?}-s{si}-{scene_name}-{look_name}-{light_name}-{variant}");
                            write(&out, &name, &draw(&mut r, &device, &queue, &frame, format));
                            written += 1;
                        }
                    }
                }
            }
        }
        for tilt in [0.0f32, 0.6] {
            for (look_name, look) in looks() {
                if !matches!(look_name, "product" | "contrast") {
                    continue;
                }
                install(&mut r, look.studio);
                for (variant, ao, ssaa, eye) in VARIANTS {
                    number += 1;
                    let frame = Frame {
                        key: number,
                        ao_radius: ao,
                        eye,
                        outline: true,
                        product_look: Some(look),
                        hl_key: number,
                        hl_slot: Arc::new(Mutex::new(Some(highlight()))),
                        hover_tint: [0.1, 0.1, 0.1],
                        selection_tint: [0.2, 0.5, 0.9],
                        ..test_frame(scalar_scene(tilt), ssaa)
                    };
                    write(&out, &format!("real-{format:?}-scalar-t{tilt}-{look_name}-{variant}"), &draw(&mut r, &device, &queue, &frame, format));
                    written += 1;
                }
            }
        }
        for (graph_name, json) in GRAPHS {
            for slope in [0.0f32, 0.6] {
                let Some(scene) = graph_scene(json, slope) else { panic!("{graph_name}: the graph builds") };
                for (look_name, look) in looks() {
                    if !matches!(look_name, "product" | "room") {
                        continue;
                    }
                    install(&mut r, look.studio);
                    for (light_name, lights) in [("fixed", None), ("key", Some(vec![super::parity_frames::KEY]))] {
                        for (variant, ao, ssaa) in [("o", 0.0, 1.0), ("ss", 4.0, 2.0)] {
                            number += 1;
                            let frame = Frame {
                                key: number,
                                ao_radius: ao,
                                product_lights: lights.clone(),
                                product_look: Some(look),
                                ..test_frame(scene.clone(), ssaa)
                            };
                            let name = format!("real-{format:?}-graph-{graph_name}-z{slope}-{look_name}-{light_name}-{variant}");
                            write(&out, &name, &draw(&mut r, &device, &queue, &frame, format));
                            if r.material_stats().fallback_draws > 0 {
                                fallbacks.push(name);
                            }
                            written += 1;
                        }
                    }
                }
            }
        }
    }
    eprintln!("parity: wrote {written} realistic frames");
    assert!(fallbacks.is_empty(), "graph frames drawn by the fallback, not their own pipeline: {fallbacks:?}");
}

/// The studios' brightness for the founder's comparison (evidence, printed only): per preset, at
/// its calibrated scale (a white diffuse face under the key reads 0.80 display in each), the
/// solid-angle mean luminance of the cyclorama alone (the surround a face reflects) and of the
/// whole studio, the darkest and brightest cyclorama directions, and the brightest light's
/// luminance over the surround.
#[test]
#[ignore = "prints the studios' brightness; evidence only"]
fn studio_surround_brightness() {
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    const N: usize = 20_000;
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let directions: Vec<[f64; 3]> = (0..N)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / N as f64;
            let r = (1.0 - z * z).sqrt();
            let phi = golden * i as f64;
            [r * phi.cos(), r * phi.sin(), z]
        })
        .collect();
    let product = StudioEnvironment::bake(StudioPreset::Product);
    let surround_of = |env: &StudioEnvironment| directions.iter().map(|d| luminance(env.ambient_source(*d))).sum::<f64>() / N as f64;
    let reference = surround_of(&product);
    for preset in StudioPreset::ALL {
        let env = StudioEnvironment::bake(preset);
        let ambient: Vec<f64> = directions.iter().map(|d| luminance(env.ambient_source(*d))).collect();
        let surround = ambient.iter().sum::<f64>() / N as f64;
        let all: Vec<f64> = directions.iter().map(|d| luminance(env.source(*d))).collect();
        let whole = all.iter().sum::<f64>() / N as f64;
        let peak = all.iter().copied().fold(0.0f64, f64::max);
        let (lo, hi) = ambient.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        eprintln!(
            "studio {preset:?}: calibration {:.4}, surround mean {surround:.4} ({:.2}× Product's), whole studio mean {whole:.4}, surround min {lo:.4} max {hi:.4}, brightest light {peak:.1} ({:.0}× the surround)",
            env.calibration(),
            surround / reference,
            peak / surround.max(1e-9)
        );
    }
}
