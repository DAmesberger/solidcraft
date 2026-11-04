use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_render::material_graph::{Ir, MaterialGraph, SurfaceSample, base_response, base_sample, lower, uniform_words, validate};

use super::GraphScene;
use crate::view::glass_tests::{inner_pixels, render_frame, studio, test_frame};
use crate::view::{Frame, GpuScene, GpuTarget, ProductLook, Renderer, StudioPreset};

/// The worked examples of docs/material-graph.md §3.8 and the `pbr`-equivalent graph.
pub(crate) const FIXTURES: &[(&str, &str)] = &[
    ("black-epoxy-clear-coat", include_str!("../../../render/tests/fixtures/material_graph/black-epoxy-clear-coat.json")),
    ("brushed-nickel-silver", include_str!("../../../render/tests/fixtures/material_graph/brushed-nickel-silver.json")),
    ("electrolytic-sleeve", include_str!("../../../render/tests/fixtures/material_graph/electrolytic-sleeve.json")),
    ("laser-etched-nickel-silver", include_str!("../../../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json")),
    ("pbr-copper", include_str!("../../../render/tests/fixtures/material_graph/pbr-copper.json")),
    ("solder-mask-over-copper", include_str!("../../../render/tests/fixtures/material_graph/solder-mask-over-copper.json")),
];

/// A graph, lowered, with its base sample.
struct Lowered {
    ir: Arc<Ir>,
    base: SurfaceSample,
}

fn lowered(graph: &MaterialGraph) -> Result<Lowered, String> {
    let valid = validate(graph).map_err(|e| e.to_string())?;
    let ir = Arc::new(lower(&valid).map_err(|e| e.to_string())?);
    let base = base_sample(&valid).map_err(|e| e.to_string())?;
    Ok(Lowered { ir, base })
}

fn fixture(text: &str) -> Lowered {
    let graph: MaterialGraph = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}"));
    lowered(&graph).unwrap_or_else(|e| panic!("{e}"))
}

fn graph_of(value: Value) -> MaterialGraph {
    serde_json::from_value(value).unwrap_or_else(|e| panic!("{e}"))
}

/// The GLSL ES 3.00 (WebGL2) of `entries` of a module.
fn glsl(source: &str, entries: &[(naga::ShaderStage, &str)]) -> Result<Vec<String>, String> {
    use naga::back::glsl;
    let (module, info) = super::validate(source).map_err(|e| e.0)?;
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    entries
        .iter()
        .map(|(stage, entry)| {
            let pipeline = glsl::PipelineOptions { shader_stage: *stage, entry_point: (*entry).into(), multiview: None };
            let mut out = String::new();
            glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
                .and_then(|mut w| w.write().map(|_| ()))
                .map_err(|e| format!("{entry}: {e:?}"))?;
            Ok(out)
        })
        .collect()
}

