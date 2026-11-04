//! The viewport's BSDF additions in `bsdf.wgsl` (docs/material-graph.md §5.5, track C2) on the GPU:
//! anisotropic GGX against `render::anisotropic_surface`, the coat colour against
//! `render::clear_coat::tinted_coat_brdf`, isotropic equivalence with today's GGX, white-furnace
//! bounds, the see-through fallback and the tangent, and the GLSL ES 3.00 translation.

use solidcore_record::Vec3;
use solidcraft_render::anisotropic_surface::{AnisotropicSurface, VisualFrame};
use solidcraft_render::clear_coat::{ClearCoat, tinted_brdf, tinted_coat_brdf};
use solidcraft_render::surface::OpaqueSurface;

use super::tests::evaluate_cases;

fn gpu_required() {
    assert!(std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none(), "a GPU is required here");
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / l)
}

fn v3(v: [f64; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn row(v: [f64; 3], w: f64) -> [f32; 4] {
    [v[0] as f32, v[1] as f32, v[2] as f32, w as f32]
}

/// A frame (n, t) and view/light pairs above it: head-on, oblique, along and across the tangent,
/// and near grazing.
fn frames() -> Vec<([f64; 3], [f64; 3])> {
    let n = unit([0.2, -0.3, 0.9]);
    let t = unit([n[2], 0.0, -n[0]]);
    vec![([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), ([0.0, 0.0, 1.0], unit([1.0, 1.0, 0.0])), (n, t)]
}

fn directions(n: [f64; 3], t: [f64; 3]) -> Vec<([f64; 3], [f64; 3])> {
    let b = [n[1] * t[2] - n[2] * t[1], n[2] * t[0] - n[0] * t[2], n[0] * t[1] - n[1] * t[0]];
    let at = |x: f64, y: f64, z: f64| unit(std::array::from_fn(|k| t[k] * x + b[k] * y + n[k] * z));
    vec![
        (at(0.0, 0.0, 1.0), at(0.0, 0.0, 1.0)),
        (at(0.5, 0.2, 1.0), at(-0.4, 0.3, 0.8)),
        (at(0.7, 0.0, 0.7), at(-0.6, 0.0, 0.8)),
        (at(0.0, 0.7, 0.7), at(0.0, -0.6, 0.8)),
        (at(0.6, -0.5, 0.6), at(-0.3, 0.5, 0.8)),
        (at(1.0, 0.1, 0.05), at(-1.0, 0.2, 0.1)),
    ]
}

fn close(a: f32, b: f64) -> bool {
    (f64::from(a) - b).abs() <= 3e-3 * b.abs() + 2e-5
}

/// `sc_aniso_base_brdf` is the f64 anisotropic oracle's BRDF (`AnisotropicSurface::brdf`, specular
/// plus the attenuated Lambert layer) for metals, dielectrics and an index-matched base, at equal
/// and unequal axes, along and across the tangent, in a tilted frame and near grazing.
#[test]
fn gpu_anisotropic_ggx_matches_the_f64_oracle() {
    let bases = [([0.8, 0.5, 0.2], 0.0, 1.5), ([0.95, 0.64, 0.54], 1.0, 1.5), ([0.3, 0.6, 0.9], 0.5, 1.0), ([0.9, 0.9, 0.9], 0.0, 2.4)];
    let axes = [(0.3, 0.3), (0.5, 0.2), (0.2, 0.6), (0.08, 0.4), (1.0, 0.1), (0.7, 0.9)];
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for (base, metallic, ior) in bases {
        for (rx, ry) in axes {
            for (n, t) in frames() {
                for (v, l) in directions(n, t) {
                    let surface = AnisotropicSurface::new(v3(base), metallic, rx, ry, ior).unwrap();
                    let frame = VisualFrame::new(v3(n), v3(t)).unwrap();
                    let e = surface.brdf(frame, v3(n), v3(v), v3(l)).unwrap();
                    expected.push(([e.x, e.y, e.z], format!("base {base:?} m={metallic} ior={ior} r=({rx}, {ry}) n={n:?} v={v:?} l={l:?}")));
                    inputs.extend([
                        row(base, metallic),
                        [(rx * rx) as f32, (ry * ry) as f32, ior as f32, 0.0],
                        row(n, 0.0),
                        row(t, 0.0),
                        row(v, 0.0),
                        row(l, 0.0),
                    ]);
                }
            }
        }
    }
    let body = r#"
    let k = id.x * stride;
    let p = inputs[k + 1u];
    o = vec4<f32>(sc_aniso_base_brdf(i.xyz, vec3<f32>(i.w, 0.5, p.z), p.xy, inputs[k + 2u].xyz, inputs[k + 3u].xyz, inputs[k + 4u].xyz, inputs[k + 5u].xyz), 0.0);
"#;
    let Some(got) = evaluate_cases(body, &inputs, 6) else { return gpu_required() };
    assert_eq!(got.len(), expected.len());
    for (g, (e, what)) in got.iter().zip(&expected) {
        assert!((0..3).all(|k| close(g[k], e[k])), "{what}: {g:?} vs {e:?}");
    }
}

/// With anisotropy 0 the viewport keeps today's GGX: `sc_layered_brdf` takes `sc_base_brdf` and
/// `sc_coated_brdf`'s exact path (bit for bit), as it does with anisotropy but no tangent, and
/// `sc_aniso_reflect` is the mirror direction bit for bit. The anisotropic lobe itself at equal
/// axes, `sc_aniso_alpha(r, 0)` = (r², r²), is today's GGX up to f32 rounding (a few ULPs: it
/// evaluates the same function through the local frame).
#[test]
fn gpu_isotropic_anisotropy_is_todays_ggx() {
    let mut inputs = Vec::new();
    let mut a2 = Vec::new();
    for roughness in [0.05_f64, 0.12, 0.3, 0.6, 1.0] {
        for metallic in [0.0, 1.0] {
            for (n, t) in frames() {
                for (v, l) in directions(n, t) {
                    a2.push(roughness.powi(4));
                    inputs.extend([[0.8, 0.5, 0.2, metallic], [roughness as f32, 1.5, 0.0, 0.0], row(n, 0.0), row(t, 0.0), row(v, 0.0), row(l, 0.0)]);
                }
            }
        }
    }
    let body = r#"
    let k = id.x * stride;
    let base = i.xyz;
    let r = inputs[k + 1u].x;
    let parameters = vec3<f32>(i.w, r, inputs[k + 1u].y);
    let n = inputs[k + 2u].xyz;
    let t = inputs[k + 3u].xyz;
    let v = inputs[k + 4u].xyz;
    let l = inputs[k + 5u].xyz;
    let coat = vec3<f32>(1.0, 0.1, 1.5);
    let today = sc_base_brdf(base, parameters, n, v, l);
    let lobe = sc_aniso_base_brdf(base, parameters, sc_aniso_alpha(r, 0.0), n, t, v, l);
    let alpha = sc_aniso_alpha(r, 0.0);
    // 1: every exact path is bit-identical.
    let exact = all(sc_layered_brdf(base, parameters, vec4<f32>(1.0), 0.0, t, coat, vec3<f32>(1.0), n, n, v, l) == sc_coated_brdf(base, parameters, coat, n, v, l))
        && all(sc_layered_brdf(base, parameters, vec4<f32>(1.0), 0.7, vec3<f32>(0.0), coat, vec3<f32>(1.0), n, n, v, l) == sc_coated_brdf(base, parameters, coat, n, v, l))
        && all(sc_layered_brdf(base, parameters, vec4<f32>(1.0), 0.0, t, vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.3), n, n, v, l) == today)
        && all(sc_aniso_reflect(n, t, v, r, 0.0) == 2.0 * max(dot(n, v), 0.0001) * n - v)
        && all(sc_aniso_reflect(n, vec3<f32>(0.0), v, r, 0.8) == 2.0 * max(dot(n, v), 0.0001) * n - v);
    let rel = abs(lobe - today) / max(abs(today), vec3<f32>(1e-30));
    o = vec4<f32>(select(0.0, 1.0, exact), max(rel.x, max(rel.y, rel.z)), abs(alpha.x - r * r) / (r * r), abs(alpha.y - r * r) / (r * r));
"#;
    let Some(got) = evaluate_cases(body, &inputs, 6) else { return gpu_required() };
    let worst = got.iter().map(|g| g[1]).fold(0.0_f32, f32::max);
    for (k, (g, a2)) in got.iter().zip(&a2).enumerate() {
        assert_eq!(g[0], 1.0, "case {k}: an exact path differs");
        // A few f32 ULPs (2⁻²³ ≈ 1.2e-7 each) through D, G and the Fresnel product; at a sharp
        // peak today's D rounds more, since its 1 − (n·h)² loses ~2.4e-7 against α² (the
        // anisotropic D reads h's tangential parts directly), so D squares that error over α².
        let tolerance = 4e-6 + 6e-7 / a2;
        assert!(f64::from(g[1]) <= tolerance, "case {k}: anisotropic lobe at equal axes is {} from today's GGX (α² = {a2})", g[1]);
        assert!(g[2] <= 2.4e-7 && g[3] <= 2.4e-7, "case {k}: sc_aniso_alpha(r, 0) is not r²: {g:?}");
    }
    eprintln!("largest relative difference of the isotropic anisotropic lobe from today's GGX: {worst:e}");
}

/// White furnace (directional albedo, ∫ f·cos dl, by deterministic multiple-importance quadrature
/// over cosine, the base's anisotropic NDF and the coat's NDF): a white surface under the
/// viewport's layered BRDF never reflects more than it receives (single scattering loses some at
/// high roughness), for anisotropy 0 to 1 and views along and across the tangent, with and without
/// a coat; a coloured coat only ever reflects less than a white one; an anisotropic white metal
/// keeps a sensible albedo.
#[test]
fn gpu_anisotropic_and_tinted_lobes_pass_the_white_furnace() {
    // (roughness, anisotropy, nv, azimuth, metallic, specular (weight, colour), coat (weight,
    // roughness), coat colour)
    let mut cases = Vec::new();
    for roughness in [0.045, 0.2, 0.5, 1.0] {
        for anisotropy in [0.0, 0.5, 0.9, 1.0] {
            for nv in [0.1_f64, 0.5, 1.0] {
                for azimuth in [0.0, std::f64::consts::FRAC_PI_2] {
                    for metallic in [0.0, 1.0] {
                        for specular in [[1.0; 4], [0.6, 1.0, 0.5, 0.2]] {
                            for (coat, colour) in
                                [([0.0, 1.0], [1.0; 3]), ([1.0, 0.1], [1.0; 3]), ([1.0, 0.1], [0.3, 0.6, 0.9]), ([0.5, 0.4], [0.3, 0.6, 0.9])]
                            {
                                cases.push((roughness, anisotropy, nv, azimuth, metallic, specular, coat, colour));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut inputs = Vec::new();
    for (roughness, anisotropy, nv, azimuth, metallic, specular, coat, colour) in &cases {
        let s = (1.0 - nv * nv).sqrt();
        inputs.extend([
            [*roughness as f32, *anisotropy as f32, *metallic as f32, 0.0],
            [(s * azimuth.cos()) as f32, (s * azimuth.sin()) as f32, *nv as f32, 0.0],
            [coat[0] as f32, coat[1] as f32, 1.5, 0.0],
            row(*colour, 0.0),
            specular.map(|x| x as f32),
        ]);
    }
    let body = r#"
    let k = id.x * stride;
    let r = i.x;
    let anisotropy = i.y;
    let parameters = vec3<f32>(i.z, r, 1.5);
    let v = inputs[k + 1u].xyz;
    let coat = inputs[k + 2u].xyz;
    let colour = inputs[k + 3u].xyz;
    let specular = inputs[k + 4u];
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let t = vec3<f32>(1.0, 0.0, 0.0);
    var alpha = vec2<f32>(r * r);
    if (anisotropy > 0.0) { alpha = sc_aniso_alpha(r, anisotropy); }
    let ac = max(coat.y * coat.y, 1e-4);
    let count = 2048u;
    var e = vec3<f32>(0.0);
    for (var s = 0u; s < count; s += 1u) {
        let u1 = (f32(s) + 0.5) / f32(count);
        let phi = 2.0 * sc_pi * f32(reverseBits(s)) / 4294967296.0;
        for (var lobe = 0u; lobe < 3u; lobe += 1u) {
            var l: vec3<f32>;
            if (lobe == 0u) {
                l = vec3<f32>(sqrt(u1) * cos(phi), sqrt(u1) * sin(phi), sqrt(1.0 - u1));
            } else {
                let a = select(alpha, vec2<f32>(ac), lobe == 2u);
                let h = sc_unit(vec3<f32>(a.x * sqrt(u1) * cos(phi), a.y * sqrt(u1) * sin(phi), sqrt(1.0 - u1)));
                l = 2.0 * dot(v, h) * h - v;
            }
            if (l.z <= 0.0) { continue; }
            let h = sc_unit(v + l);
            let vh = max(dot(v, h), 1e-7);
            let pdf = l.z / sc_pi + sc_aniso_ggx_d(h, alpha) * h.z / (4.0 * vh) + sc_aniso_ggx_d(h, vec2<f32>(ac)) * h.z / (4.0 * vh);
            let f = sc_layered_brdf(vec3<f32>(1.0), parameters, specular, anisotropy, t, coat, colour, n, n, v, l);
            e += f * l.z / pdf;
        }
    }
    e /= f32(count);
    o = vec4<f32>(e, 0.0);
"#;
    let Some(got) = evaluate_cases(body, &inputs, 5) else { return gpu_required() };
    assert_eq!(got.len(), cases.len());
    let mut worst = 0.0_f32;
    for (k, (case, g)) in cases.iter().zip(&got).enumerate() {
        let (_, _, nv, _, metallic, specular, coat, colour) = case;
        assert!(g[..3].iter().all(|c| c.is_finite() && *c >= 0.0), "{case:?}: {g:?}");
        worst = worst.max(g[0].max(g[1]).max(g[2]));
        assert!(g[..3].iter().all(|c| *c <= 1.005), "{case:?}: albedo {g:?} above one");
        // Single scattering with separable Smith loses up to ~60 % at roughness 1.
        if *metallic == 1.0 && *nv >= 0.5 && coat[0] == 0.0 && case.0 <= 0.5 && *specular == [1.0; 4] {
            assert!(g[..3].iter().all(|c| *c >= 0.5), "{case:?}: a white metal loses too much: {g:?}");
        }
        // The cases come in groups of four coats: none, white, tinted, tinted half coat. The
        // tinted coat reflects no more than the white one, channel by channel.
        if k % 4 == 2 {
            let white = got[k - 1];
            assert!((0..3).all(|c| g[c] <= white[c] + 1e-4), "{case:?}: tinted {g:?} above white {white:?}");
            assert!(g[0] < g[2] || *colour == [1.0; 3], "red is absorbed most: {g:?}");
        }
    }
    eprintln!("largest white-furnace albedo of the viewport's layered BRDF: {worst}");
}

/// The coat colour on the GPU (`sc_coat_layer` over `sc_base_brdf`) is the f64 oracle's tinted
/// coat, for full and partial coats, with and without an interface, and a white coat colour is
/// the untinted coat.
#[test]
fn gpu_coat_colour_matches_the_f64_oracle() {
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for (base, metallic) in [([0.8, 0.5, 0.2], 0.0), ([0.95, 0.64, 0.54], 1.0)] {
        for (weight, roughness, ior) in [(1.0, 0.12, 1.5), (0.5, 0.5, 1.5), (1.0, 0.3, 1.0), (0.7, 0.2, 2.2)] {
            for colour in [[1.0; 3], [0.10, 0.45, 0.12], [0.0, 0.5, 1.0]] {
                for (n, t) in frames() {
                    for (v, l) in directions(n, t) {
                        let surface = OpaqueSurface::new(v3(base), metallic, 0.35, 1.5)
                            .unwrap()
                            .with_clear_coat(ClearCoat::new(weight, roughness, ior).unwrap());
                        let e = tinted_coat_brdf(surface, v3(colour), v3(n), v3(v), v3(l)).unwrap();
                        expected.push((
                            [e.x, e.y, e.z],
                            format!("base {base:?} m={metallic} coat ({weight}, {roughness}, {ior}) {colour:?} v={v:?} l={l:?}"),
                        ));
                        inputs.extend([
                            row(base, metallic),
                            [weight as f32, roughness as f32, ior as f32, 0.0],
                            row(colour, 0.0),
                            row(n, 0.0),
                            row(v, 0.0),
                            row(l, 0.0),
                        ]);
                    }
                }
            }
        }
    }
    let body = r#"
    let k = id.x * stride;
    let n = inputs[k + 3u].xyz;
    let v = inputs[k + 4u].xyz;
    let l = inputs[k + 5u].xyz;
    let parameters = vec3<f32>(i.w, 0.35, 1.5);
    o = vec4<f32>(sc_coat_layer(sc_base_brdf(i.xyz, parameters, n, v, l), inputs[k + 1u].xyz, inputs[k + 2u].xyz, n, v, l), 0.0);
"#;
    let Some(got) = evaluate_cases(body, &inputs, 6) else { return gpu_required() };
    assert_eq!(got.len(), expected.len());
    for (g, (e, what)) in got.iter().zip(&expected) {
        assert!((0..3).all(|k| close(g[k], e[k])), "{what}: {g:?} vs {e:?}");
    }
}

/// The specular tint on the GPU (`sc_layered_brdf` with `specular` = (weight, colour), isotropic,
/// under no coat and under a tinted coat) is the f64 oracle's `tinted_brdf`; the anisotropic
/// branch of `sc_tinted_base_brdf` at equal axes along a tangent is the same lobe.
#[test]
fn gpu_specular_tint_matches_the_f64_oracle() {
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for (base, metallic, ior) in
        [([0.8, 0.5, 0.2], 0.0, 1.5), ([0.95, 0.64, 0.54], 1.0, 1.5), ([0.3, 0.6, 0.9], 0.5, 1.0), ([0.9, 0.9, 0.9], 0.3, 2.4)]
    {
        for (weight, colour) in [(1.0, [1.0, 0.5, 0.2]), (0.5, [1.0; 3]), (0.0, [1.0; 3]), (0.8, [0.2, 0.9, 0.6])] {
            for (coat, coat_colour) in [((0.0, 0.1, 1.5), [1.0; 3]), ((0.7, 0.2, 1.5), [0.1, 0.45, 0.12])] {
                for (n, t) in frames() {
                    for (v, l) in directions(n, t) {
                        let mut surface = OpaqueSurface::new(v3(base), metallic, 0.35, ior).unwrap();
                        if coat.0 > 0.0 {
                            surface = surface.with_clear_coat(ClearCoat::new(coat.0, coat.1, coat.2).unwrap());
                        }
                        let e = tinted_brdf(surface, weight, v3(colour), v3(coat_colour), v3(n), v3(v), v3(l)).unwrap();
                        expected.push((
                            [e.x, e.y, e.z],
                            format!(
                                "base {base:?} m={metallic} ior={ior} specular {weight} {colour:?} coat {coat:?} {coat_colour:?} v={v:?} l={l:?}"
                            ),
                        ));
                        inputs.extend([
                            row(base, metallic),
                            [weight as f32, colour[0] as f32, colour[1] as f32, colour[2] as f32],
                            [coat.0 as f32, coat.1 as f32, coat.2 as f32, ior as f32],
                            row(coat_colour, 0.0),
                            row(n, 0.0),
                            row(t, 0.0),
                            row(v, 0.0),
                            row(l, 0.0),
                        ]);
                    }
                }
            }
        }
    }
    let body = r#"
    let k = id.x * stride;
    let c = inputs[k + 2u];
    let parameters = vec3<f32>(i.w, 0.35, c.w);
    let specular = inputs[k + 1u];
    let n = inputs[k + 4u].xyz;
    let t = inputs[k + 5u].xyz;
    let v = inputs[k + 6u].xyz;
    let l = inputs[k + 7u].xyz;
    let layered = sc_layered_brdf(i.xyz, parameters, specular, 0.0, t, c.xyz, inputs[k + 3u].xyz, n, n, v, l);
    let along = sc_tinted_base_brdf(i.xyz, parameters, specular, vec2<f32>(0.35 * 0.35), n, t, v, l);
    let plain = sc_tinted_base_brdf(i.xyz, parameters, specular, vec2<f32>(0.35 * 0.35), n, vec3<f32>(0.0), v, l);
    let rel = abs(along - plain) / max(plain, vec3<f32>(1e-20));
    o = vec4<f32>(layered, max(rel.x, max(rel.y, rel.z)));
"#;
    let Some(got) = evaluate_cases(body, &inputs, 8) else { return gpu_required() };
    assert_eq!(got.len(), expected.len());
    for (g, (e, what)) in got.iter().zip(&expected) {
        assert!((0..3).all(|k| close(g[k], e[k])), "{what}: {g:?} vs {e:?}");
        assert!(g[3] <= 1e-4, "{what}: the anisotropic branch at equal axes is {} off", g[3]);
    }
}

/// The tangent is made perpendicular and turned counter-clockwise about the normal, a tangent
/// along the normal (or none) gives no direction; OpenPBR's roughness axes; the see-through
/// fallback's body colour and coverage; the bent reflection at full bend and without roughness;
/// a cut-out's premultiplied coverage (opacity 1 unchanged, below 0 nothing).
#[test]
fn gpu_tangent_alpha_see_through_and_bent_reflection_match_analytic_values() {
    let body = r#"
    let n = vec3<f32>(0.0, 0.0, 1.0);
    switch (id.x) {
        case 0u: { o = vec4<f32>(sc_aniso_tangent(n, vec3<f32>(2.0, 0.0, 1.0), 0.0), 0.0); }
        case 1u: { o = vec4<f32>(sc_aniso_tangent(n, vec3<f32>(1.0, 0.0, 0.0), sc_pi / 2.0), 0.0); }
        case 2u: { o = vec4<f32>(sc_aniso_tangent(n, vec3<f32>(0.0, 0.0, 3.0), 0.4) + sc_aniso_tangent(n, vec3<f32>(0.0), 0.4), 1.0); }
        case 3u: { o = vec4<f32>(sc_aniso_alpha(0.5, 0.5), sc_aniso_alpha(0.5, 1.0)); }
        case 4u: { o = sc_see_through(vec3<f32>(0.6), 0.0, vec3<f32>(0.2)); }
        case 5u: { o = sc_see_through(vec3<f32>(0.6), 1.0, vec3<f32>(1.0)); }
        case 6u: { o = sc_see_through(vec3<f32>(0.6), 0.5, vec3<f32>(1.0, 0.0, 0.0)); }
        case 7u: {
            // Fully bent (anisotropy 1, rough): the reflection about the plane across the long
            // axis t = x, (v.x, −v.y, v.z), instead of the mirror (−v.x, −v.y, v.z).
            let v = sc_unit(vec3<f32>(0.3, 0.4, 0.8));
            let r = sc_aniso_reflect(n, vec3<f32>(1.0, 0.0, 0.0), v, 0.5, 1.0);
            o = vec4<f32>(r - vec3<f32>(v.x, -v.y, v.z), length(r));
        }
        case 9u: { o = sc_cover(vec4<f32>(0.4, 0.6, 0.8, 0.9), 0.5); }
        case 10u: { o = sc_cover(vec4<f32>(0.4, 0.6, 0.8, 0.9), 1.0) + sc_cover(vec4<f32>(1.0), -1.0); }
        case 8u: {
            // A smooth surface is not bent.
            let v = sc_unit(vec3<f32>(0.3, 0.4, 0.8));
            o = vec4<f32>(sc_aniso_reflect(n, vec3<f32>(1.0, 0.0, 0.0), v, 0.0, 1.0) - (2.0 * dot(n, v) * n - v), 0.0);
        }
        default: {}
    }
"#;
    let Some(got) = evaluate_cases(body, &[[0.0; 4]; 11], 1) else { return gpu_required() };
    let near = |g: [f32; 4], e: [f64; 4]| g.iter().zip(e).all(|(a, b)| (f64::from(*a) - b).abs() < 1e-5);
    let at = 0.25 * (2.0 / 1.25_f64).sqrt();
    let at1 = 0.25 * 2.0_f64.sqrt();
    let expected: [[f64; 4]; 11] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
        [at, 0.5 * at, at1, 0.002025],
        [0.6, 0.6, 0.6, 1.0],
        [0.0, 0.0, 0.0, 0.0],
        [0.3 + 0.5 * (2.0 / 3.0), 0.3, 0.3, 1.0 - 0.5 / 3.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, 0.0],
        [0.2, 0.3, 0.4, 0.45],
        [0.4, 0.6, 0.8, 0.9],
    ];
    for (k, e) in expected.iter().enumerate() {
        assert!(near(got[k], *e), "case {k}: {:?} vs {e:?}", got[k]);
    }
}

/// Every C2 function translates to WebGL2's GLSL ES 3.00 inside a fragment entry.
#[test]
fn the_bsdf_additions_translate_for_webgl2() {
    use naga::back::glsl;
    let probe = r#"
@fragment
fn probe_c2(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>) -> @location(0) vec4<f32> {
    let un = sc_unit(n);
    let t = sc_aniso_tangent(un, sc_unit(p), p.x);
    let alpha = sc_aniso_alpha(p.y, p.z);
    var c = sc_layered_brdf(vec3<f32>(0.5), vec3<f32>(0.0, 0.4, 1.5), vec4<f32>(p.x, 0.5, 0.6, 0.7), p.z, t, vec3<f32>(1.0, 0.1, 1.5), vec3<f32>(0.2, 0.5, 0.2), un, sc_unit(n + p * 0.1), sc_unit(p + n), sc_unit(n - p));
    c += sc_aniso_base_brdf(vec3<f32>(0.5), vec3<f32>(1.0, 0.4, 1.5), alpha, un, t, sc_unit(p + n), un);
    c += sc_coat_layer(c, vec3<f32>(0.5, 0.2, 1.4), vec3<f32>(0.9), un, sc_unit(p + n), un) * sc_coat_tint(0.5, vec3<f32>(0.3));
    c += sc_aniso_reflect(un, t, sc_unit(p + n), p.y, p.z);
    c += sc_tinted_base_brdf(vec3<f32>(0.5), vec3<f32>(0.5, 0.4, 1.5), vec4<f32>(0.8, p), alpha, un, t, sc_unit(p + n), un);
    c += sc_tinted_fresnel(sc_specular_f0(c, 0.5, 1.5, vec4<f32>(p, 1.0)), sc_specular_f90(0.5, 1.5, vec4<f32>(p, 1.0)), p.x);
    let x = sc_aniso_ggx_d(un, alpha) + sc_aniso_smith_g1(un, alpha);
    let s = sc_see_through(c, p.x, vec3<f32>(0.5, 0.2, 0.1));
    return sc_cover(vec4<f32>(s.rgb + vec3<f32>(x), s.a), p.y);
}
"#;
    let source = format!("{}\n{probe}", super::library());
    let module = match naga::front::wgsl::parse_str(&source) {
        Ok(m) => m,
        Err(e) => panic!("{}", e.emit_to_string(&source)),
    };
    let info = match naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module) {
        Ok(i) => i,
        Err(e) => panic!("{e:?}"),
    };
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    let pipeline = glsl::PipelineOptions { shader_stage: naga::ShaderStage::Fragment, entry_point: "probe_c2".into(), multiview: None };
    let mut out = String::new();
    let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
        .and_then(|mut w| w.write().map(|_| ()));
    assert!(written.is_ok(), "{written:?}");
    for name in [
        "sc_layered_brdf",
        "sc_aniso_base_brdf",
        "sc_coat_layer",
        "sc_aniso_reflect",
        "sc_see_through",
        "sc_aniso_tangent",
        "sc_tinted_base_brdf",
        "sc_specular_f0",
        "sc_cover",
    ] {
        assert!(out.contains(name), "{name} is in the GLSL");
    }
}
