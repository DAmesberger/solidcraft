#![allow(clippy::needless_range_loop)] // Channels compared side by side.

use solidcraft_geom::prelude::*;
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

/// The nine-term irradiance against the cosine-weighted integral of the cyclorama's own radiance.
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
                    // The rig's lights reach diffuse surfaces directly; the terms hold the rest.
                    let l = env.ambient_source(d);
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
    // Toward the key light's centre (`STU-002`: front-left, above).
    let [ca, sa, se] = unit([-1.2, -1.2, 1.0]);
    let ce = 1.0;
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
    let look = ProductLook { exposure: 1.5, studio: StudioPreset::Contrast, rotation: 0.75, intensity: 0.6, room: None };
    assert_eq!(look.sanitized(), look);
    assert_eq!(look.uniform(), [1.5, 0.6, 0.75, 1.0]);
    let wild = ProductLook { exposure: f32::NAN, studio: StudioPreset::Soft, rotation: f32::INFINITY, intensity: 1e12, room: None };
    assert_eq!(
        wild.sanitized(),
        ProductLook { exposure: 1.0, studio: StudioPreset::Soft, rotation: 0.0, intensity: ProductLook::MAX_SCALE, room: None }
    );
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

/// The plain PBR vertex keeps its 48 bytes (the default preview's memory and upload bound are
/// unchanged): position, normal, base colour and optical parameters, and no coat.
#[test]
fn pbr_vertices_are_48_bytes_without_a_coat() {
    use solidcore_record::Vec3;
    use solidcraft_render::{clear_coat::ClearCoat, surface::OpaqueSurface};
    assert_eq!(crate::view::PBR_SIZE, 48);
    let Ok(plain) = OpaqueSurface::new(Vec3::new(0.2, 0.4, 0.6), 0.25, 0.5, 1.45) else { panic!("valid surface") };
    let Ok(coat) = ClearCoat::new(0.8, 0.12, 1.52) else { panic!("valid coat") };
    let mut scene = crate::view::GpuScene::default();
    scene.pbr_tri([1.0, 2.0, 3.0], [0.0, 0.0, 1.0], plain.with_clear_coat(coat));
    let floats: Vec<f32> = scene.pbr.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    assert_eq!(floats, [1.0, 2.0, 3.0, 0.0, 0.0, 1.0, 0.2, 0.4, 0.6, 0.25, 0.5, 1.45]);
}