/// The shape's entry points in GLSL ES 3.00: its vertex stream and `fs_mat`. The fragment reads
/// the page (a uniform-block member array) at an index from the flat varying record offset,
/// which ES 3.00 allows and which the whole uniform-page design rests on (§5.3).
fn translates_for_webgl2(name: &str, source: &str, object: bool) {
    let vertex = super::cache::vertex_entry(object);
    let out = glsl(source, &[(naga::ShaderStage::Vertex, vertex), (naga::ShaderStage::Fragment, "fs_mat")]).unwrap_or_else(|e| panic!("{name}: {e}"));
    let fragment = out.get(1).map(String::as_str).unwrap_or("");
    assert!(fragment.contains("flat in uint"), "{name}: the record offset is a flat varying");
    assert!(
        fragment.lines().any(|l| l.contains(".m[") && !l.contains(".m[0]")),
        "{name}: the page is indexed dynamically\n{}",
        fragment.lines().filter(|l| l.contains(".m[")).take(3).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn every_fixture_shape_validates_and_translates_for_webgl2() {
    for (name, text) in FIXTURES {
        let l = fixture(text);
        let source = super::shape_source(&l.ir).unwrap_or_else(|e| panic!("{name}: {e}"));
        super::validate(&source).unwrap_or_else(|e| panic!("{name}: {e}\n{}", super::material_eval(&l.ir).unwrap_or_default()));
        translates_for_webgl2(name, &source, l.ir.object_space);
    }
    let fallback = super::fallback_source();
    for object in [false, true] {
        translates_for_webgl2("fallback", &fallback, object);
    }
}

/// Derivatives stay in uniform control flow: `fs_mat` takes all of them before anything else, and
/// generated code never branches or takes one (§5.1).
#[test]
fn generated_code_is_straight_line_without_derivatives() {
    let prelude = super::PRELUDE;
    let entry = prelude.split("fn fs_mat").nth(1).unwrap_or("");
    let last_derivative = ["dpdx", "dpdy", "fwidth"].iter().filter_map(|d| entry.rfind(d)).max().unwrap_or(0);
    let first_call = entry.find("material_eval").unwrap_or(0);
    assert!(last_derivative < first_call, "derivatives before the material");
    // Its one branch, the section clip, comes after every derivative.
    assert!(entry.find("if ").is_some_and(|b| b > last_derivative), "the clip follows the derivatives");
    assert_eq!(entry.matches("if ").count(), 1, "fs_mat branches only for the section");
    assert!(!entry.contains("for ") && !entry.contains("loop"), "fs_mat does not loop");
    for (name, text) in FIXTURES {
        let eval = super::material_eval(&fixture(text).ir).unwrap_or_default();
        for banned in ["if ", "if(", "for ", "loop", "while", "dpdx", "dpdy", "fwidth", "discard", "return"] {
            let count = eval.matches(banned).count();
            let allowed = usize::from(banned == "return");
            assert!(count <= allowed, "{name}: `{banned}` in generated code");
        }
    }
}

/// No user string reaches WGSL: ids, names and interface names are never emitted, and constants
/// are uniforms, so graphs that differ only in values generate the same code.
#[test]
fn no_user_string_or_value_reaches_the_shader() {
    let graph = |rough: f64, color: [f64; 3]| {
        graph_of(json!({
            "name": "Zebra\u{1F993} finish \"}; fn evil() {",
            "interface": [{"name": "zzsleeveqq", "type": "color3", "default": color}],
            "nodes": [
                {"id": "zq-proj.17x", "kind": "texcoord.planar", "params": {"space": "world"}, "inputs": {"u": [1, 0, 0], "v": [0, 1, 0]}},
                {"id": "zq-noise.42x", "kind": "pattern.noise", "params": {"kind": "gradient", "dimensions": 2, "octaves": 3},
                 "inputs": {"texcoord": {"from": "zq-proj.17x"}, "scale_mm": 0.731, "seed": 31337}},
                {"id": "zq-rough.99x", "kind": "remap", "inputs": {"in": {"from": "zq-noise.42x"}, "out_min": rough, "out_max": 0.9}},
                {"id": "zq-out.00x", "kind": "standard_surface", "inputs": {
                    "base_color": {"interface": "zzsleeveqq"}, "specular_roughness": {"from": "zq-rough.99x", "fallback": 0.4}}}],
            "output": "zq-out.00x"}))
    };
    let a = lowered(&graph(0.2, [0.1, 0.2, 0.3])).unwrap_or_else(|e| panic!("{e}"));
    let b = lowered(&graph(0.6, [0.7, 0.1, 0.0])).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(a.ir.shape, b.ir.shape, "values are not part of the shape");
    let (sa, sb) = (super::shape_source(&a.ir).unwrap_or_default(), super::shape_source(&b.ir).unwrap_or_default());
    assert_eq!(sa, sb, "graphs differing only in values generate the same module");
    let eval = super::material_eval(&a.ir).unwrap_or_default();
    for user in ["zq-", "zzsleeveqq", "Zebra", "evil", "31337", "0.731", "0.2,"] {
        assert!(!eval.contains(user), "`{user}` reached the generated code:\n{eval}");
    }
}

/// A deterministic generator of varied valid graphs (the fuzz part of the WebGL2 corpus, §5.3):
/// every node kind, both spaces, slopes and triplanar expansion, surface mixes.
pub(crate) struct Fuzz(pub(crate) u64);

impl Fuzz {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items.get(self.below(items.len())).copied().unwrap_or("")
    }
    fn unit(&mut self) -> f64 {
        (self.next() % 1000) as f64 / 1000.0
    }

    pub(crate) fn graph(&mut self) -> Value {
        let mut nodes = Vec::new();
        let mut tcs: Vec<String> = Vec::new();
        let mut floats: Vec<String> = Vec::new();
        let mut colors: Vec<String> = Vec::new();
        let mut normals: Vec<String> = Vec::new();
        let mut tangents: Vec<String> = Vec::new();
        let space = |f: &mut Fuzz| if f.below(2) == 0 { "object" } else { "world" };
        let mut id = 0;
        let mut fresh = || {
            id += 1;
            format!("n{id}")
        };
        let planar = fresh();
        nodes.push(json!({"id": planar, "kind": "texcoord.planar", "params": {"space": space(self)}, "inputs": {"u": [0.5, 0, 0], "v": [0, 2, 0]}}));
        tcs.push(planar.clone());
        tangents.push(format!("{planar}.tangent"));
        match self.below(3) {
            0 => {
                let t = fresh();
                nodes.push(json!({"id": t, "kind": "texcoord.triplanar", "params": {"space": space(self)}, "inputs": {"scale_mm": [1, 2, 0.5]}}));
                tcs.push(t);
            }
            1 => {
                let t = fresh();
                let unwrap = self.pick(&["arc_length", "angle"]);
                nodes.push(
                    json!({"id": t, "kind": "texcoord.cylindrical", "params": {"space": space(self), "unwrap": unwrap}, "inputs": {"radius_mm": 3}}),
                );
                tangents.push(format!("{t}.tangent"));
                tcs.push(t);
            }
            _ => {
                let t = fresh();
                nodes.push(json!({"id": t, "kind": "texcoord.transform", "inputs": {"texcoord": {"from": planar}, "offset": [1, 2], "scale": [2, 0.5], "rotation": 0.3}}));
                tcs.push(t);
            }
        }
        let image = self.below(3) == 0;
        for _ in 0..2 + self.below(6) {
            let tc = tcs.get(self.below(tcs.len())).cloned().unwrap_or_default();
            let n = fresh();
            match self.below(9) {
                0 => {
                    let kind = self.pick(&["value", "gradient", "cellular"]);
                    let octaves = 1 + self.below(3);
                    nodes.push(json!({"id": n, "kind": "pattern.noise", "params": {"kind": kind, "dimensions": 2, "octaves": octaves},
                        "inputs": {"texcoord": {"from": tc}, "scale_mm": 0.5, "seed": self.below(1000)}}));
                    floats.push(n);
                }
                1 => {
                    let p = fresh();
                    nodes.push(json!({"id": p, "kind": "geometry.position", "params": {"space": space(self)}}));
                    nodes.push(
                        json!({"id": n, "kind": "pattern.noise", "params": {"kind": self.pick(&["value", "cellular"]), "dimensions": 3, "octaves": 1},
                        "inputs": {"position": {"from": p}, "scale_mm": 0.2, "seed": 5}}),
                    );
                    floats.push(n);
                }
                2 => {
                    nodes.push(json!({"id": n, "kind": "pattern.brushed", "inputs": {"texcoord": {"from": tc}, "density_per_mm": 30, "streak_mm": 3, "contrast": 0.5, "jitter": 0.4, "seed": 7}}));
                    tangents.push(format!("{n}.tangent"));
                    floats.push(n);
                }
                3 => {
                    nodes.push(json!({"id": n, "kind": "pattern.weave", "params": {"kind": self.pick(&["plain", "twill_2x2"])},
                        "inputs": {"texcoord": {"from": tc}, "pitch_mm": [0.6, 0.8], "crown": self.unit()}}));
                    floats.push(n);
                }
                4 => {
                    nodes.push(json!({"id": n, "kind": "pattern.checker", "inputs": {"texcoord": {"from": tc}, "size_mm": [1, 2]}}));
                    floats.push(n);
                }
                5 => {
                    nodes.push(json!({"id": n, "kind": "pattern.grid", "inputs": {"texcoord": {"from": tc}, "size_mm": [1, 1], "line_mm": 0.1}}));
                    floats.push(n);
                }
                6 => {
                    nodes.push(json!({"id": n, "kind": "pattern.gradient", "params": {"kind": self.pick(&["linear", "radial"])}, "inputs": {"texcoord": {"from": tc}, "start": -2, "end": 3}}));
                    floats.push(n);
                }
                7 if image => {
                    nodes.push(json!({"id": n, "kind": "image", "params": {"space": self.pick(&["srgb", "linear"]), "wrap": self.pick(&["repeat", "clamp", "mirror"]), "channel": self.pick(&["rgb", "r", "a"])},
                        "inputs": {"texcoord": {"from": tc}, "file": {"interface": "tex"}, "default": [0.4, 0.5, 0.6]}}));
                    colors.push(n.clone());
                    floats.push(format!("{n}.value"));
                }
                _ => {
                    let f = fresh();
                    nodes.push(json!({"id": f, "kind": "geometry.facing"}));
                    floats.push(f);
                    nodes.push(json!({"id": n, "kind": "hsv_adjust", "inputs": {"in": [0.3, 0.5, 0.2], "hue": 0.4, "saturation": 1.5}}));
                    colors.push(n);
                }
            }
        }
        for _ in 0..self.below(8) {
            let a = floats.get(self.below(floats.len())).cloned().unwrap_or_default();
            let b = floats.get(self.below(floats.len())).cloned().unwrap_or_default();
            let n = fresh();
            let op = self.pick(&[
                "add",
                "subtract",
                "multiply",
                "divide",
                "min",
                "max",
                "power",
                "mix",
                "step",
                "smoothstep",
                "remap",
                "clamp",
                "abs",
                "floor",
                "fract",
                "sin",
                "cos",
                "sqrt",
                "invert",
                "dot",
                "length",
                "normalize",
            ]);
            let inputs = match op {
                "mix" => json!({"a": {"from": a}, "b": {"from": b}, "t": {"from": a}}),
                "step" => json!({"edge": 0.4, "in": {"from": a}}),
                "smoothstep" => json!({"edge0": 0.2, "edge1": {"from": b}, "in": {"from": a}}),
                "remap" => json!({"in": {"from": a}, "out_min": 0.2, "out_max": {"from": b}}),
                "clamp" => json!({"in": {"from": a}, "low": 0.1, "high": {"from": b}}),
                "abs" | "floor" | "fract" | "sin" | "cos" | "sqrt" | "invert" | "length" | "normalize" => json!({"in": {"from": a}}),
                _ => json!({"a": {"from": a}, "b": {"from": b}}),
            };
            nodes.push(json!({"id": n, "kind": op, "inputs": inputs}));
            floats.push(n);
        }
        // Vector maths whose components reach a height: slopes carried per component.
        if !floats.is_empty() {
            let a = floats.get(self.below(floats.len())).cloned().unwrap_or_default();
            let b = floats.get(self.below(floats.len())).cloned().unwrap_or_default();
            let v = fresh();
            nodes.push(json!({"id": v, "kind": "combine3", "inputs": {"x": {"from": a}, "y": {"from": b}, "z": 0.3}}));
            let mut last = v;
            for _ in 0..1 + self.below(3) {
                let n = fresh();
                let op = self.pick(&[
                    "add",
                    "subtract",
                    "multiply",
                    "divide",
                    "mix",
                    "min",
                    "max",
                    "abs",
                    "sin",
                    "power",
                    "clamp",
                    "smoothstep",
                    "remap",
                    "normalize",
                    "sqrt",
                ]);
                let other = if self.below(2) == 0 {
                    let p = fresh();
                    nodes.push(json!({"id": p, "kind": "geometry.position", "params": {"space": space(self)}}));
                    json!({"from": p})
                } else {
                    json!([0.7, 1.3, 0.4])
                };
                let inputs = match op {
                    "mix" => json!({"a": {"from": last}, "b": other, "t": {"from": a}}),
                    "abs" | "sin" | "normalize" | "sqrt" => json!({"in": {"from": last}}),
                    "power" => json!({"a": {"from": last}, "b": 1.7}),
                    "clamp" => json!({"in": {"from": last}, "low": 0.2, "high": other}),
                    "smoothstep" => json!({"edge0": 0.1, "edge1": other, "in": {"from": last}}),
                    "remap" => json!({"in": {"from": last}, "in_max": other, "out_min": 0.2, "out_max": 0.8}),
                    _ => json!({"a": {"from": last}, "b": other}),
                };
                nodes.push(json!({"id": n, "kind": op, "inputs": inputs}));
                last = n;
            }
            let s = fresh();
            nodes.push(json!({"id": s, "kind": "separate3", "inputs": {"in": {"from": last}}}));
            let h = fresh();
            nodes.push(json!({"id": h, "kind": "add", "inputs": {"a": {"from": format!("{s}.x")}, "b": {"from": format!("{s}.y")}}}));
            let nh = fresh();
            nodes.push(json!({"id": nh, "kind": "normal.from_height", "inputs": {"height": {"from": h}, "relief_mm": 0.01}}));
            normals.push(nh);
        }
        if let Some(a) = floats.first().cloned() {
            let n = fresh();
            nodes.push(json!({"id": n, "kind": "combine3", "inputs": {"x": {"from": a}, "y": 0.2, "z": {"from": a}}}));
            let c = fresh();
            nodes.push(json!({"id": c, "kind": "convert.vector3_to_color3", "inputs": {"in": {"from": n}}}));
            colors.push(c);
            let s = fresh();
            nodes.push(json!({"id": s, "kind": "separate3", "inputs": {"in": {"from": n}}}));
            floats.push(format!("{s}.z"));
            let l = fresh();
            nodes.push(json!({"id": l, "kind": "luminance", "inputs": {"in": {"from": colors.first().cloned().unwrap_or_default()}}}));
            floats.push(l);
        }
        if self.below(2) == 0 {
            let h = floats.get(self.below(floats.len())).cloned().unwrap_or_default();
            let n = fresh();
            nodes.push(json!({"id": n, "kind": "normal.from_height", "inputs": {"height": {"from": h}, "relief_mm": 0.01}}));
            normals.push(n);
        }
        if image && let Some(c) = colors.first().cloned() {
            let n = fresh();
            nodes.push(json!({"id": n, "kind": "normal.map", "params": {"convention": self.pick(&["opengl", "directx"])}, "inputs": {"color": {"from": c}, "strength": 0.5}}));
            normals.push(n);
        }
        if normals.len() >= 2 {
            let n = fresh();
            nodes.push(json!({"id": n, "kind": "normal.blend", "inputs": {"a": {"from": normals[0]}, "b": {"from": normals[1]}, "mix": 0.3}}));
            normals.push(n);
        }
        let mut surface = |f: &mut Fuzz, nodes: &mut Vec<Value>| {
            let n = fresh();
            let mut inputs = serde_json::Map::new();
            let mut wire = |name: &str, list: &[String], f: &mut Fuzz| {
                if !list.is_empty() && f.below(3) > 0 {
                    inputs.insert(name.to_owned(), json!({"from": list.get(f.below(list.len())).cloned().unwrap_or_default()}));
                }
            };
            wire("base_color", &colors, f);
            wire("specular_roughness", &floats, f);
            wire("base_metalness", &floats, f);
            wire("coat_weight", &floats, f);
            wire("specular_roughness_anisotropy", &floats, f);
            wire("geometry_tangent", &tangents, f);
            wire("geometry_normal", &normals, f);
            wire("geometry_coat_normal", &normals, f);
            wire("emission_luminance", &floats, f);
            wire("geometry_opacity", &floats, f);
            nodes.push(json!({"id": n, "kind": "standard_surface", "inputs": inputs}));
            n
        };
        let mut output = surface(self, &mut nodes);
        for _ in 0..self.below(3) {
            let fg = surface(self, &mut nodes);
            let m = format!("mix{}", nodes.len());
            let t = floats.get(self.below(floats.len())).cloned().unwrap_or_default();
            nodes.push(json!({"id": m, "kind": "surface.mix", "inputs": {"fg": {"from": fg}, "bg": {"from": output}, "mix": {"from": t}}}));
            output = m;
        }
        json!({"name": "fuzz", "interface": [{"name": "tex", "type": "texture"}], "nodes": nodes, "output": output})
    }
}

/// Every graph of a generated corpus that validates generates code naga validates and that
/// translates to WebGL2's GLSL ES 3.00.
#[test]
fn a_generated_corpus_validates_and_translates_for_webgl2() {
    let mut fuzz = Fuzz(0x9e37_79b9_7f4a_7c15);
    let (mut valid, mut refused) = (0, Vec::new());
    for i in 0..96 {
        let value = fuzz.graph();
        let graph = graph_of(value.clone());
        let l = match lowered(&graph) {
            Ok(l) => l,
            Err(e) => {
                refused.push(e);
                continue;
            }
        };
        valid += 1;
        let source = super::shape_source(&l.ir).unwrap_or_else(|e| panic!("graph {i}: {e}\n{value}"));
        if let Err(e) = super::validate(&source) {
            panic!("graph {i}: {e}\n{}\n{value}", super::material_eval(&l.ir).unwrap_or_default());
        }
        translates_for_webgl2(&format!("graph {i}"), &source, l.ir.object_space);
    }
    assert!(valid >= 64, "the corpus is mostly valid ({valid} of 96; refused e.g. {:?})", refused.first());
}

#[test]
fn records_hold_the_base_response_then_the_slots() {
    let copper = fixture(FIXTURES[4].1);
    let brushed = fixture(FIXTURES[1].1);
    let mut scene = GraphScene::default();
    let words = |l: &Lowered| uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
    let a = scene.material(&copper.ir, &words(&copper), &copper.base).unwrap_or_else(|e| panic!("{e}"));
    let b = scene.material(&brushed.ir, &words(&brushed), &brushed.base).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(scene.material(&copper.ir, &words(&copper), &copper.base), Ok(a), "an equal material shares its record");
    assert!(scene.material(&copper.ir, &[0; 3], &copper.base).is_err(), "words must match the graph");
    for m in [a, b] {
        scene.vertex(m, [0.0; 3], [0.0, 0.0, 1.0], [1.0, 2.0, 3.0]);
    }
    let page = scene.pages().first().map(|p| p.words.clone()).unwrap_or_default();
    let record = super::base_record(&copper.base);
    let surface = copper.base.opaque;
    assert_eq!(record[0], surface.base_color().x as f32);
    assert_eq!(record[3], surface.metallic() as f32);
    assert_eq!(record[4], surface.roughness() as f32);
    assert_eq!(page.get(..28).map(|w| w.iter().map(|x| f32::from_bits(*x)).collect::<Vec<_>>()), Some(record.to_vec()));
    let draws = scene.draws();
    assert_eq!(draws.len(), 2, "one draw per shape on a page");
    let offset = |d: &super::GraphDraw| d.vertices.get(24..28).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    assert_eq!(offset(&draws[0]), Some(0), "the first record starts the page");
    let copper_vec4s = super::BASE_VEC4 + copper.ir.uniform_floats / 4;
    assert_eq!(offset(&draws[1]), Some(copper_vec4s), "the second follows it");
    assert_eq!(draws[1].vertices.len(), if brushed.ir.object_space { super::OBJECT_VERTEX_SIZE } else { super::VERTEX_SIZE });
    // A page holds whole records: past 1024 vec4s a new page starts at 0.
    let mut full = GraphScene::default();
    let per = super::BASE_VEC4 + brushed.ir.uniform_floats / 4;
    let mut last = None;
    for i in 0..(super::PAGE_VEC4 / per + 1) {
        let mut w = words(&brushed);
        if let Some(x) = w.first_mut() {
            *x = i;
        }
        last = full.material(&brushed.ir, &w, &brushed.base).ok();
    }
    assert_eq!(full.pages().len(), 2);
    if let Some(m) = last {
        full.vertex(m, [0.0; 3], [0.0; 3], [0.0; 3]);
    }
    let second = full.draws().iter().find(|d| d.page == 1).and_then(offset);
    assert_eq!(second, Some(0));
}

// GPU frames.

const SIZE: u32 = 64;

fn target(device: &wgpu::Device) -> GpuTarget {
    GpuTarget { format: wgpu::TextureFormat::Rgba8Unorm, depth: Some(crate::depth_format(device, true)), samples: 1, tess: false, oit: false }
}

const SQUARE: [[f32; 3]; 6] =
    [[-12.0, -12.0, 0.0], [12.0, -12.0, 0.0], [12.0, 12.0, 0.0], [-12.0, -12.0, 0.0], [12.0, 12.0, 0.0], [-12.0, 12.0, 0.0]];

/// A square of `l` over the whole fixed view, facing the viewer; strip `i` of `n` when `n` > 1.
fn add_square(scene: &mut GpuScene, l: &Lowered, i: usize, n: usize) {
    let words = uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
    let m = scene.graphs.material(&l.ir, &words, &l.base).unwrap_or_else(|e| panic!("{e}"));
    let n = n.max(1) as f32;
    let (x0, x1) = (-12.0 + 24.0 * i as f32 / n, -12.0 + 24.0 * (i as f32 + 1.0) / n);
    for p in SQUARE {
        let p = [if p[0] < 0.0 { x0 } else { x1 }, p[1], p[2]];
        scene.graphs.vertex(m, p, [0.0, 0.0, 1.0], p);
    }
    // The face the graph shades, as the scene draws it.
    let face = scene.color([200, 200, 200, 255]);
    for p in SQUARE {
        let p = [if p[0] < 0.0 { x0 } else { x1 }, p[1], p[2]];
        scene.tri(p, [0.0, 0.0, 1.0], face);
    }
}

fn pbr_square(surface: solidcraft_render::surface::OpaqueSurface) -> GpuScene {
    let mut scene = GpuScene::default();
    for p in SQUARE {
        scene.pbr_tri(p, [0.0, 0.0, 1.0], surface);
    }
    scene
}

/// A frame in the realistic look: the product look with its studio, and occlusion on.
fn realistic(scene: GpuScene, key: u64, units_per_mm: f32) -> Frame {
    Frame { key, product_look: Some(ProductLook::default()), ao_radius: 2.0, units_per_mm, ..test_frame(scene, 1.0) }
}

fn renderer(device: &wgpu::Device, queue: &wgpu::Queue) -> Renderer {
    let mut r = Renderer::new(device, target(device), None);
    r.set_studio_environment(device, queue, &studio(StudioPreset::Product));
    r.materials_mut().no_time_budget = true;
    r
}

fn mean(p: &[[u8; 3]]) -> f64 {
    p.iter().map(|c| c.iter().map(|&x| f64::from(x)).sum::<f64>()).sum::<f64>() / (3 * p.len().max(1)) as f64
}

fn save(name: &str, bytes: &[u8]) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/material-frames");
    if std::fs::create_dir_all(&dir).is_ok()
        && let Some(img) = image::RgbaImage::from_raw(SIZE, SIZE, bytes.to_vec())
    {
        let _ = img.save(dir.join(format!("{name}.png")));
    }
}

