#![allow(clippy::needless_range_loop)] // Channels compared side by side.

use std::f64::consts::PI;

use super::reference::{dfg, shade, shade_parts, unturn};
use super::*;

/// The environment-BRDF fit stays within its stated error of the split-sum terms integrated
/// numerically from the viewport's own BRDF.
#[test]
fn env_brdf_fit_matches_the_numerical_split_sum() {
    let (mut worst, mut squares, mut count) = ((0.0_f64, 0.0_f64), (0.0, 0.0), 0.0);
    for i in 0..24 {
        for j in 0..24 {
            let nv = 0.02 + 0.98 * (f64::from(i) + 0.5) / 24.0;
            let r = 0.045 + 0.955 * f64::from(j) / 23.0;
            let (a, b) = dfg(nv, r, 4096);
            let (fa, fb) = env_brdf(nv as f32, r as f32);
            let (ea, eb) = ((f64::from(fa) - a).abs(), (f64::from(fb) - b).abs());
            worst = (worst.0.max(ea), worst.1.max(eb));
            squares = (squares.0 + ea * ea, squares.1 + eb * eb);
            count += 1.0;
            assert!((0.0..=1.0).contains(&fa) && (0.0..=1.0).contains(&fb));
        }
    }
    let rms = ((squares.0 / count).sqrt(), (squares.1 / count).sqrt());
    assert!(worst.0 <= 0.05 && worst.1 <= 0.03, "largest error {worst:?}");
    assert!(rms.0 <= 0.01 && rms.1 <= 0.01, "RMS error {rms:?}");
    // A smooth surface seen head-on reflects F0; at grazing nearly everything.
    let (a, b) = env_brdf(1.0, 0.045);
    assert!((a - 1.0).abs() < 0.02 && b < 0.02, "{a} {b}");
}

/// The shader's coefficients are the Rust fit's, digit for digit.
#[test]
fn shader_env_brdf_coefficients_are_the_fit() {
    for (table, name) in [(&ENV_BRDF_A, "A"), (&ENV_BRDF_B, "B")] {
        for part in table {
            for row in part {
                let text = format!("vec4<f32>({:.5}, {:.5}, {:.5}, {:.5})", row[0], row[1], row[2], row[3]);
                assert!(crate::pbr::SOURCE.contains(&text), "{name}: {text} is not in pbr::SOURCE");
            }
        }
    }
}

/// White furnace: in a studio of constant radiance 1 every level holds 1, the irradiance is π
/// in every direction, and a white metal (multiple scattering compensated), an index-matched
/// white diffuse surface and a white dielectric each reflect 1 at every roughness and angle.
#[test]
fn white_furnace() {
    let env = StudioEnvironment::furnace(1.0);
    assert_eq!(env.level_count(), ENV_MIPS as usize);
    for level in 0..env.level_count() {
        let mean = env.level_mean(level);
        assert!(mean.iter().all(|c| (c - 1.0).abs() < 1e-4), "level {level}: {mean:?}");
    }
    let normals = [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.3, -0.5, 0.81], [-0.7, 0.7, -0.14]];
    for n in normals {
        let e = env.irradiance_at(unit(n));
        assert!(e.iter().all(|c| (c - PI).abs() < 1e-3 * PI), "{n:?}: {e:?}");
    }
    let n = [0.0, 0.0, 1.0];
    for roughness in [0.045, 0.2, 0.5, 0.8, 1.0] {
        for nv in [0.05_f64, 0.3, 0.7, 1.0] {
            let v = [(1.0 - nv * nv).sqrt(), 0.0, nv];
            for rotation in [0.0, 1.3] {
                let metal = shade(&env, [1.0; 3], 1.0, roughness, 1.5, n, v, rotation);
                assert!(metal.iter().all(|c| (c - 1.0).abs() < 1e-3), "metal r={roughness} nv={nv}: {metal:?}");
                let diffuse = shade(&env, [1.0; 3], 0.0, roughness, 1.0, n, v, rotation);
                assert!(diffuse.iter().all(|c| (c - 1.0).abs() < 1e-3), "diffuse r={roughness} nv={nv}: {diffuse:?}");
                let plastic = shade(&env, [1.0; 3], 0.0, roughness, 1.5, n, v, rotation);
                // A white dielectric: the diffuse layer takes what the specular lobe leaves.
                assert!(plastic.iter().all(|c| (c - 1.0).abs() < 1e-3), "dielectric r={roughness} nv={nv}: {plastic:?}");
                let grey = shade(&env, [0.5; 3], 0.0, roughness, 1.5, n, v, rotation);
                assert!(grey.iter().all(|c| *c > 0.5 && *c < 1.0), "grey dielectric r={roughness} nv={nv}: {grey:?}");
                // A coloured metal: between its single-scattering albedo and one.
                let gold = shade(&env, [0.9, 0.6, 0.3], 1.0, roughness, 1.5, n, v, rotation);
                let (a, b) = env_brdf(nv as f32, roughness as f32);
                for (k, f0) in [0.9, 0.6, 0.3].into_iter().enumerate() {
                    let single = f0 * f64::from(a) + f64::from(b);
                    assert!(gold[k] >= single - 1e-6 && gold[k] <= 1.0 + 1e-6, "coloured metal r={roughness} nv={nv}: {gold:?}");
                }
            }
        }
    }
}

/// Prefiltering only spreads the studio's light: every level keeps the sharpest level's mean
/// radiance (a rotation-invariant, normalised lobe), within the sampling error.
#[test]
fn prefiltering_conserves_the_studio_energy() {
    for preset in StudioPreset::ALL {
        let env = StudioEnvironment::bake(preset);
        let sharp = env.level_mean(0);
        for level in 1..env.level_count() {
            let mean = env.level_mean(level);
            for k in 0..3 {
                assert!((mean[k] - sharp[k]).abs() <= 0.05 * sharp[k], "{preset:?} level {level}: {mean:?} against {sharp:?}");
            }
        }
    }
}