/// The viewport's clear coat (the shared library's actual `sc_coated_brdf`, fed the coat's weight, roughness and
/// IOR) against the f64 surface oracle (`OpaqueSurface::with_clear_coat`), as the path
/// tracer's `gpu_clear_coat_brdf_matches_f64_surface_oracle` does, on any native GPU.
#[test]
fn gpu_viewport_clear_coat_matches_f64_surface_oracle() {
    use solidcore_record::Vec3;
    use solidcraft_render::{clear_coat::ClearCoat, surface::OpaqueSurface};
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_coat(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x * 5u;
    result[id.x] = vec4<f32>(sc_coated_brdf(probes[b].xyz, probes[b + 1u].xyz, probes[b + 2u].xyz, vec3<f32>(0.0, 0.0, 1.0), probes[b + 3u].xyz, probes[b + 4u].xyz), 1.0);
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
        let (weight, roughness, ior) = coated.clear_coat().map_or((0.0, 1.0, 1.0), |c| (c.weight() as f32, c.roughness() as f32, c.ior() as f32));
        input.extend([weight, roughness, ior, 0.0]);
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

/// The product look's PBR entry points translate to WebGL2's GLSL ES 3.00 (naga's GL backend,
/// as wgpu's on the web): the studio's cube map included, so the look's shaders exist on WebGL2
/// (its four bind groups: the studio is group 3, the last).
#[test]
fn the_product_look_shaders_translate_for_webgl2() {
    use naga::back::glsl;
    // The shared shading stands alone after the viewport's shader, for shaders that bring their
    // own material and entries.
    let shade = format!("{}\n{}", crate::view::SHADER, crate::pbr::SHADE);
    let parsed = naga::front::wgsl::parse_str(&shade).map_err(|e| e.emit_to_string(&shade));
    let shade_ok =
        parsed.map(|m| naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&m).is_ok());
    assert_eq!(shade_ok, Ok(true), "pbr::SHADE validates on its own");
    assert!(crate::pbr::SOURCE.starts_with(crate::pbr::SHADE));
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
    for (stage, entry) in
        [(naga::ShaderStage::Vertex, "vs_pbr"), (naga::ShaderStage::Fragment, "fs_pbr"), (naga::ShaderStage::Fragment, "fs_ao_depth_pbr")]
    {
        let pipeline = glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
        let mut out = String::new();
        let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
            .and_then(|mut w| w.write().map(|_| ()));
        assert!(written.is_ok(), "{entry}: {written:?}");
        assert!(out.starts_with("#version 300 es"), "{entry}");
        if entry == "fs_pbr" {
            assert!(out.contains("samplerCube"), "fs_pbr samples the studio's cube");
        }
    }
}

/// Each studio binds an upstream starter rig as exported: every light's centre, axes, size and
/// calibrated radiance are `assets/render/starter/lighting/exports/rigs.json`'s (positions and
/// sizes in the fixture's extent B = 1000 mm), and the width and height axes' cross product is the
/// export's emitting normal.
#[test]
fn the_bound_rigs_are_upstreams() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/render/starter/lighting/exports/rigs.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let export: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    let items = export["items"].as_array().cloned().unwrap_or_default();
    let vector = |v: &serde_json::Value| -> [f64; 3] { std::array::from_fn(|k| v[k].as_f64().unwrap_or(f64::NAN)) };
    let close = |a: [f64; 3], b: [f64; 3], tolerance: f64| (0..3).all(|k| (a[k] - b[k]).abs() <= tolerance);
    for preset in StudioPreset::ALL {
        let rig = studio(preset);
        assert_eq!(preset.rig_id(), rig.preset);
        let item = items.iter().find(|i| i["preset_id"] == rig.preset).unwrap_or_else(|| panic!("{} is exported", rig.preset));
        assert_eq!(item["state"], "authored_numeric_unbound", "{}: a complete numeric rig", rig.preset);
        let lights = item["lights"].as_array().cloned().unwrap_or_default();
        assert_eq!(lights.len(), rig.lights.len(), "{}: every light bound", rig.preset);
        assert!(rig.lights.len() <= rig::MAX_LIGHTS);
        let extent = item["fixture_extent_mm"].as_f64().unwrap_or(f64::NAN);
        for (light, exported) in rig.lights.iter().zip(&lights) {
            let name = format!("{} {}", rig.preset, light.id);
            assert_eq!(exported["id"], light.id, "{name}");
            assert_eq!(exported["type"], "rectangular_area", "{name}");
            assert_eq!(exported["emission_sides"], "front_only", "{name}");
            assert!(close(light.centre, vector(&exported["position_mm"]).map(|v| v / extent), 1e-12), "{name}: centre");
            assert!(close(light.width_axis, vector(&exported["width_axis"]), 1e-12), "{name}: width axis");
            assert!(close(light.height_axis, vector(&exported["height_axis"]), 1e-12), "{name}: height axis");
            assert!(close(light.normal(), vector(&exported["normal"]), 1e-9), "{name}: normal {:?}", light.normal());
            assert!((2.0 * light.half_width - exported["width_mm"].as_f64().unwrap_or(f64::NAN) / extent).abs() < 1e-12, "{name}: width");
            assert!((2.0 * light.half_height - exported["height_mm"].as_f64().unwrap_or(f64::NAN) / extent).abs() < 1e-12, "{name}: height");
            assert!(close(light.radiance, vector(&exported["emission"]["rgb"]), 1e-9), "{name}: radiance");
            assert!(rig::ROOM.contains(light.centre), "{name}: inside the room");
        }
    }
}

/// A light's graded profile keeps its calibrated mean: the radiance falls off toward the edges and
/// the rectangle's mean radiance is the rig's.
#[test]
fn graded_lights_keep_their_calibrated_mean() {
    for preset in StudioPreset::ALL {
        for light in studio(preset).lights {
            let steps = 400;
            let mut sum = 0.0;
            for i in 0..steps {
                for j in 0..steps {
                    let x = -1.0 + 2.0 * (f64::from(i) + 0.5) / f64::from(steps);
                    let y = -1.0 + 2.0 * (f64::from(j) + 0.5) / f64::from(steps);
                    sum += light.profile(x, y);
                }
            }
            let mean = sum / f64::from(steps * steps);
            assert!((mean - 1.0).abs() < 2e-3, "{preset:?} {}: mean {mean}", light.id);
            assert!(light.profile(0.999, 0.0) < 0.01 && light.profile(0.0, 0.0) > 1.0, "{preset:?} {}: graded", light.id);
        }
    }
}