/// The `pbr`-equivalent graph draws exactly as its `pbr` look in the realistic look: the
/// generated code reads the same values from its page that the PBR vertices carry, and both go
/// through the one `pbr_shade`.
#[test]
fn pbr_copper_draws_as_its_pbr_look() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let copper = fixture(FIXTURES[4].1);
    let Ok(valid) = validate(&serde_json::from_str::<MaterialGraph>(FIXTURES[4].1).unwrap_or_else(|e| panic!("{e}"))) else { panic!("valid") };
    let surface = base_response(&valid).unwrap_or_else(|e| panic!("{e}"));
    let mut r = renderer(&device, &queue);
    let pbr = render_frame(&mut r, &device, &queue, &realistic(pbr_square(surface), 1, 1.0), true);
    let mut scene = GpuScene::default();
    add_square(&mut scene, &copper, 0, 1);
    let graph = render_frame(&mut r, &device, &queue, &realistic(scene, 2, 1.0), true);
    assert_eq!(r.material_stats().compiled, 1);
    assert_eq!(r.material_stats().fallback_draws, 0, "drawn by its own pipeline");
    save("pbr-copper", &graph);
    let (a, b) = (inner_pixels(&pbr), inner_pixels(&graph));
    let worst = a.iter().zip(&b).map(|(x, y)| x.iter().zip(y).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)).max().unwrap_or(255);
    assert!(worst <= 1, "the graph matches its pbr look (worst channel difference {worst})");
    assert!(mean(&b) > 20.0, "lit: {}", mean(&b));
}