/// The nine-term irradiance against the cosine-weighted integral of the studio's own radiance.
#[test]
fn irradiance_matches_the_cosine_integral() {
    for preset in StudioPreset::ALL {
        let env = StudioEnvironment::bake(preset);
        let integral = |n: [f64; 3]| {
            let (steps_t, steps_p) = (256, 512);
            let mut e = [0.0; 3];
            for i in 0..steps_t {
                let theta = PI * (f64::from(i) + 0.5) / f64::from(steps_t);
                for j in 0..steps_p {
                    let phi = 2.0 * PI * (f64::from(j) + 0.5) / f64::from(steps_p);
                    let d = [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()];
                    let cosine = dot(n, d);
                    if cosine <= 0.0 {
                        continue;
                    }
                    let w = theta.sin() * (PI / f64::from(steps_t)) * (2.0 * PI / f64::from(steps_p)) * cosine;
                    let l = env.source(d);
                    add(&mut e, l, w);
                }
            }
            e
        };
        let up = integral([0.0, 0.0, 1.0]);
        for n in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5, 0.7]] {
            let n = unit(n);
            let expected = integral(n);
            let got = env.irradiance_at(n);
            for k in 0..3 {
                // Nine terms keep a smooth lighting's irradiance; the error is a share of the
                // brightest (upward) irradiance.
                assert!((got[k] - expected[k]).abs() <= 0.1 * up[k], "{preset:?} n={n:?}: {got:?} against {expected:?} (up {up:?})");
            }
        }
    }
}

/// The sharpest level shows the studio as it is: a softbox's centre and the dark floor.
#[test]
fn the_sharpest_level_shows_the_studio() {
    let env = StudioEnvironment::bake(StudioPreset::Product);
    let (sa, ca) = 135.0_f64.to_radians().sin_cos();
    let (se, ce) = 40.0_f64.to_radians().sin_cos();
    for d in [[ca * ce, sa * ce, se], [0.2, 0.1, -0.97], [0.0, 1.0, 0.3]] {
        let expected = env.source(d);
        let got = env.prefiltered(d, 0.0);
        for k in 0..3 {
            assert!((got[k] - expected[k]).abs() <= 0.02 * expected[k] + 1e-3, "{d:?}: {got:?} against {expected:?}");
        }
    }
    assert!(env.prefiltered([ca * ce, sa * ce, se], 0.0)[0] > 10.0 * env.prefiltered([0.2, 0.1, -0.97], 0.0)[0], "the key light outshines the floor");
    // Turning the studio turns what a direction sees.
    let turned = unturn([ca * ce, sa * ce, se], 1.0);
    let back = unturn(turned, -1.0);
    assert!((0..3).all(|k| (back[k] - [ca * ce, sa * ce, se][k]).abs() < 1e-12));
    assert!(env.prefiltered(turned, 0.0)[0] < 0.5 * env.prefiltered([ca * ce, sa * ce, se], 0.0)[0], "turned away from the key light");
}

/// Every cube texel's direction maps back to the same texel.
#[test]
fn cube_faces_round_trip() {
    for face in 0..6 {
        for (s, t) in [(-0.9, -0.9), (0.0, 0.0), (0.5, -0.25), (0.9, 0.7)] {
            let (f, s2, t2) = direction_face(face_direction(face, s, t));
            assert_eq!(f, face);
            assert!((s - s2).abs() < 1e-9 && (t - t2).abs() < 1e-9, "face {face}: ({s}, {t}) -> ({s2}, {t2})");
        }
    }
}

#[test]
fn pbr_neutral_tone_map() {
    // Below the compression start, only the small offset that lifts the toe is subtracted.
    let mid = pbr_neutral([0.3, 0.4, 0.5]);
    for (got, x) in mid.iter().zip([0.3, 0.4, 0.5]) {
        assert!((got - (x - 0.04)).abs() < 1e-6, "{mid:?}");
    }
    assert_eq!(pbr_neutral([0.0; 3]), [0.0; 3]);
    let dark = pbr_neutral([0.05; 3]);
    assert!((dark[0] - 6.25 * 0.05 * 0.05).abs() < 1e-6, "{dark:?}");
    // Highlights roll off below one, keeping their order and their hue's lead channel.
    let mut previous = 0.0;
    for x in [0.5_f32, 0.8, 1.0, 2.0, 4.0, 16.0, 1000.0] {
        let y = pbr_neutral([x; 3])[0];
        assert!(y > previous && y < 1.0, "{x}: {y}");
        previous = y;
    }
    let warm = pbr_neutral([4.0, 1.0, 0.2]);
    assert!(warm[0] > warm[1] && warm[1] > warm[2] && warm[0] < 1.0, "{warm:?}");
    // Per-channel Reinhard greys a bright saturated colour more: its green/red ratio rises more.
    let reinhard = [4.0_f32, 1.0, 0.2].map(|c| c / (1.0 + c));
    assert!(warm[1] / warm[0] < reinhard[1] / reinhard[0], "{warm:?} against {reinhard:?}");
    assert_eq!(pbr_neutral([f32::NAN, -1.0, f32::INFINITY]), pbr_neutral([0.0; 3]));
}

/// The CPU twin of the shaders' `look_dither` (`viewport.wgsl` and the supersample filter).
fn dither(x: f32, y: f32) -> f32 {
    let noise = |p: (f32, f32)| (52.982_918_f32 * (p.0 * 0.06711056 + p.1 * 0.00583715).fract()).fract();
    noise((x, y)) + noise((y + 19.0, x + 47.0)) - 1.0
}

