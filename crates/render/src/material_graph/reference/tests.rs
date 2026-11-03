//! The reference on the six fixtures, and analytic checks where closed forms exist.

use std::collections::BTreeMap;

use super::dual::{D, dot3, length3, unit};
use super::noise;
use super::patterns;
use super::*;
use crate::material_graph::{Constant, base_sample};

const FIXTURES: &[(&str, &str)] = &[
    ("black-epoxy-clear-coat", include_str!("../../../tests/fixtures/material_graph/black-epoxy-clear-coat.json")),
    ("brushed-nickel-silver", include_str!("../../../tests/fixtures/material_graph/brushed-nickel-silver.json")),
    ("electrolytic-sleeve", include_str!("../../../tests/fixtures/material_graph/electrolytic-sleeve.json")),
    ("laser-etched-nickel-silver", include_str!("../../../tests/fixtures/material_graph/laser-etched-nickel-silver.json")),
    ("pbr-copper", include_str!("../../../tests/fixtures/material_graph/pbr-copper.json")),
    ("solder-mask-over-copper", include_str!("../../../tests/fixtures/material_graph/solder-mask-over-copper.json")),
];

fn fixture(name: &str) -> &'static str {
    FIXTURES.iter().find(|f| f.0 == name).map(|f| f.1).unwrap_or_else(|| panic!("no fixture {name}"))
}

fn none() -> BTreeMap<String, Constant> {
    BTreeMap::new()
}