/// Every fixture draws in the realistic look by its own pipeline: lit and not clipped, the
/// patterned ones different from their base response (drawn through the fallback) at a scale
/// where their detail is above the pixel footprint, the `pbr`-equivalent one equal to it. The
/// frames go to `target/material-frames/` for review.
#[test]
fn the_fixtures_draw_in_the_realistic_look() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    // World units per mm per fixture: zoomed in until its finest detail spans pixels.
    let zoom = [40.0, 40.0, 1.0, 20.0, 1.0, 2.0];
    for (k, ((name, text), units)) in FIXTURES.iter().zip(zoom).enumerate() {
        let l = fixture(text);
        let mut scene = GpuScene::default();
        add_square(&mut scene, &l, 0, 1);
        let key = 10 + 2 * k as u64;
        let mut r = renderer(&device, &queue);
        r.materials_mut().compiles_per_frame = 0;
        let base = inner_pixels(&render_frame(&mut r, &device, &queue, &realistic(scene.clone(), key, units), true));
        r.materials_mut().compiles_per_frame = super::COMPILES_PER_FRAME;
        let bytes = render_frame(&mut r, &device, &queue, &realistic(scene, key + 1, units), true);
        let stats = r.material_stats();
        assert_eq!(stats.fallback_draws, 0, "{name}: drawn by its own pipeline ({stats:?})");
        save(name, &bytes);
        let pixels = inner_pixels(&bytes);
        let m = mean(&pixels);
        assert!(m > 3.0 && m < 252.0, "{name}: lit, not clipped ({m})");
        let changed = pixels.iter().zip(&base).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 1)).count();
        let share = changed as f64 / pixels.len().max(1) as f64;
        match *name {
            "pbr-copper" => assert_eq!(changed, 0, "{name}: a constant graph is its base response"),
            // Black epoxy's texture is the base roughness under an index-matched coat (base IOR 1:
            // no base reflection), so it shows only once the coat's own normal is shaded.
            // The etched mark is an image: until textures are sampled (track C3) it shows none.
            "brushed-nickel-silver" | "electrolytic-sleeve" => {
                assert!(share > 0.05, "{name}: its pattern shows ({:.1} % of pixels differ from the base response)", share * 100.0)
            }
            _ => {}
        }
    }
}

/// A high-contrast pattern draws as computed: a checker between black and white shows both, in
/// cells of the size asked for.
#[test]
fn a_checker_draws_its_cells() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let checker = lowered(&graph_of(json!({"name": "checker", "nodes": [
        {"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}},
        {"id": "k", "kind": "pattern.checker", "inputs": {"texcoord": {"from": "uv"}, "size_mm": [6, 6]}},
        {"id": "c", "kind": "convert.float_to_color3", "inputs": {"in": {"from": "k"}}},
        {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "c"}, "specular_roughness": 0.8}}], "output": "s"})))
    .unwrap_or_else(|e| panic!("{e}"));
    let mut r = renderer(&device, &queue);
    let mut scene = GpuScene::default();
    add_square(&mut scene, &checker, 0, 1);
    let bytes = render_frame(&mut r, &device, &queue, &realistic(scene, 1, 1.0), true);
    save("checker", &bytes);
    // The fixed view spans ±10 mm over 64 px: cells of 6 mm are about 19 px.
    let at = |x: usize, y: usize| bytes.get((y * SIZE as usize + x) * 4).copied().unwrap_or(0);
    let cell = |i: f32, j: f32| at((32.0 + i * 19.2 + 9.6) as usize, (32.0 + j * 19.2 + 9.6) as usize);
    let (a, b, c) = (cell(0.0, 0.0), cell(-1.0, 0.0), cell(-1.0, -1.0));
    assert!(a.abs_diff(b) > 60, "neighbouring cells differ: {a} {b}");
    assert!(a.abs_diff(c) < 8, "diagonal cells match: {a} {c}");
}