/// The dither is ±1 LSB, centred and triangular (variance 1/6, not a uniform's 1/3).
#[test]
fn dither_is_triangular_within_one_step() {
    let mut values = Vec::new();
    for y in 0..256 {
        for x in 0..256 {
            values.push(f64::from(dither(x as f32 + 0.5, y as f32 + 0.5)));
        }
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    assert!(values.iter().all(|v| v.abs() < 1.0));
    assert!(mean.abs() < 0.02, "mean {mean}");
    assert!((variance - 1.0 / 6.0).abs() < 0.03, "variance {variance}");
}

#[test]
fn half_floats() {
    assert_eq!(half(1.0), 0x3c00);
    assert_eq!(half(0.5), 0x3800);
    assert_eq!(half(65504.0), 0x7bff);
    assert_eq!(half(1e9), 0x7bff);
    assert_eq!(half(2.0_f32.powi(-24)), 1);
    assert_eq!(half(2.0_f32.powi(-14)), 0x0400);
    for bad in [0.0, -1.0, f32::NAN, f32::NEG_INFINITY] {
        assert_eq!(half(bad), 0);
    }
    for x in [0.001_f32, 0.1, 0.7, 3.3, 123.4, 4000.0] {
        let bits = half(x);
        let exponent = i32::from((bits >> 10) & 0x1f) - 15;
        let value = (1.0 + f32::from(bits & 0x3ff) / 1024.0) * 2.0_f32.powi(exponent);
        assert!((value - x).abs() <= x / 2048.0 + 1e-7, "{x}: {value}");
    }
}

/// What the renderer draws of a look: hostile values clamped or replaced, never passed through.
#[test]
fn looks_are_sanitized() {
    let look = ProductLook { exposure: 1.5, studio: StudioPreset::Contrast, rotation: 0.75, intensity: 0.6, ..ProductLook::default() };
    assert_eq!(look.sanitized(), look);
    assert_eq!(
        look.uniform(),
        [1.5, 0.6, 0.75, 1.0, ProductLook::DEFAULT_WEAVE_PITCH[0], ProductLook::DEFAULT_WEAVE_PITCH[1], ProductLook::DEFAULT_PLY_THICKNESS, 1.0]
    );
    let wild = ProductLook { exposure: f32::NAN, studio: StudioPreset::Soft, rotation: f32::INFINITY, intensity: 1e12, ..ProductLook::default() };
    assert_eq!(
        wild.sanitized(),
        ProductLook { exposure: 1.0, studio: StudioPreset::Soft, rotation: 0.0, intensity: ProductLook::MAX_SCALE, ..ProductLook::default() }
    );
    // Pattern sizes: zero or less is the default; others are clamped.
    let sized = ProductLook { weave_pitch: [0.0, f32::NAN], ply_thickness: 1e9, units_per_mm: -2.0, ..look }.sanitized();
    assert_eq!(sized.weave_pitch, ProductLook::DEFAULT_WEAVE_PITCH);
    assert_eq!((sized.ply_thickness, sized.units_per_mm), (ProductLook::PATTERN_RANGE[1], 1.0));
    let custom = ProductLook { weave_pitch: [0.42, 0.44], ply_thickness: 0.1, units_per_mm: 1000.0, ..look };
    assert_eq!(custom.sanitized(), custom);
    let odd = ProductLook { exposure: -3.0, rotation: -1.0, ..look }.sanitized();
    assert_eq!(odd.exposure, 0.0);
    assert!((odd.rotation - (std::f32::consts::TAU - 1.0)).abs() < 1e-6);
    for preset in StudioPreset::ALL {
        assert_eq!(StudioPreset::from_id(preset.id()), Some(preset));
    }
    assert_eq!(StudioPreset::from_id(99), None);
}

/// A bake in bounded steps (the web's way, a frame's share at a time) gives the same studio as one
/// bake all at once, and each step does about the work it is given (whole rows).
#[test]
fn a_stepped_bake_is_the_whole_bake() {
    let whole = StudioEnvironment::bake(StudioPreset::Contrast);
    assert!(StudioBake::new(StudioPreset::Contrast).finish().is_none(), "nothing before the bake is done");
    let mut bake = StudioBake::new(StudioPreset::Contrast);
    let mut steps = 0;
    while !bake.step(StudioBake::FRAME_WORK) {
        steps += 1;
        assert!(steps < 10_000, "the bake ends");
    }
    // The sharpest level and the rough ones in shares of a frame's work: many steps.
    assert!(steps > 100, "{steps} steps");
    assert!(bake.is_done() && bake.preset() == StudioPreset::Contrast);
    let Some(stepped) = bake.finish() else { panic!("done") };
    assert_eq!(stepped.uniform_bytes(), whole.uniform_bytes());
    assert_eq!(stepped.levels.len(), whole.levels.len());
    for (a, b) in stepped.levels.iter().zip(&whole.levels) {
        assert_eq!((a.size, &a.texels), (b.size, &b.texels));
    }
}

/// A compute probe around the production shader: the studio cube sampled along a direction, and
/// the actual `pbr_ibl` term.
fn probe_source() -> String {
    format!(
        "{}\n{}\n{}",
        crate::view::SHADER,
        crate::pbr::SOURCE,
        r#"
struct Probe { base: vec4<f32>, parameters: vec4<f32>, normal: vec4<f32>, view: vec4<f32>, };
@group(0) @binding(2) var<storage, read> probes: array<Probe>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_ibl(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = probes[id.x];
    if (p.parameters.w > 0.5) {
        result[id.x] = vec4<f32>(textureSampleLevel(studio_cube, studio_smp, p.view.xyz, p.normal.w).rgb, 1.0);
        return;
    }
    result[id.x] = vec4<f32>(pbr_ibl(p.base.xyz, p.parameters.xyz, p.normal.xyz, p.view.xyz, p.view.w), 1.0);
}
"#
    )
}

#[test]
fn probe_shader_is_valid() {
    let source = probe_source();
    let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
}

/// The shader's environment term on the GPU matches the CPU reference on the baked studio: the
/// cube's face orientation, the turn, the levels, the irradiance terms and the fit.
#[test]
fn gpu_environment_term_matches_the_cpu_reference() {
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let env = StudioEnvironment::bake(StudioPreset::Product);
    let furnace = StudioEnvironment::furnace(1.0);
    let source = probe_source();
    let module =
        device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("studio probe"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let probe_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[storage(2, true), storage(3, false)] });
    let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
    let studio_layout = layout(&device, wgpu::ShaderStages::COMPUTE);
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&probe_layout), Some(&empty), Some(&empty), Some(&studio_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("probe_ibl"),
        compilation_options: Default::default(),
        cache: None,
    });
    let sampler = sampler(&device);
    let (sa, ca) = 135.0_f64.to_radians().sin_cos();
    let (se, ce) = 40.0_f64.to_radians().sin_cos();
    let key = [ca * ce, sa * ce, se];
    // (base, metallic, roughness, ior, normal, view, rotation) and cube lookups (direction, level).
    let surfaces: Vec<([f64; 3], f64, f64, f64, [f64; 3], [f64; 3], f64)> = vec![
        ([0.95, 0.64, 0.54], 1.0, 0.25, 1.5, [0.0, 0.0, 1.0], unit([0.3, 0.2, 0.9]), 0.0),
        ([0.9, 0.9, 0.9], 1.0, 0.6, 1.5, unit([0.2, -0.3, 0.9]), unit([-0.4, 0.1, 0.8]), 1.1),
        ([0.06, 0.24, 0.07], 0.0, 0.3, 1.5, [0.0, 0.0, 1.0], unit([0.5, -0.5, 0.7]), 2.5),
        ([0.8, 0.8, 0.8], 0.0, 1.0, 1.0, unit([0.6, 0.0, 0.8]), unit([0.0, 0.0, 1.0]), 0.4),
        ([0.5, 0.5, 0.5], 0.5, 0.8, 2.0, unit([-0.5, 0.5, -0.7]), unit([-0.3, 0.3, -0.9]), 0.0),
    ];
    let lookups: Vec<([f64; 3], f64)> =
        vec![(key, 0.0), (unit([0.2, 0.1, -0.97]), 0.0), (unit([0.1, 0.9, 0.4]), 2.5), (unit([-0.2, -0.3, 0.93]), 5.0)];
    let mut input: Vec<f32> = Vec::new();
    for (base, metallic, roughness, ior, n, v, rotation) in &surfaces {
        input.extend(
            [base[0], base[1], base[2], 0.0, *metallic, *roughness, *ior, 0.0, n[0], n[1], n[2], 0.0, v[0], v[1], v[2], *rotation].map(|x| x as f32),
        );
    }
    for (d, lod) in &lookups {
        input.extend([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, *lod, d[0], d[1], d[2], 0.0].map(|x| x as f32));
    }
    let count = surfaces.len() + lookups.len();
    let run = |env: &StudioEnvironment| -> Vec<[f32; 4]> {
        let gpu = upload(&device, &queue, &studio_layout, &sampler, env);
        let probes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &input.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = (count * 16) as u64;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let probe_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &probe_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 2, resource: probes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: out.as_entire_binding() },
            ],
        });
        let empty_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &empty, entries: &[] });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &probe_bind, &[]);
            pass.set_bind_group(1, &empty_bind, &[]);
            pass.set_bind_group(2, &empty_bind, &[]);
            pass.set_bind_group(3, &gpu.bind, &[]);
            pass.dispatch_workgroups(count as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out, 0, &read, 0, size);
        queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let bytes = read.slice(..).get_mapped_range().map(|b| b.to_vec()).unwrap_or_default();
        bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| std::array::from_fn(|k| f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])))
            .collect()
    };
    for (env, name) in [(&env, "product studio"), (&furnace, "furnace")] {
        let got = run(env);
        assert_eq!(got.len(), count);
        for (i, (base, metallic, roughness, ior, n, v, rotation)) in surfaces.iter().enumerate() {
            let expected = shade(env, *base, *metallic, *roughness, *ior, *n, *v, *rotation);
            for k in 0..3 {
                let g = f64::from(got[i][k]);
                assert!((g - expected[k]).abs() <= 0.04 * expected[k] + 0.01, "{name}, surface {i}: {:?} against {expected:?}", got[i]);
            }
        }
        for (j, (d, lod)) in lookups.iter().enumerate() {
            let expected = env.prefiltered(*d, *lod);
            let g = got[surfaces.len() + j];
            for k in 0..3 {
                assert!((f64::from(g[k]) - expected[k]).abs() <= 0.03 * expected[k] + 0.005, "{name}, lookup {j} {d:?}: {g:?} against {expected:?}");
            }
        }
    }
}