/// A graph from compact JSON nodes ending in a standard surface `s`.
fn graph(nodes: &str) -> (Ir, Vec<u32>) {
    let text = format!(r#"{{"name":"t","nodes":[{nodes}],"output":"s"}}"#);
    prepare_json(&text, &none()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn eval(ir: &Ir, words: &[u32], p: &Point) -> Evaluation {
    evaluate(ir, words, &[], p).unwrap_or_else(|e| panic!("{e}"))
}

/// The float value of the instruction that node `kind` lowered to (its first instruction).
fn first_of(ir: &Ir, e: &Evaluation, op: &str) -> f64 {
    let i = ir.instrs.iter().position(|i| i.op == op).unwrap_or_else(|| panic!("no {op}"));
    e.values[i].scalar().unwrap_or_else(|| panic!("{op} is not a float"))
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn assert_unit(v: [f64; 3], what: &str) {
    assert!(close(length3(v), 1.0, 1e-12), "{what} {v:?} is not unit");
}

/// Every fixture evaluates at every sample point to a finite surface inside the catalogue's
/// ranges, with unit normals, and the same point gives the same answer twice.
#[test]
fn every_fixture_evaluates_to_a_sane_surface() {
    for (name, text) in FIXTURES {
        let (ir, words) = prepare_json(text, &none()).unwrap_or_else(|e| panic!("{name}: {e}"));
        for p in sample_points() {
            let e = eval(&ir, &words, &p);
            let s = e.surface;
            assert!(s.to_floats().iter().all(|x| x.is_finite()), "{name}: {s:?}");
            assert_unit(s.normal, name);
            assert_unit(s.coat_normal, name);
            assert!((0.045..=1.0).contains(&s.roughness), "{name}: roughness {}", s.roughness);
            assert!(s.base_color.iter().all(|c| (0.0..=1.0).contains(c)), "{name}");
            if s.anisotropy == 0.0 {
                assert_eq!(s.tangent, [0.0; 3], "{name}: isotropic tangent");
            } else {
                assert_unit(s.tangent, name);
                assert!(dot3(s.tangent, s.normal).abs() < 1e-9, "{name}: tangent ⟂ normal");
            }
            assert_eq!(eval(&ir, &words, &p), e, "{name}: deterministic");
            assert_eq!(e.values.len(), ir.instrs.len());
        }
    }
}

/// A constant graph is its base response, at every point: the `pbr` look's surface exactly
/// (up to the `f32` rounding of the uniform words).
#[test]
fn the_pbr_fixture_is_its_base_response() {
    let text = fixture("pbr-copper");
    let (ir, words) = prepare_json(text, &none()).unwrap();
    let graph: MaterialGraph = serde_json::from_str(text).unwrap();
    let base = base_sample(&validate(&graph).unwrap()).unwrap();
    for p in sample_points() {
        let s = eval(&ir, &words, &p).surface;
        let o = base.opaque;
        let want = [o.base_color().x, o.base_color().y, o.base_color().z];
        for k in 0..3 {
            assert!(close(s.base_color[k], want[k], 1e-7), "{:?} vs {want:?}", s.base_color);
        }
        assert_eq!(s.metalness, 1.0);
        assert!(close(s.roughness, o.roughness(), 1e-7));
        assert!(close(s.ior, o.dielectric_ior(), 1e-7));
        assert_eq!((s.coat_weight, s.specular_weight, s.opacity, s.anisotropy), (0.0, 1.0, 1.0, 0.0));
        assert_eq!(s.normal, p.world_normal);
        assert_eq!(s.coat_normal, p.world_normal);
    }
}

/// Brushed nickel: roughness and tint are remaps of one brushed value, the base colour is the
/// constant times the tint, and the anisotropy runs along the planar u (+X) made tangential.
#[test]
fn brushed_nickel_silver_follows_its_remaps() {
    let (ir, words) = prepare_json(fixture("brushed-nickel-silver"), &none()).unwrap();
    for p in sample_points() {
        let e = eval(&ir, &words, &p);
        let v = first_of(&ir, &e, "pattern.brushed");
        assert!((0.0..=1.0).contains(&v));
        let s = e.surface;
        assert!(close(s.roughness, 0.41 + v * 0.08, 1e-6), "{} vs {v}", s.roughness);
        let tint = 0.97 + v * 0.06;
        for (k, c) in [0.80, 0.77, 0.70].iter().enumerate() {
            assert!(close(s.base_color[k], c * tint, 1e-6));
        }
        assert!(close(s.anisotropy, 0.6, 1e-7));
        let want = if p.world_normal[0].abs() > 0.999 { p.tangent } else { unit(dual::tangential([1.0, 0.0, 0.0], p.world_normal)) };
        for k in 0..3 {
            assert!(close(s.tangent[k], want[k], 1e-9), "{:?} vs {want:?} at {p:?}", s.tangent);
        }
    }
}

/// The sleeve: angle u (radians) past the stripe's edges is printed, before them plain PVC in
/// the interface colour; a look's value overrides the colour; an unbound print adds no ink.
#[test]
fn the_sleeve_stripe_and_interface_colour() {
    let (ir, words) = prepare_json(fixture("electrolytic-sleeve"), &none()).unwrap();
    let at = |angle: f64| Point::new([4.0 * angle.cos(), 4.0 * angle.sin(), 1.3], [angle.cos(), angle.sin(), 0.0]);
    let pvc = eval(&ir, &words, &at(0.4)).surface;
    for (k, c) in [0.020, 0.045, 0.16].iter().enumerate() {
        assert!(close(pvc.base_color[k], *c, 1e-7));
    }
    assert!(close(pvc.ior, 1.54, 1e-6) && close(pvc.roughness, 0.35, 1e-6));
    let printed = eval(&ir, &words, &at(3.0)).surface;
    for (k, c) in [0.80, 0.80, 0.78].iter().enumerate() {
        assert!(close(printed.base_color[k], *c, 1e-7));
    }
    // Halfway up the smoothstep the parameters are halfway.
    let mid = eval(&ir, &words, &at(2.61)).surface;
    assert!(close(mid.roughness, 0.40, 1e-4), "{}", mid.roughness);
    let values = BTreeMap::from([("sleeve".to_owned(), Constant::Vector(vec![0.5, 0.1, 0.05]))]);
    let (ir2, words2) = prepare_json(fixture("electrolytic-sleeve"), &values).unwrap();
    assert_eq!(ir2.shape, ir.shape, "a value is not a shape");
    let red = eval(&ir2, &words2, &at(0.4)).surface;
    assert!(close(red.base_color[0], 0.5, 1e-7) && close(red.base_color[2], 0.05, 1e-7));
}

/// The etched lid: no mark, plain metal everywhere; a mark's alpha selects the etched surface
/// with its pitted normal, read through the planar lid projection with v up.
#[test]
fn the_laser_etched_lid_reads_its_mark() {
    let (ir, words) = prepare_json(fixture("laser-etched-nickel-silver"), &none()).unwrap();
    let p = Point::new([1.13, 2.27, 0.0], [0.0, 0.0, 1.0]);
    let plain = eval(&ir, &words, &p).surface;
    assert!(close(plain.roughness, 0.45, 1e-6) && plain.normal == [0.0, 0.0, 1.0]);
    // A 2×2 mark: only its top-right texel (u ≥ 0.5, v ≥ 0.5) is opaque.
    let mut bytes = vec![0u8; 16];
    bytes[7] = 255; // texel (1, 0): right column, top row
    let mark = ReferenceTexture::from_rgba8(2, 2, &bytes).unwrap();
    let textures = [Some(&mark)];
    // uv = ((x + 6)/12, (y + 4)/8); a clamped bilinear sample at a texel centre is exact.
    let top_right = Point::new([0.75 * 12.0 - 6.0, 0.75 * 8.0 - 4.0, 0.0], [0.0, 0.0, 1.0]);
    let bottom_left = Point::new([0.25 * 12.0 - 6.0, 0.25 * 8.0 - 4.0, 0.0], [0.0, 0.0, 1.0]);
    let etched = evaluate(&ir, &words, &textures, &top_right).unwrap().surface;
    assert!(close(etched.roughness, 0.70, 1e-6) && close(etched.base_color[0], 0.30, 1e-7), "{etched:?}");
    assert_unit(etched.normal, "pits");
    assert!(etched.normal[2] < 1.0, "the pits tilt the normal");
    let clear = evaluate(&ir, &words, &textures, &bottom_left).unwrap().surface;
    assert!(close(clear.roughness, 0.45, 1e-6));
}

/// The solder mask: copper base under a tinted coat; the peel perturbs only the coat normal.
#[test]
fn solder_mask_peel_moves_only_the_coat_normal() {
    let (ir, words) = prepare_json(fixture("solder-mask-over-copper"), &none()).unwrap();
    let mut tilted = 0;
    for p in sample_points() {
        let s = eval(&ir, &words, &p).surface;
        assert_eq!(s.normal, p.world_normal);
        assert!(close(s.coat_weight, 1.0, 0.0) && close(s.coat_ior, 1.52, 1e-6));
        assert!(close(s.coat_color[1], 0.45, 1e-7));
        if dot3(s.coat_normal, p.world_normal) < 1.0 - 1e-12 {
            tilted += 1;
        }
        // 6 µm of relief over mm-scale noise tilts the coat by well under a degree.
        assert!(dot3(s.coat_normal, p.world_normal) > 0.9999);
    }
    assert!(tilted > 0, "the orange peel tilts the coat somewhere");
}

/// Black epoxy: the EDM roughness is the remapped 3D cellular noise of the object position.
#[test]
fn black_epoxy_roughness_is_its_remapped_noise() {
    let (ir, words) = prepare_json(fixture("black-epoxy-clear-coat"), &none()).unwrap();
    for p in sample_points() {
        let e = eval(&ir, &words, &p);
        let n = first_of(&ir, &e, "pattern.noise");
        // The scale arrives as the uniform word's f32.
        let q = p.object_position.map(|c| c / f64::from(0.04f32));
        let want = noise::fbm(2, 3, q, 11, 2, 2.0, 0.5);
        assert!(close(n, want, 1e-6), "{n} vs {want}");
        assert!(close(e.surface.roughness, 0.40 + 0.12 * n, 1e-6));
        assert_eq!(e.surface.ior, 1.0);
    }
}

/// A GPU readback round-trips through the WGSL field order.
#[test]
fn a_surface_round_trips_through_floats() {
    let (ir, words) = prepare_json(fixture("brushed-nickel-silver"), &none()).unwrap();
    let s = eval(&ir, &words, &sample_points()[1]).surface;
    let floats: Vec<f32> = s.to_floats().iter().map(|x| *x as f32).collect();
    let back = ReferenceSurface::from_floats(&floats).unwrap();
    let (field, d) = s.max_difference(&back);
    assert!(d < 1e-6, "{field}: {d}");
    assert!(ReferenceSurface::from_floats(&floats[1..]).is_err());
    let mut nan = back;
    nan.roughness = f64::NAN;
    assert_eq!(s.max_difference(&nan), ("roughness", f64::INFINITY));
}

// Analytic checks.

#[test]
fn value_noise_equals_its_hash_on_lattice_points() {
    for (x, y) in [(0, 0), (3, -7), (-12, 5)] {
        let p = [f64::from(x), f64::from(y)];
        assert_eq!(noise::value2(p, 9), noise::hash2(x, y, 9));
        let q = [f64::from(x), f64::from(y), 2.0];
        assert_eq!(noise::value3(q, 4), noise::hash3([x, y, 2], 4));
    }
    // The hash's documented range and periodicity.
    assert_eq!(noise::hash3([1, 2, 3], 5), noise::hash3([1 + 4096, 2, 3 - 4096], 5));
    for k in 0..200 {
        let h = noise::hash2(k, -k, 3);
        assert!((0.0..=1.0).contains(&h));
    }
}

#[test]
fn gradient_noise_is_half_on_lattice_points_and_cellular_zero_on_features() {
    assert!(close(noise::gradient_noise2([4.0, -2.0], 1), 0.5, 1e-12));
    assert!(close(noise::gradient_noise3([4.0, -2.0, 7.0], 1), 0.5, 1e-12));
    let feature = [3.0 + noise::hash2(3, 5, 2), 5.0 + noise::hash2(3, 5, 2 ^ 0x68e3_1da4)];
    assert!(noise::cellular2(feature, 2) < 1e-9);
    assert!(noise::cellular2([3.5, 5.5], 2) > 0.0);
}

#[test]
fn fbm_of_one_octave_is_the_noise() {
    let p = [0.37, 1.91, 0.0];
    for kind in 0..3 {
        assert_eq!(noise::fbm(kind, 2, p, 6, 1, 2.0, 0.5), noise::noise(kind, 2, p, 6));
    }
    // Two octaves weigh 1 and gain, normalised.
    let two = noise::fbm(0, 2, p, 6, 2, 2.0, 0.5);
    let want = (noise::value2([0.37, 1.91], 6) + 0.5 * noise::value2([0.74, 3.82], 7)) / 1.5;
    assert!(close(two, want, 1e-15));
}

#[test]
fn projections_have_closed_forms() {
    let p = Point::new([2.5, -1.0, 0.75], [0.0, 0.0, 1.0]);
    // Planar: u spans 2 mm along X, v 4 mm along Y.
    let (ir, words) = graph(
        r#"{"id":"t","kind":"texcoord.planar","params":{"space":"world"},"inputs":{"origin":[0.5,1,0],"u":[2,0,0],"v":[0,4,0]}},
           {"id":"s","kind":"standard_surface","inputs":{"base_color":{"from":"g"}}},
           {"id":"g","kind":"pattern.gradient","inputs":{"texcoord":{"from":"t"},"start":0,"end":2}}"#,
    );
    let e = eval(&ir, &words, &p);
    let Some(Value::Texcoord(t)) = e.values.iter().find(|v| matches!(v, Value::Texcoord(_))) else { panic!("no texcoord") };
    let (pl, w) = t.planes[0];
    assert_eq!(w, 1.0);
    assert!(close(pl.uv[0], 1.0, 1e-12) && close(pl.uv[1], -0.5, 1e-12), "{:?}", pl.uv);
    assert!(close(pl.gu[0], 0.5, 1e-12) && close(pl.gv[1], 0.25, 1e-12));
    // Linear gradient from 0 to 2 at u = 1: 0.5.
    assert!(close(first_of(&ir, &e, "pattern.gradient"), 0.5, 1e-7));

    // Cylindrical: arc length at radius 4 and height along the axis.
    let q = Point::new([3.0 * 0.3f64.cos(), 3.0 * 0.3f64.sin(), 2.0], [0.3f64.cos(), 0.3f64.sin(), 0.0]);
    let c = patterns::cylindrical(q.world_position.map(|x| D::new(x, [0.0; 3])), [0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], 4.0, true, &q);
    assert!(close(c.planes[0].0.uv[0], 1.2, 1e-12) && close(c.planes[0].0.uv[1], 2.0, 1e-12));

    // Triplanar weights: pure on an axis face, summing to one.
    assert_eq!(patterns::triplanar_weights([0.0, 0.0, 1.0], 4.0).map(|w| (w * 1e6).round()), [0.0, 0.0, 1e6]);
    let w = patterns::triplanar_weights([0.6, 0.0, 0.8], 2.0);
    assert!(close(w[0] + w[1] + w[2], 1.0, 1e-12) && close(w[0] / w[2], 0.36 / 0.64, 1e-6));
}

#[test]
fn a_transform_scales_rotates_and_offsets() {
    let p = Point::new([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let q = p.world_position.map(D::c);
    let mut q = q;
    q[0] = D::new(1.0, [1.0, 0.0, 0.0]);
    q[1] = D::new(0.0, [0.0, 1.0, 0.0]);
    let t = patterns::planar(q, [0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], &p);
    let r = patterns::transform(&t, [10.0, 20.0], [2.0, 3.0], std::f64::consts::FRAC_PI_2, &p);
    let pl = r.planes[0].0;
    // uv (1, 0) → scaled (2, 0) → rotated (0, 2) → offset (10, 22).
    assert!(close(pl.uv[0], 10.0, 1e-12) && close(pl.uv[1], 22.0, 1e-12), "{:?}", pl.uv);
    // u' = −3v, so the tangent turns to −Y.
    assert!(close(pl.tangent[1], -1.0, 1e-12), "{:?}", pl.tangent);
}

#[test]
fn checker_grid_and_fades_have_closed_forms() {
    let pl = |u: f64, v: f64, fp: f64| Plane { uv: [u, v], gu: [1.0, 0.0, 0.0], gv: [0.0, 1.0, 0.0], fp, tangent: [1.0, 0.0, 0.0] };
    assert!(close(patterns::checker(&pl(0.25, 0.25, 0.0), [1.0, 1.0]).value, 0.0, 1e-12));
    assert!(close(patterns::checker(&pl(1.25, 0.25, 0.0), [1.0, 1.0]).value, 1.0, 1e-12));
    assert!(close(patterns::checker(&pl(2.5, 1.5, 0.0), [2.0, 2.0]).value, 1.0, 1e-12));
    // A footprint of two cells has faded to exactly 0.5; half a cell leaves part of the contrast.
    assert!(close(patterns::checker(&pl(0.3, 0.7, 2.0), [1.0, 1.0]).value, 0.5, 1e-12));
    let soft = patterns::checker(&pl(0.25, 0.25, 0.3), [1.0, 1.0]).value;
    assert!(soft > 0.0 && soft < 0.5, "{soft}");
    // Next to an edge, the box filter's linear ramp: d = 0.05 cells, w = 0.2 → ±0.5 per axis.
    let edge = patterns::checker(&pl(0.95, 0.5, 0.2), [1.0, 1.0]).value;
    assert!(close(edge, 0.5 - 0.5 * 0.5 * 1.0 * patterns::fade(0.2, 1.0), 1e-6), "{edge}");
    // Grid: on a line, between lines, and its covered fraction far away.
    assert!(close(patterns::grid(&pl(2.0, 0.37, 0.0), [1.0, 1.0], 0.1).value, 1.0, 1e-12));
    assert!(close(patterns::grid(&pl(2.5, 0.37, 0.0), [1.0, 1.0], 0.1).value, 0.0, 1e-12));
    assert!(close(patterns::grid(&pl(2.5, 0.37, 10.0), [1.0, 1.0], 0.1).value, 1.0 - 0.9 * 0.9, 1e-12));
    // Fades: whole below 0.2 periods, gone above 0.6, a smoothstep between.
    assert_eq!(patterns::fade(0.1, 1.0), 1.0);
    assert_eq!(patterns::fade(0.7, 1.0), 0.0);
    assert!(close(patterns::fade(0.4, 1.0), 0.5, 1e-12));
    assert!(close(patterns::weave_fade(0.2, 1.0), 0.5, 1e-12));
    // A noise far beyond its fade is its mean.
    assert_eq!(patterns::noise2(&pl(0.3, 0.4, 5.0), 1.0, 2.0, 0.5, 1, 0, 3).value, 0.5);
    assert_eq!(patterns::brushed(&pl(0.3, 0.4, 5.0), 20.0, 2.0, 0.5, 0.3, 1).value, 0.5);
    assert_eq!(patterns::weave(&pl(0.3, 0.4, 5.0), [1.0, 1.0], 1.0, 7.0, 0.3, 0).value, 0.5);
}

#[test]
fn the_weave_crowns_and_alternates() {
    // A plain weave's warp is up at cell (0, 0) and the weft at (1, 0): the crown's top is 1.
    assert!(close(patterns::weave_raw([0.5, 0.5], 0, 1.0, 0.0, 0.0), 1.0, 1e-12));
    assert!(close(patterns::weave_raw([1.5, 0.5], 0, 1.0, 0.0, 0.0), 1.0, 1e-12));
    // Halfway between yarn axes the crown is lower.
    assert!(patterns::weave_raw([0.95, 0.95], 0, 1.0, 0.0, 0.0) < 0.7);
    // A flat crown leaves only the over-under factor: on the weft's axis, where it is up.
    assert!(close(patterns::weave_raw([0.5, 0.95], 0, 0.0, 0.0, 0.0), 1.0, 1e-12));
    // A 2/2 twill: the warp is up over two cells along a row, then down for two.
    let up = |x: f64| patterns::weave_raw([x, 0.5], 1, 0.0, 0.0, 0.0);
    assert!(close(up(0.5), 1.0, 1e-12) && close(up(1.5), 1.0, 1e-12));
    // Fibres darken between filaments by up to the fibre contrast, never above the yarn.
    let striped = patterns::weave_raw([0.5, 0.5 + 1.0 / 14.0], 0, 0.0, 7.0, 0.3);
    assert!(close(striped, 0.7, 1e-9), "{striped}");
}

#[test]
fn maths_nodes_follow_their_documented_rules() {
    let p = Point::new([0.0; 3], [0.0, 0.0, 1.0]);
    let run = |kind: &str, inputs: &str| {
        let (ir, words) = graph(&format!(
            r#"{{"id":"m","kind":"{kind}","inputs":{{{inputs}}}}},{{"id":"s","kind":"standard_surface","inputs":{{"emission_luminance":{{"from":"m"}}}}}}"#
        ));
        let e = eval(&ir, &words, &p);
        e.values.last().and_then(|v| v.num()).map(|n| n.c[0].v).unwrap_or(f64::NAN)
    };
    assert!(close(run("divide", r#""a":1,"b":0"#), 1e6, 1e-3));
    assert!(close(run("divide", r#""a":3,"b":-2"#), -1.5, 1e-7));
    assert_eq!(run("power", r#""a":-2,"b":2"#), 0.0);
    assert!(close(run("power", r#""a":3,"b":2"#), 9.0, 1e-6));
    assert_eq!(run("sqrt", r#""in":-4"#), 0.0);
    assert!(close(run("remap", r#""in":0.75,"in_min":0.5,"in_max":1,"out_min":10,"out_max":20"#), 15.0, 1e-6));
    assert!(close(run("remap", r#""in":3,"out_min":10,"out_max":20"#), 20.0, 1e-6), "clamped");
    assert_eq!(run("step", r#""edge":0.5,"in":0.5"#), 1.0);
    assert!(close(run("smoothstep", r#""edge0":0,"edge1":2,"in":1"#), 0.5, 1e-7));
    assert_eq!(run("smoothstep", r#""edge0":1,"edge1":1,"in":1"#), 0.0, "guarded, no NaN");
    assert!(close(run("clamp", r#""in":5,"low":2,"high":1"#), 1.0, 0.0), "min(max(x, low), high)");
    assert!(close(run("fract", r#""in":-0.25"#), 0.75, 1e-7));
    assert!(close(run("mix", r#""a":2,"b":4,"t":0.25"#), 2.5, 1e-7));
    assert!(close(run("invert", r#""in":0.25"#), 0.75, 1e-7));
    // normalize: +Z for zero; the luminance weights; hsv identity and a hue turn.
    let (ir, words) = graph(
        r#"{"id":"z","kind":"normalize","inputs":{"in":[0,0,0]}},{"id":"s","kind":"standard_surface","inputs":{"geometry_tangent":{"from":"z"}}}"#,
    );
    assert_eq!(eval(&ir, &words, &p).values[0].num().map(|n| n.values()), Some(vec![0.0, 0.0, 1.0]));
    assert_eq!(hsv_adjust([0.2, 0.5, 0.9], 0.0, 1.0, 1.0).map(|c| (c * 1e9).round()), [0.2, 0.5, 0.9].map(|c: f64| (c * 1e9).round()));
    let turned = hsv_adjust([1.0, 0.0, 0.0], std::f64::consts::TAU / 3.0, 1.0, 1.0);
    assert!(close(turned[1], 1.0, 1e-12) && close(turned[0], 0.0, 1e-12), "{turned:?}");
}

/// Out-of-range wires are clamped by the surface node and NaN takes the default.
#[test]
fn the_surface_clamps_and_maps_nan_to_defaults() {
    let (ir, words) = graph(
        r#"{"id":"big","kind":"multiply","inputs":{"a":3,"b":2}},
           {"id":"nan","kind":"sqrt","inputs":{"in":-1}},
           {"id":"zero","kind":"divide","inputs":{"a":0,"b":0}},
           {"id":"s","kind":"standard_surface","inputs":{"specular_roughness":{"from":"big"},"specular_ior":{"from":"nan"},"base_metalness":{"from":"zero"}}}"#,
    );
    let s = eval(&ir, &words, &Point::new([0.0; 3], [0.0, 0.0, 1.0])).surface;
    assert_eq!(s.roughness, 1.0);
    assert_eq!(s.ior, 1.0, "sqrt(max(−1, 0)) = 0, clamped to the range");
    assert_eq!(s.metalness, 0.0);
}

/// The chain rule against numeric differences of the whole evaluation: the height's gradient
/// that tilts the normal is the derivative of the height along the surface.
#[test]
fn gradients_match_numeric_differences() {
    let nodes = r#"{"id":"t","kind":"texcoord.planar","params":{"space":"world"},"inputs":{"u":[3,0,0],"v":[0,5,0]}},
        {"id":"g","kind":"pattern.gradient","inputs":{"texcoord":{"from":"t"},"start":-1,"end":2}},
        {"id":"r","kind":"pattern.gradient","params":{"kind":"radial"},"inputs":{"texcoord":{"from":"t"},"start":0,"end":1}},
        {"id":"a","kind":"multiply","inputs":{"a":{"from":"g"},"b":{"from":"r"}}},
        {"id":"b","kind":"sin","inputs":{"in":{"from":"a"}}},
        {"id":"c","kind":"power","inputs":{"a":{"from":"b"},"b":1.5}},
        {"id":"h","kind":"smoothstep","inputs":{"edge0":0,"edge1":0.8,"in":{"from":"c"}}},
        {"id":"n","kind":"normal.from_height","inputs":{"height":{"from":"h"},"relief_mm":0.5}},
        {"id":"s","kind":"standard_surface","inputs":{"geometry_normal":{"from":"n"}}}"#;
    let (ir, words) = graph(nodes);
    let h_index = ir.instrs.iter().position(|i| i.op == "smoothstep").unwrap();
    let height = |p: &Point| eval(&ir, &words, p).values[h_index].num().map(|n| n.c[0]).unwrap();
    let p = Point::new([0.4, 0.3, 0.0], [0.0, 0.0, 1.0]);
    let at = height(&p);
    assert!(at.v > 0.0 && at.v < 1.0, "inside the smoothstep: {}", at.v);
    for axis in 0..2 {
        let mut q = p;
        let mut r = p;
        q.world_position[axis] += 1e-6;
        r.world_position[axis] -= 1e-6;
        q.object_position = q.world_position;
        r.object_position = r.world_position;
        // The gradient patterns' slopes are forward differences of step 0.05 period, exact for a
        // linear ramp and close for the radial one, so compare loosely.
        let numeric = (height(&q).v - height(&r).v) / 2e-6;
        assert!(close(at.g[axis], numeric, 2e-2 * numeric.abs().max(1.0)), "axis {axis}: {} vs {numeric}", at.g[axis]);
    }
    let n = eval(&ir, &words, &p).surface.normal;
    let want = unit([-0.5 * at.g[0], -0.5 * at.g[1], 1.0]);
    for k in 0..3 {
        assert!(close(n[k], want[k], 1e-12));
    }
}

/// An object-space projection moves with the part; its gradients turn into world space.
#[test]
fn object_space_follows_the_placement() {
    let nodes = r#"{"id":"t","kind":"texcoord.planar","params":{"space":"object"},"inputs":{"u":[2,0,0],"v":[0,2,0]}},
        {"id":"g","kind":"pattern.gradient","inputs":{"texcoord":{"from":"t"},"start":0,"end":1}},
        {"id":"n","kind":"normal.from_height","inputs":{"height":{"from":"g"},"relief_mm":1}},
        {"id":"s","kind":"standard_surface","inputs":{"geometry_normal":{"from":"n"}}}"#;
    let (ir, words) = graph(nodes);
    let local = Point::new([0.8, 0.2, 0.0], [0.0, 0.0, 1.0]);
    // A quarter turn about Z: object +X is world +Y.
    let turned = local.placed([[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], [5.0, 5.0, 5.0]).unwrap();
    let a = eval(&ir, &words, &local);
    let b = eval(&ir, &words, &turned);
    let g = ir.instrs.iter().position(|i| i.op == "pattern.gradient").unwrap();
    assert_eq!(a.values[g].scalar(), b.values[g].scalar(), "the pattern stays on the part");
    // u = x/2 rises along object X: world Y once turned, so the normal leans to −Y.
    assert!(a.surface.normal[0] < 0.0 && a.surface.normal[1].abs() < 1e-12);
    assert!(b.surface.normal[1] < 0.0 && b.surface.normal[0].abs() < 1e-12, "{:?}", b.surface.normal);
    assert!(local.placed([[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]).is_err());
}

/// Images: sRGB decoded per texel, wraps as sampler address modes, the channel picks.
#[test]
fn images_sample_like_a_gpu_sampler() {
    let bytes = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255];
    let t = ReferenceTexture::from_rgba8(2, 2, &bytes).unwrap();
    // Texel centres: (0.25, 0.75) is the top-left texel.
    assert_eq!(patterns::sample(&t, [0.25, 0.75], 0, false), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(patterns::sample(&t, [0.75, 0.25], 0, false), [1.0, 1.0, 1.0, 1.0]);
    // Between the two top texels: half each.
    let mid = patterns::sample(&t, [0.5, 0.75], 1, false);
    assert!(close(mid[0], 0.5, 1e-12) && close(mid[1], 0.5, 1e-12));
    // Repeat wraps a whole period; mirror reflects it.
    assert_eq!(patterns::sample(&t, [1.25, 0.75], 0, false), patterns::sample(&t, [0.25, 0.75], 0, false));
    assert_eq!(patterns::sample(&t, [-0.25, 0.75], 2, false), patterns::sample(&t, [0.25, 0.75], 2, false));
    // sRGB: 128/255 decodes to about 0.2158.
    let grey = ReferenceTexture::from_rgba8(1, 1, &[128, 128, 128, 255]).unwrap();
    assert!(close(patterns::sample(&grey, [0.5, 0.5], 0, true)[0], 0.215_861, 1e-6));
    assert!(ReferenceTexture::from_rgba8(2, 2, &bytes[..12]).is_err());
}

/// A normal map's flat colour leaves the normal; a tilted one turns it in the texcoord's frame.
#[test]
fn a_normal_map_turns_in_the_texcoord_frame() {
    let nodes = r#"{"id":"t","kind":"texcoord.planar","params":{"space":"world"},"inputs":{"u":[0,1,0],"v":[-1,0,0]}},
        {"id":"i","kind":"image","params":{"space":"linear"},"inputs":{"texcoord":{"from":"t"},"file":{"texture":"0000000000000000000000000000000000000000000000000000000000000000"}}},
        {"id":"m","kind":"normal.map","inputs":{"color":{"from":"i"},"strength":1}},
        {"id":"s","kind":"standard_surface","inputs":{"geometry_normal":{"from":"m"}}}"#;
    let (ir, words) = graph(nodes);
    let p = Point::new([0.3, 0.3, 0.0], [0.0, 0.0, 1.0]);
    let flat = ReferenceTexture { width: 1, height: 1, texels: vec![[0.5, 0.5, 1.0, 1.0]] };
    let s = evaluate(&ir, &words, &[Some(&flat)], &p).unwrap().surface;
    assert_eq!(s.normal, [0.0, 0.0, 1.0]);
    // +x in tangent space: the planar u runs along world +Y here.
    let tilt = ReferenceTexture { width: 1, height: 1, texels: vec![[1.0, 0.5, 0.5, 1.0]] };
    let s = evaluate(&ir, &words, &[Some(&tilt)], &p).unwrap().surface;
    assert!(close(s.normal[1], 1.0, 1e-12), "{:?}", s.normal);
}

/// Triplanar patterns blend per plane; on an axis face only that plane shows.
#[test]
fn triplanar_blends_its_planes() {
    let nodes = r#"{"id":"t","kind":"texcoord.triplanar","params":{"space":"world"},"inputs":{"scale_mm":[1,1,1],"sharpness":4}},
        {"id":"w","kind":"pattern.weave","inputs":{"texcoord":{"from":"t"},"pitch_mm":[0.5,0.7]}},
        {"id":"s","kind":"standard_surface","inputs":{"base_metalness":{"from":"w"}}}"#;
    let (ir, words) = graph(nodes);
    assert!(ir.instrs.iter().any(|i| i.triplanar));
    let top = Point::new([0.31, 0.47, 0.0], [0.0, 0.0, 1.0]);
    let e = eval(&ir, &words, &top);
    let w = first_of(&ir, &e, "pattern.weave");
    let plane = Plane { uv: [0.31, 0.47], gu: [1.0, 0.0, 0.0], gv: [0.0, 1.0, 0.0], fp: 0.0, tangent: [1.0, 0.0, 0.0] };
    let want = patterns::weave(&plane, [0.5, 0.7], 1.0, 7.0, 0.3, 0).value;
    assert!(close(w, want, 1e-6), "{w} vs {want}");
    assert!(close(e.surface.metalness, w, 0.0));
}
