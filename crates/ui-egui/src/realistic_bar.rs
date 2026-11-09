//! The Realistic view's acceptance bar (docs/material-graph.md §5.9), shared by the golden-frame
//! harness (`viewport_golden_tests`) and the lighting review (`gpu_lighting_review`): one set of
//! rules over one metrics shape.
//!
//! The metrics a tool hands in, as JSON:
//!
//! - `parts`: `{part, pixels, clipped, crushed, median, p5, p95, p99}` per part;
//! - `faces`: `{part, face, pixels, median, p5, p95, span, planar, metallic, albedo, width}` per
//!   face of at least 400 pixels, `albedo` the base colour's linear luminance and `width` the
//!   face's width ([`width`]: twice its area over its perimeter) over the studio room's extent B.
//!
//! The rules:
//!
//! - **Clipped and crushed:** at most 1 % of each part's pixels at luma ≥ 254, and at most 1 % at
//!   ≤ 5, for every material.
//! - **Face medians:** every face's median in [20, 235], except for *dark* materials (a
//!   dielectric whose base colour has a linear luminance under [`DARK_ALBEDO`]), which are judged
//!   relative to their own albedo instead: no crushed pixels (above), a visible coat or specular
//!   highlight (the part's 99th percentile at least [`HIGHLIGHT`] above its median), and faces
//!   that read apart (their medians spread at least [`DISTINCT`] when the part shows two or
//!   more), with medians at most 235.
//! - **Metal gradients:** every planar metal face at least [`MIN_WIDTH`] B wide spans at least 12
//!   between its 5th and 95th percentiles. A flat mirror reflects a patch of the studio about as
//!   wide as itself, seen from the room's walls 2 B away: a face narrower than 0.2 B sees about
//!   6° of a smooth cyclorama, which cannot hold a 12/255 gradient, so it is judged by the other
//!   rules only.
//! - **Sample Plate:** in metal, its top and largest side medians in [40, 220].

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::geom::prelude::*;

/// Linear base-colour luminance under which a dielectric is a dark material (about sRGB 63).
pub(crate) const DARK_ALBEDO: f64 = 0.05;
/// How far a dark part's 99th percentile must stand above its median: a visible highlight.
pub(crate) const HIGHLIGHT: f64 = 24.0;
/// How far a dark part's face medians must spread: faces that read apart.
pub(crate) const DISTINCT: f64 = 6.0;
/// The narrowest planar metal face, in room extents B, that must show a gradient.
pub(crate) const MIN_WIDTH: f64 = 0.2;

/// Relative luminance of a linear colour.
pub(crate) fn luminance(c: Vec3) -> f64 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// A flat face's width: twice its area over its perimeter, from its triangles (`points` taken
/// three at a time). That is the short side of a long strip and the radial width of a ring.
/// Boundary edges are those one triangle of the face uses; vertices are matched by position.
pub(crate) fn width(points: &[Vec3]) -> f64 {
    let key = |p: Vec3| [p.x, p.y, p.z].map(|v| (v * 1e6).round() as i64);
    let mut area = 0.0;
    let mut edges: std::collections::HashMap<([i64; 3], [i64; 3]), (usize, f64)> = Default::default();
    for t in points.as_chunks::<3>().0 {
        area += 0.5 * (t[1] - t[0]).cross(t[2] - t[0]).len();
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (ka, kb) = (key(a), key(b));
            let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
            let e = edges.entry(k).or_insert((0, a.dist(b)));
            e.0 += 1;
        }
    }
    let perimeter: f64 = edges.values().filter(|(n, _)| *n == 1).map(|(_, l)| l).sum();
    if perimeter > 0.0 { 2.0 * area / perimeter } else { 0.0 }
}

/// Whether a face row is of a dark material.
fn dark(face: &Value) -> bool {
    face["metallic"].as_f64().unwrap_or(0.0) < 0.5 && face["albedo"].as_f64().is_some_and(|a| a < DARK_ALBEDO)
}