/// The bake is bounded: its size is fixed, whatever the preset.
#[test]
fn bake_size_is_fixed() {
    let env = StudioEnvironment::bake(StudioPreset::Soft);
    let texels: usize = env.levels.iter().map(|l| l.texels.len()).sum();
    let expected: usize = (0..ENV_MIPS).map(|m| 6 * ((ENV_SIZE >> m) as usize).pow(2)).sum();
    assert_eq!(texels, expected);
    assert_eq!(env.preset(), StudioPreset::Soft);
    assert_eq!(env.uniform_bytes().len() as u64, ENV_UNIFORM_SIZE);
}

/// Run `entry` of the production viewport and PBR shaders plus `probe` (WGSL that reads
/// `probes: array<vec4<f32>>` at group 0 binding 2 and writes `result: array<vec4<f32>>` at
/// binding 3) once per `invocations`, with `studio` bound as group 3 when given.
fn run_probe(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    probe: &str,
    entry: &str,
    input: &[f32],
    invocations: usize,
    studio: Option<&StudioEnvironment>,
) -> Vec<[f32; 4]> {
    let source = format!("{}\n{}\n{}", crate::view::SHADER, crate::pbr::SOURCE, probe);
    let module =
        device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("look probe"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let probe_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[storage(2, true), storage(3, false)] });
    let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
    let studio_layout = layout(device, wgpu::ShaderStages::COMPUTE);
    let mut layouts = vec![Some(&probe_layout)];
    if studio.is_some() {
        layouts.extend([Some(&empty), Some(&empty), Some(&studio_layout)]);
    }
    let pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &layouts, immediate_size: 0 });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    });
    let probes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &input.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let size = (invocations * 16) as u64;
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let probe_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &probe_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 2, resource: probes.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: out.as_entire_binding() },
        ],
    });
    let empty_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &empty, entries: &[] });
    let sampler = sampler(device);
    let gpu = studio.map(|env| upload(device, queue, &studio_layout, &sampler, env));
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &probe_bind, &[]);
        if let Some(gpu) = &gpu {
            pass.set_bind_group(1, &empty_bind, &[]);
            pass.set_bind_group(2, &empty_bind, &[]);
            pass.set_bind_group(3, &gpu.bind, &[]);
        }
        pass.dispatch_workgroups(invocations as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out, 0, &read, 0, size);
    queue.submit([encoder.finish()]);
    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok();
    let bytes = read.slice(..).get_mapped_range().map(|b| b.to_vec()).unwrap_or_default();
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|c| std::array::from_fn(|k| f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])))
        .collect()
}

