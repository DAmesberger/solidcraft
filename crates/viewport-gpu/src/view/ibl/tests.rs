#![allow(clippy::needless_range_loop)] // Channels compared side by side.

use std::f64::consts::PI;

use super::reference::{brdf, shade, unturn};
use super::*;

fn rig_of(preset: StudioPreset) -> Arc<StudioRig> {
    studio(preset).unwrap_or_else(|| panic!("{preset:?} is registered"))
}

/// White furnace: in a studio of constant radiance 1 every level holds 1, the irradiance is π
/// in every direction, and a white metal (multiple scattering compensated), an index-matched
/// white diffuse surface and a white dielectric each reflect 1 at every roughness and angle.
#[test]
fn white_furnace() {
    let env = StudioEnvironment::furnace(1.0);
    assert_eq!(env.level_count(), environment::SPECULAR_LEVELS as usize);
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
                let [a, b] = brdf(nv, roughness);
                for (k, f0) in [0.9, 0.6, 0.3].into_iter().enumerate() {
                    let single = f0 * a + b;
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
    for preset in StudioPreset::all() {
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

/// The diffuse irradiance map against the cosine-weighted integral of the cyclorama's own radiance.
#[test]
fn irradiance_matches_the_cosine_integral() {
    for preset in StudioPreset::all() {
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
                // The angular discretization error is a share of the
                // brightest (upward) irradiance.
                assert!((got[k] - expected[k]).abs() <= 0.1 * up[k], "{preset:?} n={n:?}: {got:?} against {expected:?} (up {up:?})");
            }
        }
    }
}

/// The sharpest level shows the studio as it is: a softbox's centre and the dark floor.
#[test]
fn the_sharpest_level_shows_the_studio() {
    let env = StudioEnvironment::bake(StudioPreset::product());
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

/// A glossy lobe reads prefiltered levels only, never the unfiltered studio: level 0 is a GGX
/// lobe of roughness 0.1, not a mirror, and a coat of roughness 0.12 reads between it and level 1.
/// Read as `roughness × 5` with an unfiltered level 0, such a coat was 40 % mirror, whose sharp
/// edges a slight orange peel dragged into ragged blotches. Swept across the studio, level 0 is
/// smoother than the studio itself; the levels' roughnesses round-trip through the lookup.
#[test]
fn a_glossy_lobe_reads_no_unfiltered_studio() {
    let env = StudioEnvironment::bake(StudioPreset::product());
    let mirror = StudioEnvironment::bake_with_base(StudioPreset::product(), BaseLevel::Mirror);
    let top = f64::from(environment::SPECULAR_LEVELS - 1);
    for m in 1..environment::SPECULAR_LEVELS {
        let roughness = f64::from(m) / top;
        assert!((env.level_at(roughness) - f64::from(m)).abs() < 1e-12, "level {m}");
        assert_eq!(env.level_at(roughness), mirror.level_at(roughness));
    }
    assert_eq!(env.level_at(f64::from(BaseLevel::Preblurred.roughness())), 0.0);
    assert_eq!(env.level_at(0.05), 0.0);
    assert!((env.level_at(0.12) - 0.2).abs() < 1e-7);
    assert_eq!(mirror.level_at(0.12), 0.6);
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    // Sweeps around the studio at several elevations, a quarter degree a step: the largest step
    // between neighbours relative to the brightest value, of the studio and of level 0.
    let jump = |f: &dyn Fn([f64; 3]) -> f64| {
        let (mut most, mut peak) = (0.0_f64, 0.0_f64);
        for elevation in [0.05, 0.3, 0.55, 0.8, 1.2] {
            let at = |k: i32| {
                let a = (0.25 * f64::from(k)).to_radians();
                f(unit([a.cos(), a.sin(), elevation]))
            };
            let mut last = at(0);
            for k in 1..=1440 {
                let next = at(k);
                most = most.max((next - last).abs());
                peak = peak.max(next);
                last = next;
            }
        }
        most / peak.max(1e-9)
    };
    let studio = jump(&|d| luminance(env.source(d)));
    let mirror = jump(&|d| luminance(mirror.prefiltered(d, 0.0)));
    let level0 = jump(&|d| luminance(env.prefiltered(d, 0.0)));
    assert!(level0 < mirror, "preblurred {level0:.4}, mirror {mirror:.4}");
    eprintln!("largest step: source {studio:.4}, mirror {mirror:.4}, preblurred {level0:.4}");
    assert!(level0 < 0.8 * studio, "level 0 is filtered: {level0} against the studio's {studio}");
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

/// What the renderer draws of a look: hostile values clamped or replaced, never passed through.
#[test]
fn looks_are_sanitized() {
    let look = ProductLook { exposure: 1.5, studio: StudioPreset::contrast(), rotation: 0.75, intensity: 0.6, room: None };
    assert_eq!(look.sanitized(), look);
    assert_eq!(look.uniform(), [1.5, 0.6, 0.75, 1.0]);
    let wild = ProductLook { exposure: f32::NAN, studio: StudioPreset::soft(), rotation: f32::INFINITY, intensity: 1e12, room: None };
    assert_eq!(
        wild.sanitized(),
        ProductLook { exposure: 1.0, studio: StudioPreset::soft(), rotation: 0.0, intensity: ProductLook::MAX_SCALE, room: None }
    );
    let odd = ProductLook { exposure: -3.0, rotation: -1.0, ..look }.sanitized();
    assert_eq!(odd.exposure, 0.0);
    assert!((odd.rotation - (std::f32::consts::TAU - 1.0)).abs() < 1e-6);
    // A handle is its rig's content: the same rig registered again is the same studio, an edited
    // one another, and the default names none.
    assert_eq!(StudioPreset::register(&test_rig("Soft")), StudioPreset::soft());
    let mut brighter = test_rig("Soft");
    brighter.lights[0].radiance = brighter.lights[0].radiance.map(|c| c * 2.0);
    assert_ne!(StudioPreset::register(&brighter), StudioPreset::soft());
    assert!(StudioPreset::default().rig().is_none() && StudioPreset::soft().rig().is_some());
}

/// A bake in bounded steps (the web's way, a frame's share at a time) gives the same studio as one
/// bake all at once, and each step does about the work it is given (whole rows).
#[test]
fn a_stepped_bake_is_the_whole_bake() {
    let whole = StudioEnvironment::bake(StudioPreset::contrast());
    assert!(StudioBake::new(StudioPreset::contrast()).finish().is_none(), "nothing before the bake is done");
    let mut bake = StudioBake::new(StudioPreset::contrast());
    let mut steps = 0;
    while !bake.step(StudioBake::FRAME_WORK) {
        steps += 1;
        assert!(steps < 10_000, "the bake ends");
    }
    // The sharpest level and the rough ones in shares of a frame's work: many steps.
    assert!(steps > 100, "{steps} steps");
    assert!(bake.is_done() && bake.preset() == StudioPreset::contrast());
    let Some(stepped) = bake.finish() else { panic!("done") };
    assert_eq!(stepped.uniform_bytes(), whole.uniform_bytes());
    assert_eq!(stepped.levels.len(), whole.levels.len());
    let a = stepped.levels.iter().chain([&stepped.diffuse]);
    let b = whole.levels.iter().chain([&whole.diffuse]);
    for (a, b) in a.zip(b) {
        assert_eq!((a.width, &a.rgb), (b.width, &b.rgb));
    }
}

#[test]
fn bake_size_is_fixed() {
    let env = StudioEnvironment::bake(StudioPreset::soft());
    let texels: usize = env.levels.iter().map(|l| l.rgb.len()).sum();
    let expected: usize = (0..environment::SPECULAR_LEVELS).map(|m| (environment::SPECULAR_WIDTH >> m).pow(2) / 2).sum();
    assert_eq!(texels, expected);
    assert_eq!(env.preset(), StudioPreset::soft());
    assert_eq!(env.uniform_bytes().len() as u64, ENV_UNIFORM_SIZE);
}

#[test]
fn the_moody_studio_lights_tops_softly_from_above() {
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let env = StudioEnvironment::bake(StudioPreset::contrast());
    let at = |azimuth: f64, elevation: f64| {
        let (a, e) = (azimuth.to_radians(), elevation.to_radians());
        luminance(env.source([e.cos() * a.cos(), e.cos() * a.sin(), e.sin()]))
    };
    let up = luminance(env.source([0.0, 0.0, 1.0]));
    assert!((0.5..0.9).contains(&up), "straight up {up}");
    // Right, back-right, back and back-left: the mirror directions of views from the front half.
    for azimuth in [0.0, 45.0, 90.0, 135.0] {
        for elevation in [25.0, 29.0, 35.0, 41.0, 45.0] {
            let l = at(azimuth, elevation);
            assert!((0.7..0.9).contains(&l), "azimuth {azimuth}°, elevation {elevation}°: {l}");
        }
        for elevation in [0.0, 5.0, 10.0] {
            let l = at(azimuth, elevation);
            assert!(l < 0.45, "the wall stays dark: azimuth {azimuth}°, elevation {elevation}°: {l}");
        }
    }
}

#[test]
fn graded_lights_keep_their_calibrated_mean() {
    for preset in StudioPreset::all() {
        for light in rig_of(preset).lights.clone() {
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

#[test]
fn polygon_irradiance_matches_the_area_integral() {
    let product = test_rig("Product");
    let light = product.lights[0].clone();
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
    assert_eq!(product.direct_irradiance(behind, unit(light.normal().map(|c| -c))), [0.0; 3]);
}

#[test]
fn reflections_are_projected_against_the_room() {
    let room = solidcraft_viewport::studio::Room::default();
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

#[test]
fn no_direction_is_darker_than_a_tenth_of_the_mean() {
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    for preset in StudioPreset::all() {
        let env = StudioEnvironment::bake(preset);
        let mean = luminance(env.level_mean(0));
        let Some(level) = env.levels.first() else { panic!("baked") };
        let darkest = level.rgb.iter().map(|t| luminance(t.map(f64::from))).fold(f64::INFINITY, f64::min);
        assert!(darkest >= 0.1 * mean, "{preset:?}: darkest {darkest} against mean {mean}");
        let floor = luminance(env.source([0.0, 0.0, -1.0]));
        let band = luminance(env.source(unit([0.3, -1.0, 0.1])));
        assert!(floor < band, "{preset:?}: floor {floor}, band {band}");
        let key = luminance(env.source(unit(rig_of(preset).lights[0].centre)));
        // The soft studio's broad panels are the dimmest lights, at about four times the band.
        assert!(key > 3.0 * band, "{preset:?}: key {key}, band {band}");
    }
}

#[test]
fn white_under_the_key_is_calibrated_per_studio() {
    let luminance = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    for preset in StudioPreset::all() {
        let env = StudioEnvironment::bake(preset);
        let rig = rig_of(preset);
        let white = |n: [f64; 3]| {
            let mut e = rig.direct_irradiance([0.0; 3], n).map(|c| c * env.calibration());
            add(&mut e, env.irradiance_at(n), 1.0);
            e.map(|c| c / PI)
        };
        let key = white(unit(rig.lights[0].centre));
        let value = luminance(key);
        assert!((value - rig.white_under_key).abs() < 0.03 * rig.white_under_key, "{preset:?}: {value}");
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

#[test]
fn rooms_fit_the_scene() {
    let Some(room) = StudioRoom::around([0.0, 0.0, 0.0], [80.0, 50.0, 22.0]) else { panic!("a room") };
    assert_eq!(room.origin, [40.0, 25.0, 0.0]);
    assert_eq!(room.extent, 80.0);
    assert!((room.footprint[0] - (0.62 * 80.0 + 0.15 * 22.0)).abs() < 1e-4);
    for (min, max) in [([0.0; 3], [0.0; 3]), ([0.0; 3], [f32::NAN, 1.0, 1.0]), ([1.0; 3], [0.0; 3]), ([0.0; 3], [f32::INFINITY, 1.0, 1.0])] {
        assert!(StudioRoom::around(min, max).is_none(), "{min:?} {max:?}");
    }
    let look = ProductLook { room: Some(room), ..ProductLook::product() };
    assert_eq!(look.room_uniform(), [40.0, 25.0, 0.0, 80.0, room.footprint[0], room.footprint[1], 0.0, 0.0]);
    for wild in [
        StudioRoom { extent: -1.0, ..room },
        StudioRoom { extent: 1e-9, ..room },
        StudioRoom { extent: f32::INFINITY, ..room },
        StudioRoom { origin: [f32::NAN, 0.0, 0.0], ..room },
        StudioRoom { origin: [1e30, 0.0, 0.0], ..room },
        StudioRoom { footprint: [-1.0, 1.0], ..room },
    ] {
        let bad = ProductLook { room: Some(wild), ..ProductLook::product() };
        assert_eq!(bad.sanitized().room, None, "{wild:?}");
        assert_eq!(bad.room_uniform(), [0.0; 8], "{wild:?}");
    }
}

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
            let resources: Vec<_> = module.global_variables.iter().filter_map(|(_, variable)| variable.binding.as_ref()).collect();
            for slot in [0, 11, 12] {
                assert!(resources.iter().any(|b| b.group == 3 && b.binding == slot), "studio texture in slot {slot}");
            }
            assert!(out.contains("sampler2D"));
        }
    }
}