/// The bar on one frame's metrics: each failure as `{check, part, detail}`. `check` is one of
/// `clipped`, `crushed`, `face median`, `dark highlight`, `dark faces`, `metal gradient` and
/// `plate top/side`.
pub(crate) fn failures(m: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut fail = |check: &str, part: &str, detail: String| out.push(json!({"check": check, "part": part, "detail": detail}));
    let empty = Vec::new();
    let faces = m["faces"].as_array().unwrap_or(&empty);
    for p in m["parts"].as_array().unwrap_or(&empty) {
        let part = p["part"].as_str().unwrap_or("?");
        for key in ["clipped", "crushed"] {
            let v = p[key].as_f64().unwrap_or(0.0);
            if v > 0.01 {
                fail(key, part, format!("{:.2} % (bar ≤ 1 %)", 100.0 * v));
            }
        }
        let own: Vec<&Value> = faces.iter().filter(|f| f["part"] == p["part"]).collect();
        if own.is_empty() || !own.iter().all(|f| dark(f)) {
            continue;
        }
        let (median, p99) = (p["median"].as_f64().unwrap_or(0.0), p["p99"].as_f64().unwrap_or(0.0));
        if p99 - median < HIGHLIGHT {
            fail("dark highlight", part, format!("99th percentile {p99:.0} over median {median:.0} (bar ≥ {HIGHLIGHT:.0} above)"));
        }
        let medians: Vec<f64> = own.iter().filter_map(|f| f["median"].as_f64()).collect();
        if medians.len() >= 2 {
            let spread = medians.iter().copied().fold(f64::NEG_INFINITY, f64::max) - medians.iter().copied().fold(f64::INFINITY, f64::min);
            if spread < DISTINCT {
                fail("dark faces", part, format!("face medians spread {spread:.0} (bar ≥ {DISTINCT:.0})"));
            }
        }
    }
    for f in faces {
        let (part, face, median) = (f["part"].as_str().unwrap_or("?"), &f["face"], f["median"].as_f64().unwrap_or(0.0));
        let low = if dark(f) { f64::NEG_INFINITY } else { 20.0 };
        if !(low..=235.0).contains(&median) {
            let bar = if dark(f) { "dark material, bar ≤ 235" } else { "bar 20–235" };
            fail("face median", part, format!("face {face} median {median:.0} ({bar})"));
        }
        let metal = f["metallic"].as_f64().unwrap_or(0.0) >= 0.5;
        let wide = f["width"].as_f64().unwrap_or(f64::INFINITY) >= MIN_WIDTH;
        let span = f["span"].as_f64().unwrap_or(0.0);
        if metal && wide && f["planar"].as_bool() == Some(true) && span < 12.0 {
            let width = f["width"].as_f64().unwrap_or(f64::NAN);
            fail("metal gradient", part, format!("planar face {face} ({width:.2} B wide) spans {span:.0} (bar ≥ 12)"));
        }
    }
    let plate: Vec<&Value> = faces.iter().filter(|f| f["part"] == "Sample Plate" && f["metallic"].as_f64().unwrap_or(0.0) >= 0.5).collect();
    let largest = |want: fn(f64) -> bool| {
        plate
            .iter()
            .filter(|f| want(f["normal"][2].as_f64().unwrap_or(0.0)))
            .max_by_key(|f| f["pixels"].as_u64().unwrap_or(0))
            .and_then(|f| f["median"].as_f64())
    };
    for (name, median) in [("top", largest(|z| z > 0.99)), ("side", largest(|z| z.abs() < 0.05))] {
        if let Some(median) = median.filter(|m| !(40.0..=220.0).contains(m)) {
            fail("plate top/side", "Sample Plate", format!("{name} median {median:.0} (bar 40–220)"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(part: &str, median: f64, metallic: f64, albedo: f64) -> Value {
        json!({"part": part, "face": 1, "pixels": 500, "median": median, "span": 30.0, "planar": true, "metallic": metallic, "albedo": albedo, "width": 0.5, "normal": [0, 0, 1]})
    }

    #[test]
    fn dark_materials_are_judged_by_their_own_albedo() {
        let part = |median: f64, p99: f64| json!({"part": "Box", "pixels": 1000, "clipped": 0.0, "crushed": 0.0, "median": median, "p99": p99});
        // A dark box at medians 8 and 15 with a highlight passes; a light one fails on both faces.
        let dark = json!({"parts": [part(10.0, 60.0)], "faces": [face("Box", 8.0, 0.0, 0.01), face("Box", 15.0, 0.0, 0.01)]});
        assert!(failures(&dark).is_empty(), "{:?}", failures(&dark));
        let light = json!({"parts": [part(10.0, 60.0)], "faces": [face("Box", 8.0, 0.0, 0.5), face("Box", 15.0, 0.0, 0.5)]});
        let checks: Vec<String> = failures(&light).iter().filter_map(|f| f["check"].as_str().map(str::to_owned)).collect();
        assert_eq!(checks, ["face median", "face median"]);
        // Without a highlight, or with faces that read alike, a dark part fails.
        let flat = json!({"parts": [part(10.0, 20.0)], "faces": [face("Box", 8.0, 0.0, 0.01), face("Box", 10.0, 0.0, 0.01)]});
        let checks: Vec<String> = failures(&flat).iter().filter_map(|f| f["check"].as_str().map(str::to_owned)).collect();
        assert_eq!(checks, ["dark highlight", "dark faces"]);
        // Crushing still fails a dark part; metals are never dark.
        let crushed = json!({"parts": [{"part": "Box", "crushed": 0.05, "median": 10.0, "p99": 60.0}], "faces": [face("Box", 8.0, 0.0, 0.01), face("Box", 15.0, 0.0, 0.01)]});
        assert_eq!(failures(&crushed)[0]["check"], "crushed");
        let metal = json!({"parts": [part(10.0, 60.0)], "faces": [face("Box", 8.0, 1.0, 0.01)]});
        assert_eq!(failures(&metal)[0]["check"], "face median");
    }

    #[test]
    fn narrow_metal_faces_are_not_judged_for_a_gradient() {
        let part = json!({"part": "Lid", "pixels": 1000, "clipped": 0.0, "crushed": 0.0, "median": 100.0, "p99": 200.0});
        let mut narrow = face("Lid", 100.0, 1.0, 0.6);
        narrow["span"] = json!(4.0);
        narrow["width"] = json!(0.1);
        let mut wide = narrow.clone();
        wide["width"] = json!(0.3);
        assert!(failures(&json!({"parts": [part.clone()], "faces": [narrow]})).is_empty());
        assert_eq!(failures(&json!({"parts": [part], "faces": [wide]}))[0]["check"], "metal gradient");
    }

    #[test]
    fn width_is_twice_the_area_over_the_perimeter() {
        // A long strip, 80 × 8: 2·640/176, close to its short side.
        let (a, b, c, d) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(80.0, 0.0, 0.0), Vec3::new(80.0, 8.0, 0.0), Vec3::new(0.0, 8.0, 0.0));
        assert!((width(&[a, b, c, a, c, d]) - 1280.0 / 176.0).abs() < 1e-9);
        // A square ring between radii 6 and 12 (a boss's top), as a polygon of 64 sides: its
        // radial width, 6, not its outer size.
        let ring: Vec<Vec3> = (0..64)
            .flat_map(|k| {
                let (t0, t1) = (k as f64 / 64.0 * std::f64::consts::TAU, (k + 1) as f64 / 64.0 * std::f64::consts::TAU);
                let at = |r: f64, t: f64| Vec3::new(r * t.cos(), r * t.sin(), 0.0);
                [at(6.0, t0), at(12.0, t0), at(12.0, t1), at(6.0, t0), at(12.0, t1), at(6.0, t1)]
            })
            .collect();
        assert!((width(&ring) - 6.0).abs() < 0.05, "{}", width(&ring));
        assert_eq!(width(&[]), 0.0);
    }
}