/// The last four floats of a PBR vertex written by `pbr_tri_with` (the detail the shader reads).
fn vertex_detail(surface: solidcraft_render::surface::OpaqueSurface, pattern: SurfacePattern, strength: f32) -> [f32; 4] {
    let mut scene = crate::view::GpuScene::default();
    scene.pbr_tri_with([0.0; 3], [0.0, 0.0, 1.0], surface, pattern, strength);
    assert!(scene.pbr.is_empty());
    assert_eq!(scene.pbr_detailed.len(), crate::view::PBR_DETAILED_SIZE);
    std::array::from_fn(|k| {
        let at = 48 + 4 * k;
        f32::from_le_bytes([scene.pbr_detailed[at], scene.pbr_detailed[at + 1], scene.pbr_detailed[at + 2], scene.pbr_detailed[at + 3]])
    })
}

/// The plain PBR vertex keeps its 48 bytes (the default preview's memory and upload bound are
/// unchanged), and carries no coat; `pbr_tri_with` writes 64-byte vertices into a stream of their
/// own, their first 48 bytes as `pbr_tri`'s, then the pattern and the coat, its IOR in the
/// pattern float's fraction.
#[test]
fn pbr_vertices_carry_pattern_and_coat() {
    use solidcraft_geom::Vec3;
    use solidcraft_render::{clear_coat::ClearCoat, surface::OpaqueSurface};
    assert_eq!((crate::view::PBR_SIZE, crate::view::PBR_DETAILED_SIZE), (48, 64));
    let Ok(plain) = OpaqueSurface::new(Vec3::new(0.2, 0.4, 0.6), 0.25, 0.5, 1.45) else { panic!("valid surface") };
    let Ok(coat) = ClearCoat::new(0.8, 0.12, 1.52) else { panic!("valid coat") };
    let mut scene = crate::view::GpuScene::default();
    scene.pbr_tri([1.0, 2.0, 3.0], [0.0, 0.0, 1.0], plain.with_clear_coat(coat));
    assert!(scene.pbr_detailed.is_empty());
    let floats: Vec<f32> = scene.pbr.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    assert_eq!(floats, [1.0, 2.0, 3.0, 0.0, 0.0, 1.0, 0.2, 0.4, 0.6, 0.25, 0.5, 1.45]);
    scene.pbr_tri_with([1.0, 2.0, 3.0], [0.0, 0.0, 1.0], plain, SurfacePattern::None, 0.0);
    let detailed: Vec<f32> = scene.pbr_detailed.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    assert_eq!(&detailed[..12], &floats[..]);
    assert_eq!(&detailed[12..], &[0.25, 0.0, 0.0, 1.0], "no pattern, no coat: what `vs_pbr` gives a plain vertex");
    assert!(crate::pbr::SOURCE.contains("vec4<f32>(0.25, 0.0, 0.0, 1.0)"), "vs_pbr's detail is the plain one");
    let detail = vertex_detail(plain.with_clear_coat(coat), SurfacePattern::PeelOverWeave, 0.4);
    assert_eq!(detail[0].floor(), 3.0);
    assert!((1.0 + (detail[0].fract() - 0.25) * 4.0 - 1.52).abs() < 1e-5, "{detail:?}");
    assert_eq!(&detail[1..], &[0.4, 0.8, 0.12]);
    for (strength, kept) in [(f32::NAN, 0.0), (-1.0, 0.0), (7.0, 1.0)] {
        assert_eq!(vertex_detail(plain, SurfacePattern::InkGrain, strength)[1], kept);
    }
    for pattern in SurfacePattern::ALL {
        assert_eq!(SurfacePattern::from_id(pattern.id()), Some(pattern));
    }
    assert_eq!(SurfacePattern::from_id(5), None);
}