/// A shape that is not compiled yet draws its base response through the fallback pipeline, which
/// equals its base response's `pbr` drawing; once compiled it draws its pattern.
#[test]
fn a_shape_draws_its_base_response_until_it_compiles() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let (name, text) = FIXTURES[1];
    let brushed = fixture(text);
    let mut r = renderer(&device, &queue);
    let surface = brushed.base.opaque;
    let pbr = inner_pixels(&render_frame(&mut r, &device, &queue, &realistic(pbr_square(surface), 1, 40.0), true));
    r.materials_mut().compiles_per_frame = 0;
    let mut scene = GpuScene::default();
    add_square(&mut scene, &brushed, 0, 1);
    let first = render_frame(&mut r, &device, &queue, &realistic(scene.clone(), 2, 40.0), true);
    let stats = r.material_stats();
    assert_eq!((stats.compiled, stats.pending, stats.fallback_draws), (0, 1, 1), "{name}: {stats:?}");
    assert!(r.wants_redraw(), "a frame with a shape compiling asks for another");
    save("brushed-fallback", &first);
    let first = inner_pixels(&first);
    let worst = pbr.iter().zip(&first).map(|(x, y)| x.iter().zip(y).map(|(p, q)| p.abs_diff(*q)).max().unwrap_or(0)).max().unwrap_or(255);
    assert!(worst <= 1, "the fallback is the base response (worst {worst})");
    r.materials_mut().compiles_per_frame = super::COMPILES_PER_FRAME;
    let compiled = inner_pixels(&render_frame(&mut r, &device, &queue, &realistic(scene, 3, 40.0), true));
    let stats = r.material_stats();
    assert_eq!((stats.compiled, stats.pending, stats.fallback_draws), (1, 0, 0), "{stats:?}");
    assert!(!r.wants_redraw());
    assert!(compiled.iter().zip(&first).any(|(a, b)| a != b), "the compiled shape draws its pattern");
}

/// Graphs whose noise has 1–5 octaves: five shapes.
fn octave_shapes() -> Vec<Lowered> {
    (1..=5)
        .map(|octaves| {
            lowered(&graph_of(json!({"name": "o", "nodes": [
                {"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}},
                {"id": "n", "kind": "pattern.noise", "params": {"octaves": octaves}, "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 2}},
                {"id": "c", "kind": "convert.float_to_color3", "inputs": {"in": {"from": "n"}}},
                {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "c"}}}], "output": "s"})))
            .unwrap_or_else(|e| panic!("{e}"))
        })
        .collect()
}

/// At most `COMPILES_PER_FRAME` new pipelines a frame; the rest draw their base response
/// meanwhile, and every shape has its own pipeline within a few frames. A value edit afterwards
/// rewrites a page and compiles nothing.
#[test]
fn shapes_compile_within_the_frame_budget() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let shapes = octave_shapes();
    let keys: std::collections::BTreeSet<_> = shapes.iter().map(|l| l.ir.shape).collect();
    assert_eq!(keys.len(), 5, "octaves are part of the shape");
    let mut r = renderer(&device, &queue);
    let mut scene = GpuScene::default();
    for (i, l) in shapes.iter().enumerate() {
        add_square(&mut scene, l, i, shapes.len());
    }
    let mut seen = Vec::new();
    for key in 1..=3 {
        render_frame(&mut r, &device, &queue, &realistic(scene.clone(), key, 1.0), true);
        let s = r.material_stats();
        seen.push((s.compiled, s.pending, s.fallback_draws));
    }
    assert_eq!(seen, vec![(2, 3, 3), (2, 1, 1), (1, 0, 0)]);
    assert!(shapes.iter().all(|l| r.materials_mut().is_ready(&l.ir.shape)));
    // A value edit: same shapes, other values. Nothing compiles; the page is rewritten.
    let mut edited = GpuScene::default();
    for (i, l) in shapes.iter().enumerate() {
        let mut words = uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
        for w in &mut words {
            if f32::from_bits(*w) == 2.0 {
                *w = 0.5f32.to_bits();
            }
        }
        let m = edited.graphs.material(&l.ir, &words, &l.base).unwrap_or_else(|e| panic!("{e}"));
        let n = shapes.len() as f32;
        let face = edited.color([200, 200, 200, 255]);
        for p in SQUARE {
            let x = -12.0 + 24.0 * (i as f32 + if p[0] < 0.0 { 0.0 } else { 1.0 }) / n;
            edited.graphs.vertex(m, [x, p[1], p[2]], [0.0, 0.0, 1.0], [x, p[1], p[2]]);
            edited.tri([x, p[1], p[2]], [0.0, 0.0, 1.0], face);
        }
    }
    render_frame(&mut r, &device, &queue, &realistic(edited, 4, 1.0), true);
    let s = r.material_stats();
    assert_eq!((s.compiled, s.pending, s.fallback_draws), (0, 0, 0), "a value edit compiles nothing: {s:?}");
    let uploads = r.uploads();
    assert!(uploads.material_pages >= super::PAGE_BYTES, "the edited page is rewritten: {uploads:?}");
    assert_eq!(uploads.material_vertices, 0, "and no vertex is uploaded again: {uploads:?}");
}

/// The cache keeps at most its capacity, never drops a shape drawn this frame (a shape past it
/// draws its base response), and drops the least recently drawn one for a new shape.
#[test]
fn the_cache_drops_the_least_recently_drawn_shape() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let shapes = octave_shapes();
    let mut r = renderer(&device, &queue);
    r.materials_mut().capacity = 2;
    let scene_of = |list: &[&Lowered]| {
        let mut scene = GpuScene::default();
        for (i, l) in list.iter().enumerate() {
            add_square(&mut scene, l, i, list.len());
        }
        scene
    };
    let [a, b, c, ..] = shapes.as_slice() else { panic!("five shapes") };
    let all = scene_of(&[a, b, c]);
    render_frame(&mut r, &device, &queue, &realistic(all.clone(), 1, 1.0), true);
    render_frame(&mut r, &device, &queue, &realistic(all, 2, 1.0), true);
    let s = r.material_stats();
    assert_eq!((s.ready, s.pending, s.fallback_draws), (2, 1, 1), "full of shapes in use: {s:?}");
    render_frame(&mut r, &device, &queue, &realistic(scene_of(&[b]), 3, 1.0), true);
    render_frame(&mut r, &device, &queue, &realistic(scene_of(&[c]), 4, 1.0), true);
    let m = r.materials_mut();
    assert!(!m.is_ready(&a.ir.shape) && m.is_ready(&b.ir.shape) && m.is_ready(&c.ir.shape), "a, drawn least recently, made room for c");
}

/// Prints the time to generate and validate each fixture's module, and to make its pipeline
/// (§5.4 targets: ≤ 5 ms and ≤ 30 ms native). Run with `--ignored --nocapture`, optimised.
#[test]
#[ignore]
fn shape_compile_times() {
    let device = crate::open_headless().ok();
    for (name, text) in FIXTURES {
        let l = fixture(text);
        let t = web_time::Instant::now();
        let source = super::shape_source(&l.ir).unwrap_or_default();
        let generated = t.elapsed().as_secs_f64() * 1e3;
        let _ = super::validate(&source);
        let validated = t.elapsed().as_secs_f64() * 1e3;
        let pipeline = device.as_ref().map(|(_, device, queue)| {
            let mut r = renderer(device, queue);
            let mut scene = GpuScene::default();
            add_square(&mut scene, &l, 0, 1);
            // The fallback and the AO pipelines are made by a first frame of the copper graph.
            let mut warm = GpuScene::default();
            add_square(&mut warm, &fixture(FIXTURES[4].1), 0, 1);
            render_frame(&mut r, device, queue, &realistic(warm, 1, 1.0), true);
            let t = web_time::Instant::now();
            render_frame(&mut r, device, queue, &realistic(scene, 2, 1.0), true);
            t.elapsed().as_secs_f64() * 1e3
        });
        println!("{name}: generate {generated:.2} ms, + validate {validated:.2} ms; a frame that makes its pipeline {pipeline:?} ms");
    }
}