/// Lambert's polygon irradiance against a direct integral over the rectangle (cos θ cos θ' / d²
/// dA), for a light in front of the surface, one that crosses its horizon and one behind it.
#[test]
fn polygon_irradiance_matches_the_area_integral() {
    let light = rig::PRODUCT.lights[0];
    let numeric = |p: [f64; 3], n: [f64; 3]| {
        let steps = 300;
        let ln = light.normal();
        let area = 4.0 * light.half_width * light.half_height;
        let mut e = 0.0;
        for i in 0..steps {
            for j in 0..steps {
                let a = -1.0 + 2.0 * (f64::from(i) + 0.5) / f64::from(steps);
                let b = -1.0 + 2.0 * (f64::from(j) + 0.5) / f64::from(steps);
                let x: [f64; 3] = std::array::from_fn(|k| {
                    light.centre[k] + light.width_axis[k] * a * light.half_width + light.height_axis[k] * b * light.half_height - p[k]
                });
                let d2 = dot(x, x);
                let w = unit(x);
                let (c, cl) = (dot(n, w), -dot(ln, w));
                if c > 0.0 && cl > 0.0 {
                    e += c * cl / d2 * area / f64::from(steps * steps);
                }
            }
        }
        e
    };
    let towards = unit(light.centre);
    for (p, n) in [
        ([0.0; 3], towards),
        ([0.3, -0.2, -0.4], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 0.0], [-1.0, 0.0, 0.0]),
        // The light crosses this surface's horizon.
        ([0.0, 0.0, 0.0], unit([0.0, -1.0, -0.6])),
        ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ] {
        let expected = numeric(p, n);
        let got = rig::polygon_irradiance(p, n, &light.corners());
        assert!((got - expected).abs() <= 0.01 * expected.max(0.01), "p={p:?} n={n:?}: {got} against {expected}");
    }
    assert_eq!(rig::polygon_irradiance([0.0; 3], [0.0, 0.0, -1.0], &light.corners()), 0.0, "below the horizon");
    // From behind the light gives nothing.
    let behind: [f64; 3] = std::array::from_fn(|k| light.centre[k] - 0.1 * light.normal()[k]);
    assert_eq!(rig::PRODUCT.direct_irradiance(behind, unit(light.normal().map(|c| -c))), [0.0; 3]);
}