/// The shader's pattern ids are [`SurfacePattern::id`].
#[test]
fn shader_pattern_ids_are_the_enum() {
    for (pattern, name) in [
        (SurfacePattern::Weave, "PATTERN_WEAVE"),
        (SurfacePattern::OrangePeel, "PATTERN_PEEL"),
        (SurfacePattern::PeelOverWeave, "PATTERN_PEEL_OVER_WEAVE"),
        (SurfacePattern::InkGrain, "PATTERN_INK"),
    ] {
        let text = format!("const {name}: i32 = {};", pattern.id());
        assert!(crate::pbr::SOURCE.contains(&text), "{text} is not in pbr::SOURCE");
    }
}

/// The viewport's clear coat (the actual `pbr_coated`, fed the vertex detail `pbr_tri_with`
/// writes) against the f64 surface oracle (`OpaqueSurface::with_clear_coat`), as the path
/// tracer's `gpu_clear_coat_brdf_matches_f64_surface_oracle` does, on any native GPU.
#[test]
fn gpu_viewport_clear_coat_matches_f64_surface_oracle() {
    use solidcraft_geom::Vec3;
    use solidcraft_render::{clear_coat::ClearCoat, surface::OpaqueSurface};
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_coat(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x * 5u;
    result[id.x] = vec4<f32>(pbr_coated(probes[b].xyz, probes[b + 1u].xyz, pbr_coat_of(probes[b + 2u]), vec3<f32>(0.0, 0.0, 1.0), probes[b + 3u].xyz, probes[b + 4u].xyz), 1.0);
}
"#;
    let base = Vec3::new(0.8, 0.5, 0.2);
    let pairs = [
        (Vec3::Z, Vec3::Z),
        (Vec3::new(0.5, 0.2, 1.0), Vec3::new(-0.4, 0.3, 0.8)),
        (Vec3::new(1.0, 0.0, 0.02), Vec3::new(-1.0, 0.1, 0.02)),
        (Vec3::new(0.0, 0.7, 0.7), Vec3::new(0.1, -0.7, 0.7)),
    ];
    let mut cases = Vec::new();
    for metallic in [0.0, 1.0] {
        for base_ior in [1.0, 1.5] {
            for roughness in [0.045, 0.12, 0.5] {
                for ior in [1.0, 1.5, 3.0] {
                    for weight in [0.5, 1.0] {
                        let (Ok(under), Ok(coat)) = (OpaqueSurface::new(base, metallic, 0.3, base_ior), ClearCoat::new(weight, roughness, ior))
                        else {
                            panic!("valid surface")
                        };
                        for (view, light) in pairs {
                            let (Some(view), Some(light)) = (view.normalized(), light.normalized()) else { panic!("unit") };
                            cases.push((under, under.with_clear_coat(coat), view, light));
                        }
                    }
                }
            }
        }
    }
    let mut input = Vec::new();
    for (under, coated, view, light) in &cases {
        input.extend([0.8, 0.5, 0.2, 0.0, under.metallic() as f32, under.roughness() as f32, under.dielectric_ior() as f32, 0.0]);
        input.extend(vertex_detail(*coated, SurfacePattern::None, 0.0));
        input.extend([view.x as f32, view.y as f32, view.z as f32, 0.0, light.x as f32, light.y as f32, light.z as f32, 0.0]);
    }
    let got = run_probe(&device, &queue, probe, "probe_coat", &input, cases.len(), None);
    assert_eq!(got.len(), cases.len());
    for ((under, coated, view, light), g) in cases.iter().zip(&got) {
        let Ok(expected) = coated.brdf(Vec3::Z, *view, *light) else { panic!("valid BRDF") };
        for (a, b) in g.iter().zip([expected.x, expected.y, expected.z]) {
            assert!(
                (f64::from(*a) - b).abs() <= 3e-3 * b.abs() + 2e-5,
                "m={}, base ior={}, coat {:?}, v={view:?}, l={light:?}: {a} vs {b}",
                under.metallic(),
                under.dielectric_ior(),
                coated.clear_coat()
            );
        }
    }
}