/// A second target (the path tracer's) gets the same code with its own name, record reads and
/// image samples, and nothing of the raster back end's.
#[test]
fn another_target_shares_the_generated_code() {
    struct Probe;
    impl super::Target for Probe {
        fn name(&self) -> &str {
            "graph_eval_3"
        }
        fn vec4(&self, index: &str) -> String {
            format!("probe_vec4({index})")
        }
        fn image(&self, binding: u8, tc: &str, default: &str, params: super::ImageParams) -> String {
            format!("probe_image({binding}u, {tc}, {default}, {}u)", params.wrap)
        }
    }
    for (name, text) in FIXTURES {
        let ir = fixture(text).ir;
        let raster = super::material_eval(&ir).unwrap_or_default();
        let probe = super::generate(&ir, &Probe).unwrap_or_default();
        assert!(probe.starts_with("fn graph_eval_3(c: sc_GraphCtx) -> sc_Surface {"), "{name}");
        assert!(!probe.contains("mat_"), "{name}: no raster names in another target's code");
        assert_eq!(probe.replace("probe_vec4(", "mat_page.m[").matches("mat_page.m[").count(), raster.matches("mat_page.m[").count(), "{name}");
        if ir.textures.is_empty() {
            assert!(!probe.contains("probe_image"));
        } else {
            assert!(probe.contains("probe_image(0u, "), "{name}: images go through the target");
        }
    }
    struct Bad;
    impl super::Target for Bad {
        fn name(&self) -> &str {
            "evil() {"
        }
        fn vec4(&self, index: &str) -> String {
            index.to_owned()
        }
        fn image(&self, _: u8, _: &str, default: &str, _: super::ImageParams) -> String {
            default.to_owned()
        }
    }
    assert!(super::generate(&fixture(FIXTURES[4].1).ir, &Bad).is_err(), "a target name must be an identifier");
}

// Against the test-only reference (track T): the generated code evaluated on the GPU at fixed
// points equals the f64 interpreter of the same IR.

use solidcraft_render::material_graph::reference::{self, Point, ReferenceSurface};

/// Floats per point in the readback test's input: world and object position, world and object
/// normal, footprint, facing, the object-to-world rows, their inverse's rows and the tangent.
const POINT_FLOATS: usize = 35;

fn point_floats(p: &Point) -> Vec<f32> {
    let m = p.object_to_world;
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let cof = |r: usize, c: usize| {
        let (r0, r1) = ((r + 1) % 3, (r + 2) % 3);
        let (c0, c1) = ((c + 1) % 3, (c + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let inv: Vec<f64> = (0..3).flat_map(|r| (0..3).map(move |c| (r, c))).map(|(r, c)| cof(c, r) / det).collect();
    let mut out: Vec<f64> = Vec::new();
    out.extend(p.world_position);
    out.extend(p.object_position);
    out.extend(p.world_normal);
    out.extend(p.object_normal);
    out.push(p.footprint_mm);
    out.push(p.facing);
    out.extend(m.iter().flatten());
    out.extend(inv);
    out.extend(p.tangent);
    assert_eq!(out.len(), POINT_FLOATS);
    out.into_iter().map(|x| x as f32).collect()
}

const READBACK: &str = r#"
struct MatPage { m: array<vec4<f32>, 1024>, };
@group(0) @binding(0) var<uniform> mat_page: MatPage;
@group(0) @binding(1) var<storage, read> pts: array<f32>;
@group(0) @binding(2) var<storage, read_write> outs: array<f32>;
fn r3(i: u32) -> vec3<f32> { return vec3<f32>(pts[i], pts[i + 1u], pts[i + 2u]); }
@compute @workgroup_size(1)
fn readback(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x * 35u;
    let n = r3(b + 6u);
    let facing = pts[b + 13u];
    let tangent = r3(b + 32u);
    let v = n * facing + tangent * sqrt(max(1.0 - facing * facing, 0.0));
    let to_world = transpose(mat3x3<f32>(r3(b + 14u), r3(b + 17u), r3(b + 20u)));
    let to_object = transpose(mat3x3<f32>(r3(b + 23u), r3(b + 26u), r3(b + 29u)));
    let c = sc_GraphCtx(0u, v, n, tangent, facing, r3(b + 9u), r3(b), r3(b + 3u), pts[b + 12u], sc_ObjectFrame(to_object, to_world));
    let s = material_eval(c);
    let f = array<f32, 37>(s.base_color.x, s.base_color.y, s.base_color.z, s.metalness, s.roughness, s.ior, s.coat_weight,
        s.coat_roughness, s.coat_ior, s.normal.x, s.normal.y, s.normal.z, s.coat_normal.x, s.coat_normal.y, s.coat_normal.z,
        s.specular_weight, s.specular_color.x, s.specular_color.y, s.specular_color.z, s.anisotropy, s.rotation, s.tangent.x,
        s.tangent.y, s.tangent.z, s.coat_color.x, s.coat_color.y, s.coat_color.z, s.emission_luminance, s.emission_color.x,
        s.emission_color.y, s.emission_color.z, s.transmission_weight, s.transmission_color.x, s.transmission_color.y,
        s.transmission_color.z, s.transmission_depth_mm, s.opacity);
    for (var k = 0u; k < 37u; k += 1u) { outs[id.x * 37u + k] = f[k]; }
}
"#;

/// The generated surface of `ir` (record words `words`, the base response in front) at `points`,
/// evaluated by a compute shader.
fn gpu_surfaces(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ir: &Ir,
    words: &[u32],
    base: &SurfaceSample,
    points: &[Point],
) -> Result<Vec<Vec<f32>>, String> {
    gpu_surfaces_with(device, queue, ir, words, base, points, None)
}

/// [`gpu_surfaces`] with the material textures bound (group 3, as the raster pipelines bind them;
/// `None`: the 1 × 1 stand-in in every binding).
pub(super) fn gpu_surfaces_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ir: &Ir,
    words: &[u32],
    base: &SurfaceSample,
    points: &[Point],
    textures: Option<[&wgpu::TextureView; 4]>,
) -> Result<Vec<Vec<f32>>, String> {
    use wgpu::util::DeviceExt;
    let source =
        format!("{}\n{READBACK}\n{}\n{}", crate::shading::library(), super::textures::sampling_source(), super::material_eval(ir).map_err(|e| e.0)?);
    super::validate(&source).map_err(|e| e.0)?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(source.into()) });
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
    let storage = |read_only| wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let group0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
            entry(1, storage(true)),
            entry(2, storage(false)),
        ],
    });
    let texture = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    let sampler = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
    let group3 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            entry(4, texture),
            entry(5, texture),
            entry(6, texture),
            entry(7, texture),
            entry(8, sampler),
            entry(9, sampler),
            entry(10, sampler),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&group0), None, None, Some(&group3)],
        immediate_size: 0,
    });
    let blank = device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255; 4],
        )
        .create_view(&Default::default());
    let views = textures.unwrap_or([&blank, &blank, &blank, &blank]);
    let samplers = [wgpu::AddressMode::Repeat, wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::MirrorRepeat].map(|mode| {
        device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: mode,
            address_mode_v: mode,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        })
    });
    let mut entries: Vec<wgpu::BindGroupEntry> = views
        .iter()
        .enumerate()
        .map(|(i, v)| wgpu::BindGroupEntry { binding: 4 + i as u32, resource: wgpu::BindingResource::TextureView(v) })
        .collect();
    entries.extend(
        samplers.iter().enumerate().map(|(i, s)| wgpu::BindGroupEntry { binding: 8 + i as u32, resource: wgpu::BindingResource::Sampler(s) }),
    );
    let textures_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &group3, entries: &entries });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &module,
        entry_point: Some("readback"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut page: Vec<u32> = super::base_record(base).iter().map(|x| x.to_bits()).collect();
    page.extend_from_slice(words);
    page.resize(super::PAGE_VEC4 as usize * 4, 0);
    let bytes = |w: &[u32]| w.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let uniform =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &bytes(&page), usage: wgpu::BufferUsages::UNIFORM });
    let input: Vec<u32> = points.iter().flat_map(point_floats).map(f32::to_bits).collect();
    let pts =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &bytes(&input), usage: wgpu::BufferUsages::STORAGE });
    let size = (points.len() * 37 * 4) as u64;
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
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &group0,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: pts.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: out.as_entire_binding() },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.set_bind_group(3, &textures_bind, &[]);
        pass.dispatch_workgroups(points.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out, 0, &read, 0, size);
    queue.submit([encoder.finish()]);
    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
    let data = read.slice(..).get_mapped_range().map_err(|e| e.to_string())?.to_vec();
    let floats: Vec<f32> = data.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    Ok(floats.chunks(37).map(<[f32]>::to_vec).collect())
}