/// Box projection: a ray leaves the room where it meets the nearest wall, so points across a flat
/// face look at different parts of the studio (a gradient, not one colour).
#[test]
fn reflections_are_projected_against_the_room() {
    let room = rig::ROOM;
    let hit = room.exit([0.5, 0.0, 0.0], [1.0, 0.0, 0.0]);
    assert!((hit[0] - room.half).abs() < 1e-9 && hit[1].abs() < 1e-9);
    let up = room.exit([0.0, 0.0, 0.0], unit([0.1, 0.2, 1.0]));
    assert!((up[2] - room.ceiling).abs() < 1e-9);
    let down = room.exit([0.2, 0.1, 0.0], unit([0.0, 0.0, -1.0]));
    assert!((down[2] - room.floor).abs() < 1e-9 && (down[0] - 0.2).abs() < 1e-9);
    // A point outside the room is clamped into it first.
    let outside = room.exit([9.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    assert!((outside[0] - room.half).abs() < 1e-9);
    // Along a flat mirror the same reflected direction reaches the walls at different heights.
    let r = unit([1.0, 0.0, 0.25]);
    let near = unit(room.exit([-0.5, 0.0, 0.0], r));
    let far = unit(room.exit([0.5, 0.0, 0.0], r));
    assert!((near[2] - far[2]).abs() > 0.02, "{near:?} {far:?}");
}

/// The cyclorama keeps every direction lit: no texel of the sharpest level is darker than a tenth
/// of the studio's mean radiance, so no face reflects black; the floor is darker than the horizon
/// band, and the lights outshine everything else.
#[test]
fn no_direction_is_darker_than_a_tenth_of_the_mean() {
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    for preset in StudioPreset::ALL {
        let env = StudioEnvironment::bake(preset);
        let mean = luminance(env.level_mean(0));
        let Some(level) = env.levels.first() else { panic!("baked") };
        let darkest = level.texels.iter().map(|t| luminance(t.map(f64::from))).fold(f64::INFINITY, f64::min);
        assert!(darkest >= 0.1 * mean, "{preset:?}: darkest {darkest} against mean {mean}");
        let floor = luminance(env.source([0.0, 0.0, -1.0]));
        let band = luminance(env.source(unit([0.3, -1.0, 0.1])));
        assert!(floor < band, "{preset:?}: floor {floor}, band {band}");
        let key = luminance(env.source(unit(studio(preset).lights[0].centre)));
        // The soft studio's broad panels are the dimmest lights, at about four times the band.
        assert!(key > 3.0 * band, "{preset:?}: key {key}, band {band}");
    }
}

/// The exposure is calibrated per studio: a white diffuse face at the studio's centre turned to the
/// key is lit (by the rig's lights directly and the cyclorama's irradiance terms) to
/// `WHITE_UNDER_KEY`, 0.80 on the display after PBR Neutral; and no white diffuse face, whichever
/// way it turns, reaches the tone curve's shoulder.
#[test]
fn white_under_the_key_is_calibrated_per_studio() {
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    for preset in StudioPreset::ALL {
        let env = StudioEnvironment::bake(preset);
        let rig = studio(preset);
        let white = |n: [f64; 3]| {
            let mut e = rig.direct_irradiance([0.0; 3], n).map(|c| c * env.calibration());
            add(&mut e, env.irradiance_at(n), 1.0);
            e.map(|c| c / PI)
        };
        let key = white(unit(rig.lights[0].centre));
        let value = luminance(key);
        assert!((value - rig::WHITE_UNDER_KEY).abs() < 0.03 * rig::WHITE_UNDER_KEY, "{preset:?}: {value}");
        let shown = pbr_neutral(key.map(|c| c as f32));
        let display = |c: f32| if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        assert!((display(shown[1]) - 0.8).abs() < 0.02, "{preset:?}: display {}", display(shown[1]));
        for i in 0..64 {
            let (z, a) = (-1.0 + 2.0 * (f64::from(i) + 0.5) / 64.0, f64::from(i) * 2.399);
            let n = [(1.0 - z * z).sqrt() * a.cos(), (1.0 - z * z).sqrt() * a.sin(), z];
            let c = white(n);
            assert!(c.iter().all(|v| *v < 0.76), "{preset:?}: a white face turned {n:?} reaches the shoulder: {c:?}");
        }
    }
}

/// The rooms fit to a scene's bounds and refuse nonsense; the look keeps a room only when it is
/// valid.
#[test]
fn rooms_fit_the_scene() {
    let Some(room) = StudioRoom::around([0.0, 0.0, 0.0], [80.0, 50.0, 22.0]) else { panic!("a room") };
    assert_eq!(room.origin, [40.0, 25.0, 0.0]);
    assert_eq!(room.extent, 80.0);
    assert!((room.footprint[0] - (0.62 * 80.0 + 0.15 * 22.0)).abs() < 1e-4);
    for (min, max) in [([0.0; 3], [0.0; 3]), ([0.0; 3], [f32::NAN, 1.0, 1.0]), ([1.0; 3], [0.0; 3]), ([0.0; 3], [f32::INFINITY, 1.0, 1.0])] {
        assert!(StudioRoom::around(min, max).is_none(), "{min:?} {max:?}");
    }
    let look = ProductLook { room: Some(room), ..ProductLook::default() };
    assert_eq!(look.room_uniform(), [40.0, 25.0, 0.0, 80.0, room.footprint[0], room.footprint[1], 0.0, 0.0]);
    for wild in [
        StudioRoom { extent: -1.0, ..room },
        StudioRoom { extent: 1e-9, ..room },
        StudioRoom { extent: f32::INFINITY, ..room },
        StudioRoom { origin: [f32::NAN, 0.0, 0.0], ..room },
        StudioRoom { origin: [1e30, 0.0, 0.0], ..room },
        StudioRoom { footprint: [-1.0, 1.0], ..room },
    ] {
        let bad = ProductLook { room: Some(wild), ..ProductLook::default() };
        assert_eq!(bad.sanitized().room, None, "{wild:?}");
        assert_eq!(bad.room_uniform(), [0.0; 8], "{wild:?}");
    }
}

/// The shader's direct rig lights (`pbr_rig_irradiance`) match the CPU's polygon irradiance on the
/// baked studio's uniform: corners, emitting side, horizon clipping and the calibrated radiance.
#[test]
fn gpu_rig_lights_match_the_cpu() {
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_rig(@builtin(global_invocation_id) id: vec3<u32>) {
    let q = probes[2u * id.x].xyz;
    let n = probes[2u * id.x + 1u].xyz;
    result[id.x] = vec4<f32>(pbr_rig_irradiance(q, n), 1.0);
}
"#;
    let cases: Vec<([f64; 3], [f64; 3])> = vec![
        ([0.0; 3], unit([-1.2, -1.2, 1.0])),
        ([0.3, -0.2, -0.4], [0.0, 0.0, 1.0]),
        ([0.0; 3], [-1.0, 0.0, 0.0]),
        ([0.1, 0.4, 0.2], unit([0.0, -1.0, -0.6])),
        ([0.0; 3], [0.0, 0.0, -1.0]),
        ([-0.4, 0.0, 0.1], unit([0.2, 0.9, 0.3])),
    ];
    for preset in StudioPreset::ALL {
        let env = StudioEnvironment::bake(preset);
        let input: Vec<f32> = cases.iter().flat_map(|(q, n)| [q[0], q[1], q[2], 0.0, n[0], n[1], n[2], 0.0]).map(|x| x as f32).collect();
        let got = run_probe(&device, &queue, probe, "probe_rig", &input, cases.len(), Some(&env));
        assert_eq!(got.len(), cases.len());
        for ((q, n), g) in cases.iter().zip(&got) {
            let expected = studio(preset).direct_irradiance(*q, *n).map(|c| c * env.calibration());
            for k in 0..3 {
                assert!((f64::from(g[k]) - expected[k]).abs() <= 2e-3 * expected[k] + 1e-4, "{preset:?} q={q:?} n={n:?}: {g:?} against {expected:?}");
            }
        }
    }
}

/// The studio term of a material graph's surface (track C2): an anisotropic base looks up its
/// bent reflection (`sc_aniso_reflect`) and a coloured coat tints the base. White furnace: a
/// white anisotropic metal or dielectric still reflects what it receives, exactly as the
/// isotropic one, whatever the bend (the split-sum albedo is the isotropic one); a white coat
/// colour is `pbr_ibl_parts`; a tinted coat reflects less than a white one, but at least its own
/// reflection. In the baked studio the bend changes what an anisotropic metal reflects.
#[test]
fn anisotropic_and_tinted_studio_terms_pass_the_white_furnace() {
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_layers(@builtin(global_invocation_id) id: vec3<u32>) {
    // Even invocations: the graph surface's term; odd: `pbr_ibl_parts` (isotropic, white coat).
    let b = (id.x / 2u) * 5u;
    let s = probes[b];
    let q = probes[b + 1u];
    let c = probes[b + 2u];
    let n = probes[b + 3u].xyz;
    let v = probes[b + 4u].xyz;
    let parameters = vec3<f32>(s.w, q.x, q.y);
    let coat = vec3<f32>(c.w, 0.2, 1.5);
    var p: PbrIbl;
    if (id.x % 2u == 0u) {
        let t = sc_aniso_tangent(n, vec3<f32>(1.0, 0.0, 0.0), q.w);
        p = pbr_ibl_layers(s.xyz, parameters, vec4<f32>(1.0), coat, c.xyz, n, n, v, sc_aniso_reflect(n, t, v, q.x, q.z), 0.3, vec4<f32>(t, q.z));
    } else {
        p = pbr_ibl_parts(s.xyz, parameters, coat, n, v, 0.3);
    }
    result[id.x] = vec4<f32>(p.specular + p.coat + p.diffuse, p.coat.x);
}
"#;
    // (base, metallic, roughness, ior, anisotropy, rotation, coat colour, coat weight, normal, view)
    type Case = (Rgb, f64, f64, f64, f64, f64, Rgb, f64, Rgb, Rgb);
    let mut cases: Vec<Case> = Vec::new();
    for (metallic, ior) in [(1.0, 1.5), (0.0, 1.5), (0.0, 1.0)] {
        for roughness in [0.2, 0.6, 1.0] {
            for anisotropy in [0.5, 1.0] {
                for (coat, weight) in [([1.0; 3], 0.0), ([1.0; 3], 1.0), ([0.3, 0.6, 0.9], 1.0), ([0.3, 0.6, 0.9], 0.5)] {
                    for (n, v) in [([0.0, 0.0, 1.0], unit([0.6, 0.3, 0.7])), (unit([0.2, -0.3, 0.9]), unit([-0.5, 0.4, 0.6]))] {
                        cases.push(([1.0; 3], metallic, roughness, ior, anisotropy, 0.7, coat, weight, n, v));
                    }
                }
            }
        }
    }
    let mut input = Vec::new();
    for (base, metallic, roughness, ior, anisotropy, rotation, coat, weight, n, v) in &cases {
        input.extend(
            [
                base[0],
                base[1],
                base[2],
                *metallic,
                *roughness,
                *ior,
                *anisotropy,
                *rotation,
                coat[0],
                coat[1],
                coat[2],
                *weight,
                n[0],
                n[1],
                n[2],
                0.0,
                v[0],
                v[1],
                v[2],
                0.0,
            ]
            .map(|x| x as f32),
        );
    }
    let furnace = StudioEnvironment::furnace(1.0);
    let got = run_probe(&device, &queue, probe, "probe_layers", &input, 2 * cases.len(), Some(&furnace));
    assert_eq!(got.len(), 2 * cases.len());
    for (k, case) in cases.iter().enumerate() {
        let (graph, today) = (got[2 * k], got[2 * k + 1]);
        let coat = case.6;
        if coat == [1.0; 3] {
            assert!((0..3).all(|c| (graph[c] - 1.0).abs() < 1e-3 && (graph[c] - today[c]).abs() < 1e-4), "{case:?}: {graph:?} against {today:?}");
        } else {
            assert!((0..3).all(|c| graph[c] < today[c] && graph[c] >= graph[3] - 1e-6), "{case:?}: tinted {graph:?} against white {today:?}");
            assert!(graph[0] < graph[2], "{case:?}: red is absorbed most: {graph:?}");
        }
    }
    // In the baked studio the bend moves an anisotropic metal's reflection.
    let studio = StudioEnvironment::bake(StudioPreset::Product);
    let got = run_probe(&device, &queue, probe, "probe_layers", &input, 2 * cases.len(), Some(&studio));
    let moved = cases.iter().enumerate().filter(|(k, c)| c.1 == 1.0 && (0..3).any(|j| (got[2 * k][j] - got[2 * k + 1][j]).abs() > 1e-3)).count();
    assert!(got.iter().all(|g| g.iter().all(|x| x.is_finite() && *x >= 0.0)), "{got:?}");
    assert!(moved > 0, "the bend changes no anisotropic metal");
}

/// The studio term with a specular tint (track C2): in the white furnace a white dielectric still
/// reflects exactly what it receives, whatever the tint (the diffuse layer takes what the weaker
/// specular leaves); a tinted white metal reflects less than an untinted one, its colour where
/// the tint is, and nothing at weight 0. The default tint is `pbr_ibl_parts` (to f32 rounding).
#[test]
fn the_specular_tint_in_the_studio_passes_the_white_furnace() {
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_tint(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x * 2u;
    let s = probes[b];
    let specular = probes[b + 1u];
    let parameters = vec3<f32>(s.x, s.y, s.z);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let v = sc_unit(vec3<f32>(0.5, 0.2, 0.8));
    let r = 2.0 * max(dot(n, v), 0.0001) * n - v;
    let p = pbr_ibl_layers(vec3<f32>(1.0), parameters, specular, vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(1.0), n, n, v, r, 0.3, vec4<f32>(0.0));
    let q = pbr_ibl_parts(vec3<f32>(1.0), parameters, vec3<f32>(0.0, 1.0, 1.0), n, v, 0.3);
    let d = abs((p.specular + p.diffuse) - (q.specular + q.diffuse)) / max(q.specular + q.diffuse, vec3<f32>(1e-20));
    result[id.x] = vec4<f32>(p.specular + p.diffuse, max(d.x, max(d.y, d.z)));
}
"#;
    // (metallic, roughness, ior, specular weight and colour)
    let mut cases: Vec<(f64, f64, f64, [f64; 4])> = Vec::new();
    for (metallic, ior) in [(0.0, 1.5), (0.0, 2.4), (1.0, 1.5)] {
        for roughness in [0.1, 0.5, 1.0] {
            for specular in [[1.0; 4], [0.5, 1.0, 1.0, 1.0], [1.0, 1.0, 0.5, 0.2], [0.0, 1.0, 1.0, 1.0]] {
                cases.push((metallic, roughness, ior, specular));
            }
        }
    }
    let input: Vec<f32> = cases.iter().flat_map(|(m, r, ior, s)| [*m, *r, *ior, 0.0, s[0], s[1], s[2], s[3]]).map(|x| x as f32).collect();
    let furnace = StudioEnvironment::furnace(1.0);
    let got = run_probe(&device, &queue, probe, "probe_tint", &input, cases.len(), Some(&furnace));
    assert_eq!(got.len(), cases.len());
    for (k, (case, g)) in cases.iter().zip(&got).enumerate() {
        let (metallic, _, _, specular) = case;
        if *specular == [1.0; 4] {
            // The same expressions; two inlined call sites may round differently under fast math.
            assert!(g[3] <= 1e-6, "{case:?}: the default tint is {} from pbr_ibl_parts", g[3]);
        }
        if *metallic == 0.0 {
            assert!((0..3).all(|c| (g[c] - 1.0).abs() < 1e-3), "{case:?}: a white dielectric reflects {g:?}");
        } else {
            let plain = got[k - k % 4];
            assert!((0..3).all(|c| g[c] <= plain[c] + 1e-6), "{case:?}: tinted metal {g:?} above {plain:?}");
            if specular[0] == 0.0 {
                assert!(g[..3].iter().all(|c| *c == 0.0), "{case:?}: weight 0 leaves {g:?}");
            }
            // The edge tint acts through the split sum's grazing term B, which the fit takes to 0
            // for a fully rough lobe seen this steeply.
            if specular[3] < 1.0 && case.1 < 1.0 {
                assert!(g[2] < g[0], "{case:?}: the edge tint is not blue-poor: {g:?}");
            }
        }
    }
}

/// The anisotropic studio lobe (`pbr_stretched_studio`) on the baked studio: sweeping the mirror
/// direction along the tangent, a brushed metal's reflection is smeared (it keeps much less detail
/// than the isotropic one's), while across the tangent it keeps about as much. Detail is the
/// summed second differences of the reflected luminance along short sweeps from 32 starting
/// directions, relative to the isotropic surface's.
#[test]
fn the_anisotropic_studio_lobe_spreads_along_the_tangent() {
    let Ok((_, device, queue)) = crate::open_headless() else { return };
    let probe = r#"
@group(0) @binding(2) var<storage, read> probes: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
@compute @workgroup_size(1)
fn probe_stretch(@builtin(global_invocation_id) id: vec3<u32>) {
    let q = probes[id.x];
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let r = sc_unit(q.xyz);
    let v = 2.0 * dot(n, r) * n - r;
    let parameters = vec3<f32>(1.0, 0.35, 1.5);
    let p = pbr_ibl_layers(vec3<f32>(1.0), parameters, vec4<f32>(1.0), vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(1.0), n, n, v, r, 0.0,
        select(vec4<f32>(0.0), vec4<f32>(1.0, 0.0, 0.0, 0.8), q.w > 0.5));
    let c = p.specular;
    result[id.x] = vec4<f32>(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 0.0, 1.0);
}
"#;
    const STEPS: usize = 24;
    // Sweeps of the mirror direction from (elevation, azimuth) starts, along x (the tangent) or y.
    let mut input = Vec::new();
    let mut sweeps = 0;
    for elevation in [25.0_f64, 40.0, 55.0, 70.0] {
        for azimuth in (0..8).map(|k| f64::from(k) * 45.0) {
            let (e, a) = (elevation.to_radians(), azimuth.to_radians());
            let start = [e.cos() * a.cos(), e.cos() * a.sin(), e.sin()];
            for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
                for stretched in [0.0, 1.0] {
                    for k in 0..STEPS {
                        let t = (k as f64 / (STEPS - 1) as f64 - 0.5) * 0.6;
                        let d: [f64; 3] = std::array::from_fn(|i| start[i] + t * axis[i]);
                        input.extend([d[0], d[1], d[2].max(0.05), stretched].map(|x| x as f32));
                    }
                }
            }
            sweeps += 1;
        }
    }
    let env = StudioEnvironment::bake(StudioPreset::Product);
    let got = run_probe(&device, &queue, probe, "probe_stretch", &input, input.len() / 4, Some(&env));
    assert!(got.iter().all(|g| g[0].is_finite() && g[0] >= 0.0));
    // Detail along a sweep: the summed second differences, which a blur along it removes first.
    let variation = |sweep: usize, axis: usize, stretched: usize| -> f64 {
        let base = ((sweep * 2 + axis) * 2 + stretched) * STEPS;
        let l = |k: usize| f64::from(got[base + k][0]);
        (1..STEPS - 1).map(|k| (l(k + 1) - 2.0 * l(k) + l(k - 1)).abs()).sum()
    };
    let total = |axis: usize, stretched: usize| -> f64 { (0..sweeps).map(|s| variation(s, axis, stretched)).sum() };
    let along = total(0, 1) / total(0, 0);
    let across = total(1, 1) / total(1, 0);
    eprintln!("anisotropic / isotropic variation: along the tangent {along:.3}, across {across:.3}");
    assert!(along < 0.75 * across, "the reflection is smeared along the tangent: {along:.3} against {across:.3} across");
    assert!(across > 0.85, "and not blurred across it: {across:.3}");
}

/// The tap calibration against the f64 anisotropic GGX oracle: at normal incidence, the half
/// width of the lobe's reflected specular along the tangent matches the taps' model — their own
/// lobe αl = max(αb, 0.36 αt), widened by their spread (`PBR_STRETCH_SCALE`, offsets of 0.75
/// steps, Gaussian weights) — within 15 %; across the tangent the taps' lobe is never wider than
/// the isotropic one's.
#[test]
fn the_anisotropic_studio_taps_match_the_oracle_lobe() {
    use solidcore_record::Vec3;
    use solidcraft_render::anisotropic_surface::{AnisotropicSurface, VisualFrame};
    assert!(crate::pbr::SOURCE.contains("const PBR_STRETCH_SCALE: f32 = 1.28;"));
    assert!(crate::pbr::SOURCE.contains("const PBR_STRETCH_FLOOR: f32 = 0.36;"));
    assert!(crate::pbr::SOURCE.contains("let o = (f32(k) - 2.0) * 0.75;"));
    let Ok(frame) = VisualFrame::new(Vec3::Z, Vec3::X) else { panic!("frame") };
    // The reflected angle (rad) along `axis` where the specular falls to half its peak.
    let half_width = |rx: f64, ry: f64, axis: Vec3| -> f64 {
        let Ok(s) = AnisotropicSurface::new(Vec3::new(1.0, 1.0, 1.0), 1.0, rx, ry, 1.5) else { panic!("surface") };
        let at = |theta: f64| s.specular_brdf(frame, Vec3::Z, Vec3::Z, axis * theta.sin() + Vec3::Z * theta.cos()).map_or(0.0, |f| f.x);
        let peak = at(0.0);
        let (mut lo, mut hi) = (0.0, 1.5);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if at(mid) > 0.5 * peak { lo = mid } else { hi = mid }
        }
        lo
    };
    let offsets = [-1.5_f64, -0.75, 0.0, 0.75, 1.5];
    let weights = offsets.map(|o| (-0.5 * o * o).exp());
    let deviation = (offsets.iter().zip(&weights).map(|(o, w)| w * o * o).sum::<f64>() / weights.iter().sum::<f64>()).sqrt();
    for (roughness, anisotropy) in [(0.3_f64, 0.5_f64), (0.35, 0.6), (0.35, 0.8), (0.4, 0.95), (0.5, 0.4)] {
        let alpha = roughness * roughness;
        let k = 1.0 - anisotropy;
        let at = alpha * (2.0 / (1.0 + k * k)).sqrt();
        let ab = (k * at).max(0.002025);
        let own = ab.max(0.36 * at);
        let along = half_width(at.sqrt(), ab.sqrt(), Vec3::X);
        let taps = half_width(own.sqrt(), own.sqrt(), Vec3::X);
        // A Gaussian's half width at half maximum is 1.1774 σ.
        let spread = 1.28 * (at * at - own * own).max(0.0).sqrt() * deviation;
        let model = (taps * taps + (1.1774 * spread).powi(2)).sqrt();
        assert!((model / along - 1.0).abs() <= 0.15, "r={roughness} a={anisotropy}: taps' half width {model:.4} against the oracle's {along:.4}");
        assert!(taps <= half_width(roughness, roughness, Vec3::X) + 1e-9, "r={roughness} a={anisotropy}: across, sharper than isotropic");
    }
}