/// A clear coat in the studio: its own reflection plus the base it passes. White furnace: a
/// coated white surface still reflects exactly what it receives; and the actual WGSL term on the
/// baked studio matches the CPU reference.
#[test]
fn coated_environment_term_conserves_energy_and_matches_the_gpu() {
    let furnace = StudioEnvironment::furnace(1.0);
    let n = [0.0, 0.0, 1.0];
    for coat_roughness in [0.045, 0.2, 0.6] {
        for weight in [0.5, 1.0] {
            for nv in [0.05_f64, 0.4, 1.0] {
                let v = [(1.0 - nv * nv).sqrt(), 0.0, nv];
                for (metallic, ior) in [(1.0, 1.5), (0.0, 1.0), (0.0, 1.5)] {
                    let parts = shade_parts(&furnace, [1.0; 3], metallic, 0.4, ior, [weight, coat_roughness, 1.5], n, v, 0.3);
                    let total: Rgb = std::array::from_fn(|k| parts[0][k] + parts[1][k] + parts[2][k]);
                    assert!(total.iter().all(|c| (c - 1.0).abs() < 1e-3), "coat r={coat_roughness} w={weight} nv={nv} m={metallic}: {total:?}");
                    assert!(parts[1][0] > 0.0, "the coat reflects");
                }
                // A coat at IOR 1 is no coat.
                let plain = shade(&furnace, [0.3; 3], 0.0, 0.4, 1.5, n, v, 0.0);
                let matched = shade_parts(&furnace, [0.3; 3], 0.0, 0.4, 1.5, [weight, coat_roughness, 1.0], n, v, 0.0);
                assert!((0..3).all(|k| (matched[0][k] + matched[2][k] - plain[k]).abs() < 1e-12 && matched[1][k] == 0.0));
            }
        }
    }
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_coated_ibl(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x * 5u;
    let p = pbr_ibl_parts(probes[b].xyz, probes[b + 1u].xyz, probes[b + 2u].xyz, probes[b + 3u].xyz, probes[b + 4u].xyz, probes[b + 4u].w);
    result[id.x] = vec4<f32>(p.specular + p.coat + p.diffuse, 1.0);
}
"#;
    // (base, metallic, roughness, ior, coat, normal, view, rotation)
    type Case = (Rgb, f64, f64, f64, Rgb, Rgb, Rgb, f64);
    let cases: Vec<Case> = vec![
        ([0.06, 0.24, 0.07], 0.0, 0.6, 1.0, [1.0, 0.06, 1.52], [0.0, 0.0, 1.0], unit([0.3, 0.2, 0.9]), 0.0),
        ([0.06, 0.24, 0.07], 0.0, 0.6, 1.0, [1.0, 0.2, 1.52], unit([0.1, -0.2, 0.95]), unit([-0.5, 0.4, 0.6]), 1.2),
        ([0.95, 0.64, 0.54], 1.0, 0.25, 1.5, [0.5, 0.1, 1.5], [0.0, 0.0, 1.0], unit([0.6, -0.3, 0.7]), 2.0),
        ([0.5, 0.5, 0.5], 0.0, 0.8, 1.5, [1.0, 0.4, 2.0], unit([0.8, 0.0, 0.6]), unit([0.2, 0.1, 0.97]), 0.0),
    ];
    let mut input = Vec::new();
    for (base, metallic, roughness, ior, coat, n, v, rotation) in &cases {
        input.extend(
            [
                base[0], base[1], base[2], 0.0, *metallic, *roughness, *ior, 0.0, coat[0], coat[1], coat[2], 0.0, n[0], n[1], n[2], 0.0, v[0], v[1],
                v[2], *rotation,
            ]
            .map(|x| x as f32),
        );
    }
    for (env, name) in [(&StudioEnvironment::bake(StudioPreset::Product), "product studio"), (&furnace, "furnace")] {
        let got = run_probe(&device, &queue, probe, "probe_coated_ibl", &input, cases.len(), Some(env));
        assert_eq!(got.len(), cases.len());
        for ((base, metallic, roughness, ior, coat, n, v, rotation), g) in cases.iter().zip(&got) {
            let p = shade_parts(env, *base, *metallic, *roughness, *ior, *coat, *n, *v, *rotation);
            for k in 0..3 {
                let expected = p[0][k] + p[1][k] + p[2][k];
                assert!((f64::from(g[k]) - expected).abs() <= 0.04 * expected + 0.01, "{name}, coat {coat:?}: {g:?} against {expected}");
            }
        }
    }
}