/// T's sample points, a filtered copy of each (fades) and placed copies (object space).
fn reference_points() -> Vec<Point> {
    let mut points = reference::sample_points();
    let filtered: Vec<Point> = points.iter().map(|p| p.with_footprint(0.02)).collect();
    let (c, s) = (0.6f64, 0.8f64);
    let turn = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
    let placed: Vec<Point> = points.iter().filter_map(|p| p.placed(turn, [3.0, -1.0, 2.0]).ok()).collect();
    points.extend(filtered);
    points.extend(placed);
    points
}

/// The worst difference between the GPU's and the reference's surface of a graph at the points.
fn worst_against_reference(device: &wgpu::Device, queue: &wgpu::Queue, text: &str) -> Result<(String, f64), String> {
    let graph: MaterialGraph = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let l = lowered(&graph)?;
    let (ir, words) = reference::prepare_json(text, &BTreeMap::new()).map_err(|e| e.0)?;
    if ir.shape != l.ir.shape {
        return Err("the reference lowers another shape".to_owned());
    }
    let points = reference_points();
    let got = gpu_surfaces(device, queue, &ir, &words, &l.base, &points)?;
    let mut worst = (String::new(), 0.0);
    for (i, (p, g)) in points.iter().zip(&got).enumerate() {
        let mut want = reference::evaluate(&ir, &words, &[], p).map_err(|e| e.0)?.surface;
        let mut have = ReferenceSurface::from_floats(g).map_err(|e| e.0)?;
        // The tangent is zeroed at exactly zero anisotropy: below 1e-6 in both it shades nothing,
        // and f32 and f64 can land on either side of the rule.
        if want.anisotropy.abs() < 1e-6 && have.anisotropy.abs() < 1e-6 {
            (want.tangent, have.tangent) = ([0.0; 3], [0.0; 3]);
        }
        let (field, d) = want.max_difference(&have);
        if d > worst.1 {
            worst = (format!("{field} at point {i}"), d);
        }
    }
    Ok(worst)
}

/// Every fixture evaluates on the GPU as the reference does, at unfiltered, filtered and placed
/// points.
#[test]
fn the_fixtures_match_the_reference() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    for (name, text) in FIXTURES {
        let (field, d) = worst_against_reference(&device, &queue, text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(d < 1e-3, "{name}: {field} differs by {d}");
    }
}

/// The generated corpus evaluates on the GPU as the reference does.
#[test]
fn the_generated_corpus_matches_the_reference() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let mut fuzz = Fuzz(0x9e37_79b9_7f4a_7c15);
    let mut bad = Vec::new();
    let mut compared = 0;
    for i in 0..96 {
        let text = fuzz.graph().to_string();
        if lowered(&graph_of(serde_json::from_str(&text).unwrap_or_default())).is_err() {
            continue;
        }
        compared += 1;
        match worst_against_reference(&device, &queue, &text) {
            Ok((field, d)) if d >= 1e-3 => bad.push(format!("graph {i}: {field} by {d}")),
            Ok(_) => {}
            Err(e) => bad.push(format!("graph {i}: {e}")),
        }
    }
    assert!(compared >= 64, "{compared} compared");
    assert!(bad.is_empty(), "{} of {compared} graphs differ:\n{}", bad.len(), bad.join("\n"));
}

/// Prints a corpus graph's code and every point where it differs from the reference, with the
/// reference's instruction values: `GRAPHS=3,17 cargo test … debug_corpus_graph -- --ignored --nocapture`.
#[test]
#[ignore]
fn debug_corpus_graph() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let wanted: Vec<usize> = std::env::var("GRAPHS").unwrap_or_default().split(',').filter_map(|s| s.parse().ok()).collect();
    let mut fuzz = Fuzz(0x9e37_79b9_7f4a_7c15);
    for i in 0..96 {
        let text = fuzz.graph().to_string();
        if !wanted.contains(&i) {
            continue;
        }
        let (ir, words) = reference::prepare_json(&text, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
        let l = lowered(&graph_of(serde_json::from_str(&text).unwrap_or_default())).unwrap_or_else(|e| panic!("{e}"));
        let points = reference_points();
        let got = gpu_surfaces(&device, &queue, &ir, &words, &l.base, &points).unwrap_or_else(|e| panic!("{e}"));
        println!("== graph {i}\n{text}\n{}", super::material_eval(&ir).unwrap_or_default());
        for (k, (p, g)) in points.iter().zip(&got).enumerate() {
            let e = reference::evaluate(&ir, &words, &[], p).unwrap_or_else(|e| panic!("{e}"));
            let have = ReferenceSurface::from_floats(g).unwrap_or_else(|e| panic!("{e}"));
            let (field, d) = e.surface.max_difference(&have);
            if d > 1e-3 {
                println!("point {k}: {field} by {d}; want {:?}\n got {:?}", e.surface.to_floats(), have.to_floats());
                for (j, v) in e.values.iter().enumerate() {
                    println!("  v{j} = {v:?}");
                }
            }
        }
    }
}

/// Slopes through vector maths (a component-wise multiply, add, mix, abs, power and clamp of a
/// vector3, and of a position) into a height: the GPU carries each component's gradient as the
/// reference does, so the relief survives.
#[test]
fn vector_slopes_match_the_reference() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let graph = |op: &str, extra: Value, source: &str| {
        json!({"name": "vector slope", "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}, "inputs": {"u": [0.7, 0, 0], "v": [0, 0.9, 0]}},
            {"id": "n", "kind": "pattern.noise", "params": {"kind": "gradient"}, "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 0.8}},
            {"id": "w", "kind": "pattern.weave", "inputs": {"texcoord": {"from": "uv"}, "pitch_mm": [0.6, 0.8]}},
            {"id": "v", "kind": "combine3", "inputs": {"x": {"from": "n"}, "y": {"from": "w"}, "z": {"from": "n"}}},
            {"id": "p", "kind": "geometry.position", "params": {"space": "object"}},
            {"id": "op", "kind": op, "inputs": extra},
            {"id": "s", "kind": "separate3", "inputs": {"in": {"from": "op"}}},
            {"id": "h", "kind": "add", "inputs": {"a": {"from": "s.y"}, "b": {"from": "s.z"}}},
            {"id": "nh", "kind": "normal.from_height", "inputs": {"height": {"from": "h"}, "relief_mm": 0.02}},
            {"id": "out", "kind": "standard_surface", "inputs": {"geometry_normal": {"from": "nh"}}}], "output": "out"})
        .to_string()
        .replace("SRC", source)
    };
    let cases = [
        graph("multiply", json!({"a": {"from": "SRC"}, "b": [2.0, 1.5, 0.5]}), "v"),
        graph("add", json!({"a": {"from": "SRC"}, "b": {"from": "v"}}), "v"),
        graph("mix", json!({"a": {"from": "SRC"}, "b": [0.2, 0.9, 0.4], "t": {"from": "n"}}), "v"),
        graph("abs", json!({"in": {"from": "SRC"}}), "v"),
        graph("power", json!({"a": {"from": "SRC"}, "b": 2.5}), "v"),
        graph("clamp", json!({"in": {"from": "SRC"}, "low": 0.3, "high": 0.7}), "v"),
        graph("sin", json!({"in": {"from": "SRC"}}), "p"),
        graph("normalize", json!({"in": {"from": "SRC"}}), "v"),
        graph("multiply", json!({"a": {"from": "SRC"}, "b": {"from": "v"}}), "p"),
    ];
    for (i, text) in cases.iter().enumerate() {
        let (field, d) = worst_against_reference(&device, &queue, text).unwrap_or_else(|e| panic!("case {i}: {e}"));
        assert!(d < 1e-3, "case {i}: {field} differs by {d}\n{text}");
        // The relief is there: the normal is not the geometric one everywhere.
        let (ir, words) = reference::prepare_json(text, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
        let tilted = reference::sample_points()
            .iter()
            .any(|p| reference::evaluate(&ir, &words, &[], p).is_ok_and(|e| (e.surface.normal[2] - p.world_normal[2]).abs() > 1e-4));
        assert!(tilted, "case {i}: the height tilts the normal");
    }
}