/// The product look's patterns, through the actual `look_detail`: a weave with two yarn
/// families and over-under parity at the given pitch (zero mean, so it fades to the plain
/// colour), an edge's weave along the edge and up through the plies, a gloss coating's orange peel
/// (normal only), ink grain, and every pattern gone where its period is below the pixel.
#[test]
fn gpu_patterns_have_their_period_parity_and_mean() {
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_detail(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x * 5u;
    let d = look_detail(probes[b].xyz, probes[b + 1u].xyz, probes[b + 2u].xyz, probes[b + 3u], probes[b + 4u]);
    result[id.x] = vec4<f32>(d.n, d.albedo);
}
"#;
    let pitch = ProductLook::DEFAULT_WEAVE_PITCH;
    let ply = ProductLook::DEFAULT_PLY_THICKNESS;
    let params = |units: f32| [pitch[0], pitch[1], ply, units];
    // (position, normal, footprint, pattern, strength, units per mm)
    let mut cases: Vec<([f32; 3], [f32; 3], f32, SurfacePattern, f32, f32)> = Vec::new();
    let up = [0.0, 0.0, 1.0];
    // 0..4: parity and period of the weave on a face.
    let p0 = [0.8 * pitch[0], 0.5 * pitch[1], 0.0];
    for shift in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [2.0, 0.0]] {
        cases.push(([p0[0] + shift[0] * pitch[0], p0[1] + shift[1] * pitch[1], 0.0], up, 0.0, SurfacePattern::Weave, 1.0, 1.0));
    }
    // 4: the same point in world units of 0.1 mm (the pattern keeps its size in millimetres).
    cases.push(([p0[0] * 10.0, p0[1] * 10.0, 0.0], up, 0.0, SurfacePattern::Weave, 1.0, 10.0));
    // 5, 6: a footprint of one pitch fades the weave to its mean, and no pattern is no change.
    cases.push((p0, up, pitch[0], SurfacePattern::Weave, 1.0, 1.0));
    cases.push((p0, up, 0.0, SurfacePattern::None, 1.0, 1.0));
    // 7..10: an edge facing +X: unchanged along X, changed along Y and up through a ply.
    let e0 = [3.0, 0.3 * pitch[1], 0.6 * ply];
    for shift in [[0.0, 0.0, 0.0], [0.37, 0.0, 0.0], [0.0, pitch[1], 0.0], [0.0, 0.0, ply]] {
        cases.push(([e0[0] + shift[0], e0[1] + shift[1], e0[2] + shift[2]], [1.0, 0.0, 0.0], 0.0, SurfacePattern::Weave, 1.0, 1.0));
    }
    let grid = 24;
    let start = cases.len();
    // A grid over one full repeat (2 × 2 cells) of each pattern, then of the grain and peel.
    for pattern in [SurfacePattern::Weave, SurfacePattern::OrangePeel, SurfacePattern::PeelOverWeave, SurfacePattern::InkGrain] {
        for j in 0..grid {
            for i in 0..grid {
                let x = 2.0 * pitch[0] * (i as f32 + 0.5) / grid as f32 + 10.0 * pitch[0];
                let y = 2.0 * pitch[1] * (j as f32 + 0.5) / grid as f32 + 4.0 * pitch[1];
                cases.push(([x, y, 0.0], up, 0.0, pattern, 0.5, 1.0));
            }
        }
    }
    let mut input = Vec::new();
    for (p, n, footprint, pattern, strength, units) in &cases {
        input.extend([p[0], p[1], p[2], 0.0, n[0], n[1], n[2], 0.0, *footprint, *footprint, *footprint, 0.0]);
        input.extend([pattern.id() as f32 + 0.25, *strength, 0.0, 1.0]);
        input.extend(params(*units));
    }
    let got = run_probe(&device, &queue, probe, "probe_detail", &input, cases.len(), None);
    assert_eq!(got.len(), cases.len());
    let albedo = |i: usize| got[i][3];
    let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
    assert!(!close(albedo(0), albedo(1)), "the next cell along X has the other yarn on top: {} {}", albedo(0), albedo(1));
    assert!(close(albedo(0), albedo(2)), "a diagonal neighbour has the same yarn on top");
    assert!(close(albedo(0), albedo(3)), "the repeat is two cells");
    assert!((albedo(0) - albedo(4)).abs() < 1e-3, "units per millimetre keep the size");
    assert!(close(albedo(5), 1.0) && got[5][..3] == [0.0, 0.0, 1.0], "a pitch-wide footprint shows the mean: {:?}", got[5]);
    assert!(albedo(6) == 1.0 && got[6][..3] == [0.0, 0.0, 1.0], "no pattern");
    assert!(close(albedo(7), albedo(8)), "along an edge's normal nothing changes");
    assert!(!close(albedo(7), albedo(9)) || !close(albedo(7), albedo(10)), "an edge's weave runs along it and through the plies");
    assert!(got[7][0] > 0.99, "an edge's normal stays near its own: {:?}", got[7]);
    let block = |k: usize| &got[start + k * grid * grid..start + (k + 1) * grid * grid];
    let mean = |b: &[[f32; 4]]| b.iter().map(|g| f64::from(g[3])).sum::<f64>() / b.len() as f64;
    let spread = |b: &[[f32; 4]]| b.iter().map(|g| g[3]).fold(f32::MIN, f32::max) - b.iter().map(|g| g[3]).fold(f32::MAX, f32::min);
    let tilt = |b: &[[f32; 4]]| b.iter().map(|g| g[2]).fold(1.0, f32::min);
    let (weave, peel, over, grain) = (block(0), block(1), block(2), block(3));
    assert!((mean(weave) - 1.0).abs() < 0.01, "the weave's mean is the plain colour: {}", mean(weave));
    assert!(spread(weave) > 0.2, "the weave shows: {}", spread(weave));
    assert!(tilt(weave) < 0.99999 && tilt(weave) > 0.99, "a small relief on bare laminate: {}", tilt(weave));
    assert!(peel.iter().all(|g| g[3] == 1.0), "orange peel keeps the colour");
    // Gentle: a gloss coat's softbox reflection keeps its shape (slopes of a few milliradians).
    assert!(tilt(peel) < 0.9999995 && tilt(peel) > 0.99995, "orange peel tilts the normal a little: {}", tilt(peel));
    // Under a coating the weave shows in the colour only; the normal is the coating's own peel.
    assert!((mean(over) - 1.0).abs() < 0.01 && (spread(over) - spread(weave)).abs() < 1e-4, "weave shows through a coating");
    // The same peel at full strength (over the weave) and at half (on its own): twice the slope.
    assert!(over.iter().zip(peel).all(|(o, p)| o[2] <= p[2] + 1e-6) && tilt(over) < tilt(peel), "the coating's peel at full strength");
    assert!(spread(grain) > 0.05 && (mean(grain) - 1.0).abs() < 0.05, "ink grain: {} {}", spread(grain), mean(grain));
    assert!(got.iter().all(|g| g.iter().all(|c| c.is_finite()) && (g[0] * g[0] + g[1] * g[1] + g[2] * g[2] - 1.0).abs() < 1e-4));
}

/// The product look's PBR entry points translate to WebGL2's GLSL ES 3.00 (naga's GL backend,
/// as wgpu's on the web): the flat detail varying and the studio's cube map included, so the
/// look's shaders exist on WebGL2 (its four bind groups: the studio is group 3, the last).
#[test]
fn the_product_look_shaders_translate_for_webgl2() {
    use naga::back::glsl;
    let source = format!("{}\n{}", crate::view::SHADER, crate::pbr::SOURCE);
    let module = match naga::front::wgsl::parse_str(&source) {
        Ok(m) => m,
        Err(e) => panic!("{}", e.emit_to_string(&source)),
    };
    let info = match naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module) {
        Ok(i) => i,
        Err(e) => panic!("{e:?}"),
    };
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    for (stage, entry) in [
        (naga::ShaderStage::Vertex, "vs_pbr"),
        (naga::ShaderStage::Vertex, "vs_pbr_detailed"),
        (naga::ShaderStage::Fragment, "fs_pbr"),
        (naga::ShaderStage::Fragment, "fs_ao_depth_pbr"),
    ] {
        let pipeline = glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
        let mut out = String::new();
        let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
            .and_then(|mut w| w.write().map(|_| ()));
        assert!(written.is_ok(), "{entry}: {written:?}");
        assert!(out.starts_with("#version 300 es"), "{entry}");
        assert!(out.lines().any(|l| l.contains("flat") && l.contains("vec4")), "{entry}: the detail varying is flat");
        if entry == "fs_pbr" {
            assert!(out.contains("samplerCube"), "fs_pbr samples the studio's cube");
        }
    }
}