/// C2's rule for the blended class: a surface that drives transmission or opacity, or holds a
/// transmission above 0 or an opacity below 1, in any layer.
#[test]
fn see_through_materials_are_the_ones_with_coverage_below_one() {
    let surface = |inputs: Value| {
        lowered(&graph_of(json!({"name": "s", "nodes": [
            {"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}},
            {"id": "n", "kind": "pattern.noise", "inputs": {"texcoord": {"from": "uv"}}},
            {"id": "s", "kind": "standard_surface", "inputs": inputs}], "output": "s"})))
        .unwrap_or_else(|e| panic!("{e}"))
    };
    let words = |l: &Lowered| uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
    for (inputs, want) in [
        (json!({}), false),
        (json!({"geometry_opacity": 1}), false),
        (json!({"geometry_opacity": 0.4}), true),
        (json!({"transmission_weight": 0.3}), true),
        (json!({"transmission_weight": 0}), false),
        (json!({"geometry_opacity": {"from": "n"}}), true),
    ] {
        let l = surface(inputs.clone());
        assert_eq!(super::see_through(&l.ir, &words(&l)), want, "{inputs}");
    }
}

/// A see-through graph surface is drawn blended over what lies behind it (premultiplied, as C2's
/// shading returns it): a transmissive, or half cut-out, blue square over a red face shows both.
#[test]
fn a_see_through_graph_blends_over_what_lies_behind() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let glass = |transmission: f64| {
        lowered(&graph_of(json!({"name": "glass", "nodes": [
            {"id": "s", "kind": "standard_surface", "inputs": {"base_color": [0.05, 0.1, 0.9], "transmission_weight": transmission, "specular_roughness": 0.6}}],
            "output": "s"})))
        .unwrap_or_else(|e| panic!("{e}"))
    };
    let mut r = renderer(&device, &queue);
    let frame_with = |r: &mut Renderer, l: Option<&Lowered>, key: u64| {
        let mut scene = GpuScene::default();
        let red = scene.color([230, 20, 20, 255]);
        for p in SQUARE {
            scene.tri([p[0], p[1], -1.0], [0.0, 0.0, 1.0], red);
        }
        if let Some(l) = l {
            let words = uniform_words(&l.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
            let m = scene.graphs.material(&l.ir, &words, &l.base).unwrap_or_else(|e| panic!("{e}"));
            for p in SQUARE {
                scene.graphs.vertex(m, p, [0.0, 0.0, 1.0], p);
            }
        }
        let pixels = inner_pixels(&render_frame(r, &device, &queue, &realistic(scene, key, 1.0), true));
        let n = pixels.len().max(1) as f64;
        [0, 1, 2].map(|k| pixels.iter().map(|p| f64::from(p[k])).sum::<f64>() / n)
    };
    let behind = frame_with(&mut r, None, 1);
    let half = glass(0.6);
    assert!(super::see_through(&half.ir, &uniform_words(&half.ir, &BTreeMap::new()).unwrap_or_default()));
    let blended = frame_with(&mut r, Some(&half), 2);
    let stats = r.material_stats();
    assert_eq!(stats.fallback_draws, 0, "drawn by its own far and near pipelines: {stats:?}");
    assert!(blended[2] > behind[2] + 15.0, "the blue surface shows: {blended:?} over {behind:?}");
    assert!(blended[0] > 0.3 * behind[0], "and the red face shows through it: {blended:?} over {behind:?}");
    let opaque = frame_with(&mut r, Some(&glass(0.0)), 3);
    assert!(opaque[0] < 0.2 * behind[0], "without transmission it hides the face: {opaque:?}");
    assert!(blended[0] > opaque[0] + 20.0, "transmission lets the face through: {blended:?} against {opaque:?}");
    // A cut-out (opacity ½, C2's coverage) blends the same way.
    let cut = lowered(&graph_of(json!({"name": "cut", "nodes": [
        {"id": "s", "kind": "standard_surface", "inputs": {"base_color": [0.05, 0.1, 0.9], "geometry_opacity": 0.5, "specular_roughness": 0.6}}],
        "output": "s"})))
    .unwrap_or_else(|e| panic!("{e}"));
    let half = frame_with(&mut r, Some(&cut), 4);
    assert!(half[0] > opaque[0] + 40.0 && half[2] > behind[2] + 15.0, "half the face, half the surface: {half:?}");
}

/// A texture the scene gives only as encoded bytes is decoded on the renderer's worker and made
/// resident: until then its `image` node draws its default colour; afterwards the image itself
/// (red on the left half of the square, blue on the right).
#[test]
fn scene_textures_are_decoded_and_drawn() {
    let Some((_, device, queue)) = crate::open_headless().ok() else { return };
    let key = "ab".repeat(32);
    let graph = lowered(&graph_of(json!({"name": "label", "nodes": [
        {"id": "uv", "kind": "texcoord.planar", "params": {"space": "world"}, "inputs": {"origin": [-12, -12, 0], "u": [24, 0, 0], "v": [0, 24, 0]}},
        {"id": "img", "kind": "image", "params": {"space": "srgb", "wrap": "clamp"}, "inputs": {"texcoord": {"from": "uv"}, "file": {"texture": key}, "default": [0.0, 0.8, 0.0]}},
        {"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"from": "img"}, "specular_roughness": 0.9}}], "output": "s"})))
    .unwrap_or_else(|e| panic!("{e}"));
    let mut png = Vec::new();
    let img = image::RgbaImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) });
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap_or_else(|e| panic!("{e}"));
    let words = uniform_words(&graph.ir, &BTreeMap::new()).unwrap_or_else(|e| panic!("{e}"));
    let set = super::texture_keys(&graph.ir, &BTreeMap::new());
    assert!(set[0].is_some(), "the constant texture is bound");
    let scene = || {
        let mut scene = GpuScene::default();
        let m = scene.graphs.textured_material(&graph.ir, &words, &graph.base, &set).unwrap_or_else(|e| panic!("{e}"));
        for k in set.iter().flatten() {
            scene.graphs.texture_source(k.clone(), Arc::new(png.clone()));
        }
        let face = scene.color([200, 200, 200, 255]);
        for p in SQUARE {
            scene.graphs.vertex(m, p, [0.0, 0.0, 1.0], p);
            scene.tri(p, [0.0, 0.0, 1.0], face);
        }
        scene
    };
    let mut r = renderer(&device, &queue);
    let side = |bytes: &[u8], x: usize| {
        let at = (32 * SIZE as usize + x) * 4;
        bytes.get(at..at + 3).map(|p| [p[0], p[1], p[2]]).unwrap_or_default()
    };
    let first = render_frame(&mut r, &device, &queue, &realistic(scene(), 1, 1.0), true);
    let mut last = first.clone();
    for key_n in (2..).take(200) {
        if !r.wants_redraw() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        last = render_frame(&mut r, &device, &queue, &realistic(scene(), key_n, 1.0), true);
    }
    assert!(!r.wants_redraw(), "the texture finished decoding");
    let (left, right) = (side(&last, 14), side(&last, 50));
    assert!(left[0] > left[2] + 40 && right[2] > right[0] + 40, "red left, blue right: {left:?} {right:?}");
    let before = side(&first, 14);
    assert!(before[1] > before[0] || first == last, "before decoding the default colour (green) shows: {before:?}");
}
