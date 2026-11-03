//! Track A: validation, lowering, the cost model, the shape key, the base response and uniform
//! words, against the fixtures and against hostile graphs.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::catalogue::{CATALOGUE, InputDefault, ParamKind, PortType};
use super::graph::Constant;
use super::ir::{GeometryInput, Operand};
use super::*;
use crate::clear_coat::ClearCoat;
use crate::surface::{OpaqueSurface, srgb8_to_linear};

fn fixture(name: &str) -> MaterialGraph {
    let text = match name {
        "black-epoxy-clear-coat" => include_str!("../../tests/fixtures/material_graph/black-epoxy-clear-coat.json"),
        "brushed-nickel-silver" => include_str!("../../tests/fixtures/material_graph/brushed-nickel-silver.json"),
        "electrolytic-sleeve" => include_str!("../../tests/fixtures/material_graph/electrolytic-sleeve.json"),
        "laser-etched-nickel-silver" => include_str!("../../tests/fixtures/material_graph/laser-etched-nickel-silver.json"),
        "pbr-copper" => include_str!("../../tests/fixtures/material_graph/pbr-copper.json"),
        "solder-mask-over-copper" => include_str!("../../tests/fixtures/material_graph/solder-mask-over-copper.json"),
        _ => panic!("no fixture {name}"),
    };
    serde_json::from_str(text).unwrap()
}

fn from_json(v: Value) -> MaterialGraph {
    serde_json::from_value(v).unwrap()
}

fn ir_of(graph: &MaterialGraph) -> Ir {
    lower(&validate(graph).unwrap_or_else(|e| panic!("{}: {e:?}", graph.name))).unwrap()
}

fn codes(graph: &MaterialGraph) -> Vec<&'static str> {
    match validate(graph) {
        Err(GraphError::Invalid(issues)) => issues.iter().map(|i| i.code).collect(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A one-surface graph with `inputs` on the surface and `nodes` before it.
fn surface_graph(nodes: Value, inputs: Value) -> MaterialGraph {
    let mut list = nodes.as_array().cloned().unwrap_or_default();
    list.push(json!({"id": "surface", "kind": "standard_surface", "inputs": inputs}));
    from_json(json!({"name": "test", "nodes": list, "output": "surface"}))
}

#[test]
fn every_fixture_validates_lowers_and_costs_what_the_note_says() {
    // (fixture, expanded cost, texture samples, textures, mix layers, object space)
    let expected = [
        // planar 4 + brushed 30 + 2 × remap 2 + multiply 1 (§3.8).
        ("brushed-nickel-silver", 39, 0, 0, 0, true),
        // planar 4 + image 8 + cellular 2D noise 60 × slope 3 + from_height 6 + mix 10.
        ("laser-etched-nickel-silver", 208, 1, 1, 1, true),
        // planar 4 + 2 × value noise 20 × 3 + (multiply 1 + add 1) × 3 + from_height 6.
        ("solder-mask-over-copper", 136, 0, 0, 0, true),
        // cellular 3D noise, 2 octaves: 20 × 2 × 2 × 3, + remap 2.
        ("black-epoxy-clear-coat", 242, 0, 0, 0, true),
        // cylindrical 8 + smoothstep 4 + image 8 + max 1 + mix 10; separate3 is free.
        ("electrolytic-sleeve", 31, 1, 1, 1, true),
        ("pbr-copper", 0, 0, 0, 0, false),
    ];
    for (name, cost, samples, textures, layers, object) in expected {
        let ir = ir_of(&fixture(name));
        assert_eq!(ir.cost, cost, "{name} cost");
        assert_eq!(ir.samples, samples, "{name} samples");
        assert_eq!(ir.textures.len(), textures, "{name} textures");
        assert_eq!(ir.mix_layers.len(), layers, "{name} layers");
        assert_eq!(ir.object_space, object, "{name} object space");
        assert_eq!(ir.surface.len(), 22, "{name}: every surface input");
        assert!(ir.uniform_floats as usize <= limits::UNIFORM_FLOATS && ir.uniform_floats.is_multiple_of(4), "{name}");
        for instr in &ir.instrs {
            assert_ne!(instr.ty, PortType::Numeric, "{name}: {} unresolved", instr.op);
        }
        // Every slot sits inside the material's run and no value straddles a vec4.
        for slot in &ir.slots {
            assert!(slot.offset + u32::from(slot.width) <= ir.uniform_floats, "{name}");
            assert_eq!(slot.offset / 4, (slot.offset + u32::from(slot.width) - 1) / 4, "{name}: slot straddles a vec4");
            assert_eq!(slot.value.len(), usize::from(slot.width));
        }
        assert_eq!(lower(&validate(&fixture(name)).unwrap()).unwrap(), ir, "{name}: lowering is deterministic");
    }
}

#[test]
fn the_brushed_example_lowers_to_the_documented_ir() {
    let ir = ir_of(&fixture("brushed-nickel-silver"));
    let ops: Vec<&str> = ir.instrs.iter().map(|i| i.op).collect();
    // base_color's multiply comes first (catalogue order), the tangent selection last.
    assert_eq!(ops, ["texcoord.planar", "pattern.brushed", "remap", "multiply", "remap", "pattern.brushed.tangent"]);
    let tangent = ir.instrs.last().unwrap();
    assert_eq!((tangent.kind(), tangent.selects(), tangent.ty), ("pattern.brushed", Some("tangent"), PortType::Vector3));
    let multiply = &ir.instrs[3];
    assert_eq!(multiply.ty, PortType::Color3, "an untyped triple takes the colour meaning of base_color");
    let surface: BTreeMap<_, _> = ir.surface.iter().copied().collect();
    assert_eq!(surface["base_color"], Operand::Value(multiply.result));
    assert_eq!(surface["geometry_tangent"], Operand::Value(tangent.result));
    assert_eq!(surface["geometry_normal"], Operand::Geometry(GeometryInput::WorldNormal));
    assert!(matches!(surface["base_metalness"], Operand::Uniform { width: 1, .. }));
    assert!(ir.instrs.iter().all(|i| !i.slope && !i.triplanar));
    // The seed is an integer slot holding 7.
    let seed = ir.slots.iter().find(|s| s.ty == PortType::Int).unwrap();
    let words = uniform_words(&ir, &BTreeMap::new()).unwrap();
    assert_eq!(words[seed.offset as usize], 7);
}

#[test]
fn slopes_and_triplanar_expansion_are_marked() {
    let ir = ir_of(&fixture("solder-mask-over-copper"));
    let sloped: Vec<&str> = ir.instrs.iter().filter(|i| i.slope).map(|i| i.op).collect();
    assert_eq!(sloped, ["pattern.noise", "pattern.noise", "multiply", "add"]);
    let tri = surface_graph(
        json!([
            {"id": "t", "kind": "texcoord.triplanar"},
            {"id": "w", "kind": "pattern.weave", "inputs": {"texcoord": {"from": "t"}}},
            {"id": "n", "kind": "normal.from_height", "inputs": {"height": {"from": "w"}}}
        ]),
        json!({"base_color": {"from": "w"}, "geometry_normal": {"from": "n"}}),
    );
    let ir = ir_of(&tri);
    let weave = ir.instrs.iter().find(|i| i.op == "pattern.weave").unwrap();
    assert!(weave.slope && weave.triplanar);
    assert!(!ir.instrs.iter().find(|i| i.op == "texcoord.triplanar").unwrap().triplanar);
    // triplanar 6 + weave 60 × 3 × 3 + from_height 6.
    assert_eq!(ir.cost, 6 + 540 + 6);
}

#[test]
fn pbr_copper_base_response_is_the_pbr_look_exactly() {
    let base = base_response(&validate(&fixture("pbr-copper")).unwrap()).unwrap();
    // As `PbrParameters::surface` builds the "Copper - Clean PBR" look.
    let [r, g, b] = [204u8, 122, 80].map(srgb8_to_linear);
    let pbr = OpaqueSurface::new(Vec3::new(r, g, b), 1.0, 0.25, 1.5).unwrap();
    assert_eq!(base, pbr);
    let sample = base_sample(&validate(&fixture("pbr-copper")).unwrap()).unwrap();
    assert_eq!(sample.as_opaque(), Some(pbr), "default extensions");
}

#[test]
fn base_responses_read_constants_fallbacks_and_the_bg_surface() {
    let coat = base_response(&validate(&fixture("solder-mask-over-copper")).unwrap()).unwrap();
    assert_eq!(coat.base_color(), Vec3::new(0.955, 0.638, 0.538));
    assert_eq!(coat.clear_coat(), Some(ClearCoat::new(1.0, 0.12, 1.52).unwrap()));
    let sample = base_sample(&validate(&fixture("solder-mask-over-copper")).unwrap()).unwrap();
    assert_eq!(sample.coat_color, Vec3::new(0.10, 0.45, 0.12));
    // A mix shows its bg: the plain metal, and the sleeve's interface default.
    let etched = base_response(&validate(&fixture("laser-etched-nickel-silver")).unwrap()).unwrap();
    assert_eq!((etched.base_color(), etched.roughness()), (Vec3::new(0.80, 0.77, 0.70), 0.45));
    let sleeve = base_response(&validate(&fixture("electrolytic-sleeve")).unwrap()).unwrap();
    assert_eq!((sleeve.base_color(), sleeve.dielectric_ior()), (Vec3::new(0.020, 0.045, 0.16), 1.54));
    // A driven input reads its fallback, else the input's default.
    let epoxy = base_response(&validate(&fixture("black-epoxy-clear-coat")).unwrap()).unwrap();
    assert_eq!(epoxy.roughness(), 0.3, "no fallback: the catalogue default");
    let mut graph = fixture("black-epoxy-clear-coat");
    let surface = graph.nodes.iter_mut().find(|n| n.id == "surface").unwrap();
    surface.inputs.insert("specular_roughness".into(), serde_json::from_value(json!({"from": "edm_r", "fallback": 0.46})).unwrap());
    assert_eq!(base_response(&validate(&graph).unwrap()).unwrap().roughness(), 0.46);
}

#[test]
fn the_shape_ignores_values_layout_ids_and_order() {
    let key = |g: &MaterialGraph| ir_of(g).shape;
    let original = fixture("brushed-nickel-silver");
    let mut text = serde_json::to_value(&original).unwrap();
    // New values everywhere, a layout, a different name and provenance.
    text["name"] = json!("Brushed steel");
    text["nodes"][1]["inputs"]["density_per_mm"] = json!(12);
    text["nodes"][1]["inputs"]["seed"] = json!(99);
    text["nodes"][4]["inputs"]["a"] = json!([0.5, 0.5, 0.52]);
    text["nodes"][5]["inputs"]["base_metalness"] = json!(0.9);
    text["layout"] = json!({"uv": [10.0, 20.0], "surface": [300.0, 0.0]});
    text["source"] = json!({"library": "lib", "entry": "brushed", "revision": 3});
    assert_eq!(key(&from_json(text.clone())), key(&original));
    // An explicit default is the same as an omitted one.
    let mut explicit = text.clone();
    explicit["nodes"][5]["inputs"]["coat_weight"] = json!(0);
    assert_eq!(key(&from_json(explicit)), key(&original));
    // Other ids and another node order.
    let mut renamed = serde_json::to_string(&original).unwrap().replace("\"lines", "\"streaks");
    renamed = renamed.replace("\"rough\"", "\"r2\"");
    let mut renamed: Value = serde_json::from_str(&renamed).unwrap();
    renamed["nodes"].as_array_mut().unwrap().reverse();
    assert_eq!(key(&from_json(renamed)), key(&original));
    // Params, topology and constant-versus-connection change it.
    let mut world = text.clone();
    world["nodes"][0]["params"]["space"] = json!("world");
    assert_ne!(key(&from_json(world)), key(&original));
    let mut unwired = text;
    unwired["nodes"][5]["inputs"]["specular_roughness"] = json!(0.4);
    assert_ne!(key(&from_json(unwired)), key(&original));
}

#[test]
fn uniform_words_take_look_values_and_refuse_bad_ones() {
    let ir = ir_of(&fixture("electrolytic-sleeve"));
    let sleeve = ir.slots.iter().find(|s| s.interface.as_deref() == Some("sleeve")).unwrap();
    assert_eq!((sleeve.ty, sleeve.width, sleeve.range), (PortType::Color3, 3, Some((0.0, 1.0))));
    let at = sleeve.offset as usize;
    let defaults = uniform_words(&ir, &BTreeMap::new()).unwrap();
    assert_eq!(f32::from_bits(defaults[at + 2]), 0.16);
    let red =
        BTreeMap::from([("sleeve".to_owned(), Constant::Srgb8(graph::Srgb8 { srgb8: [255, 0, 0] })), ("print".to_owned(), Constant::Number(3.0))]);
    let words = uniform_words(&ir, &red).unwrap();
    assert_eq!(&words[at..at + 3], &[1.0f32.to_bits(), 0, 0]);
    assert_eq!(words.len(), ir.uniform_floats as usize);
    // The texture is bound by interface, not a uniform.
    assert_eq!(ir.textures[0].interface(), Some("print"));
    assert_eq!(ir.textures[0].sha256(), None);
    for bad in [Constant::Vector(vec![2.0, 0.0, 0.0]), Constant::Number(f64::NAN), Constant::Vector(vec![0.1, 0.2])] {
        let values = BTreeMap::from([("sleeve".to_owned(), bad)]);
        assert!(uniform_words(&ir, &values).is_err());
    }
}

#[test]
fn refusals_name_what_is_wrong() {
    let cases: Vec<(MaterialGraph, &str)> = vec![
        (surface_graph(json!([{"id": "x", "kind": "pattern.marble"}]), json!({})), "unknown_kind"),
        (surface_graph(json!([]), json!({"subsurface_weight": 1})), "unknown_input"),
        (surface_graph(json!([]), json!({"base_metalness": 1.5})), "out_of_range"),
        (surface_graph(json!([]), json!({"specular_roughness": 0.0})), "out_of_range"),
        (surface_graph(json!([]), json!({"base_color": [0.1, 0.2]})), "constant_type"),
        (surface_graph(json!([]), json!({"base_color": [0.1, 0.2, 0.3, 0.4]})), "constant_form"),
        (surface_graph(json!([{"id": "c", "kind": "constant", "inputs": {"value": 1e300}}]), json!({"base_metalness": {"from": "c"}})), "not_finite"),
        (surface_graph(json!([{"id": "p", "kind": "texcoord.planar"}]), json!({"base_color": {"from": "p.uv"}})), "type_mismatch"),
        (surface_graph(json!([{"id": "p", "kind": "texcoord.planar"}]), json!({"base_color": {"from": "p.nope"}})), "unknown_output"),
        (surface_graph(json!([]), json!({"base_color": {"from": "ghost"}})), "unknown_node"),
        (surface_graph(json!([{"id": "c", "kind": "pattern.checker"}]), json!({"base_metalness": {"from": "c"}})), "required_input"),
        (
            surface_graph(
                json!([{"id": "a", "kind": "add", "inputs": {"a": {"from": "b"}}}, {"id": "b", "kind": "add", "inputs": {"a": {"from": "a"}}}]),
                json!({"base_metalness": {"from": "a"}}),
            ),
            "cycle",
        ),
        (surface_graph(json!([{"id": "a", "kind": "add", "inputs": {"a": {"from": "a"}}}]), json!({"base_metalness": {"from": "a"}})), "cycle"),
        (surface_graph(json!([{"id": "surface", "kind": "constant"}]), json!({})), "duplicate_id"),
        (surface_graph(json!([{"id": "Bad Id", "kind": "constant"}]), json!({})), "id"),
        (
            surface_graph(json!([{"id": "s2", "kind": "standard_surface"}]), json!({"base_metalness": {"from": "s2", "fallback": 0.5}})),
            "type_mismatch",
        ),
        (surface_graph(json!([]), json!({"base_metalness": {"from": "surface"}})), "cycle"),
        (
            surface_graph(
                json!([{"id": "c", "kind": "constant", "inputs": {"value": 0.5}}]),
                json!({"base_metalness": {"from": "c", "fallback": 2.0}}),
            ),
            "out_of_range",
        ),
        (surface_graph(json!([{"id": "c", "kind": "constant", "inputs": {"value": [1, 2]}}]), json!({"base_color": {"from": "c"}})), "type_mismatch"),
        (surface_graph(json!([{"id": "n", "kind": "pattern.noise", "params": {"octaves": 9}}]), json!({})), "param_value"),
        (surface_graph(json!([{"id": "n", "kind": "pattern.noise", "params": {"kind": "perlin"}}]), json!({})), "param_value"),
        (surface_graph(json!([{"id": "n", "kind": "pattern.noise", "params": {"warp": 1}}]), json!({})), "unknown_param"),
        (surface_graph(json!([{"id": "n", "kind": "pattern.noise"}]), json!({"base_metalness": {"from": "n"}})), "noise_source"),
        (
            surface_graph(
                json!([{"id": "p", "kind": "texcoord.planar", "inputs": {"u": [1, 0, 0], "v": [1, 1, 0]}}, {"id": "c", "kind": "pattern.checker", "inputs": {"texcoord": {"from": "p"}}}]),
                json!({"base_metalness": {"from": "c"}}),
            ),
            "degenerate_projection",
        ),
        (surface_graph(json!([]), json!({"transmission_weight": 0.5, "base_metalness": 1})), "transmission_conflict"),
        (
            surface_graph(
                json!([{"id": "s", "kind": "constant", "inputs": {"value": 1}}]),
                json!({"base_color": {"from": "s"}, "coat_weight": {"from": "s.value"}}),
            ),
            "unknown_output",
        ),
    ];
    for (graph, code) in cases {
        let found = codes(&graph);
        assert!(found.contains(&code), "{code} expected, got {found:?} for {}", serde_json::to_string(&graph).unwrap());
    }
    // The output must be a surface; a mix's fg is a standard surface in v1.
    let g = from_json(json!({"name": "x", "nodes": [{"id": "c", "kind": "constant"}], "output": "c"}));
    assert!(codes(&g).contains(&"output"));
    let g = from_json(json!({"name": "x", "nodes": [
        {"id": "a", "kind": "standard_surface"}, {"id": "b", "kind": "standard_surface"},
        {"id": "m1", "kind": "surface.mix", "inputs": {"fg": {"from": "a"}, "bg": {"from": "b"}}},
        {"id": "m2", "kind": "surface.mix", "inputs": {"fg": {"from": "m1"}, "bg": {"from": "b"}}}], "output": "m2"}));
    assert!(codes(&g).contains(&"mix_layer"));
    // Interface inputs: types, names, defaults.
    let g = from_json(
        json!({"name": "x", "interface": [{"name": "n", "type": "normal"}], "nodes": [{"id": "s", "kind": "standard_surface"}], "output": "s"}),
    );
    assert!(codes(&g).contains(&"interface_type"));
    let g = from_json(json!({"name": "x", "interface": [{"name": "c", "type": "color3", "default": [2, 0, 0]}],
        "nodes": [{"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"interface": "c"}}}], "output": "s"}));
    assert!(codes(&g).contains(&"out_of_range"));
    let g = from_json(json!({"name": "x", "interface": [{"name": "t", "type": "texture"}],
        "nodes": [{"id": "s", "kind": "standard_surface", "inputs": {"base_color": {"interface": "t"}}}], "output": "s"}));
    assert!(codes(&g).contains(&"type_mismatch"));
    let g = from_json(json!({"name": "", "nodes": [{"id": "s", "kind": "standard_surface"}], "output": "s"}));
    assert!(codes(&g).contains(&"name"));
    let g = from_json(json!({"name": "x", "nodes": [{"id": "s", "kind": "standard_surface"}], "output": "s", "layout": {"s": [1e9, 0]}}));
    assert!(codes(&g).contains(&"layout"));
}

#[test]
fn a_node_id_with_a_dot_is_resolved_or_refused_as_ambiguous() {
    // `a.b` is a node when no node `a` has an output `b`.
    let g = surface_graph(json!([{"id": "a.b", "kind": "constant", "inputs": {"value": 0.5}}]), json!({"base_metalness": {"from": "a.b"}}));
    assert!(validate(&g).is_ok());
    let g = surface_graph(
        json!([{"id": "p", "kind": "texcoord.planar"}, {"id": "p.uv", "kind": "constant", "inputs": {"value": [0, 0]}}, {"id": "s3", "kind": "separate3", "inputs": {"in": {"from": "p.uv"}}}]),
        json!({"base_metalness": {"from": "s3"}}),
    );
    assert!(codes(&g).contains(&"ambiguous_connection"));
}

#[test]
fn budgets_refuse_over_limit_graphs() {
    // Cost: eight octaves of 3D cellular noise is 20 × 8 × 2 × 3 = 960; three of them pass 2000.
    let mut nodes = vec![json!({"id": "p", "kind": "geometry.position"})];
    for i in 0..3 {
        nodes.push(json!({"id": format!("n{i}"), "kind": "pattern.noise", "params": {"kind": "cellular", "dimensions": 3, "octaves": 8}, "inputs": {"position": {"from": "p"}}}));
    }
    nodes.push(json!({"id": "a", "kind": "add", "inputs": {"a": {"from": "n0"}, "b": {"from": "n1"}}}));
    nodes.push(json!({"id": "b", "kind": "add", "inputs": {"a": {"from": "a"}, "b": {"from": "n2"}}}));
    nodes.push(json!({"id": "m", "kind": "min", "inputs": {"a": {"from": "b"}, "b": 1}}));
    assert!(codes(&surface_graph(json!(nodes), json!({"base_metalness": {"from": "m"}}))).contains(&"cost"));
    // Texture count and samples: five distinct images, and three triplanar images (nine samples).
    let sha = |i: u8| format!("{:064x}", i);
    let mut nodes = vec![json!({"id": "p", "kind": "texcoord.planar"})];
    let mut sum = String::from("i0");
    for i in 0..5u8 {
        nodes.push(json!({"id": format!("i{i}"), "kind": "image", "inputs": {"texcoord": {"from": "p"}, "file": {"texture": sha(i + 1)}}}));
        if i > 0 {
            nodes.push(json!({"id": format!("s{i}"), "kind": "add", "inputs": {"a": {"from": sum}, "b": {"from": format!("i{i}")}}}));
            sum = format!("s{i}");
        }
    }
    nodes.push(json!({"id": "c", "kind": "clamp", "inputs": {"in": {"from": sum}}}));
    assert!(codes(&surface_graph(json!(nodes), json!({"base_color": {"from": "c"}}))).contains(&"textures"));
    let mut nodes = vec![json!({"id": "t", "kind": "texcoord.triplanar"})];
    for i in 0..3u8 {
        nodes.push(json!({"id": format!("i{i}"), "kind": "image", "inputs": {"texcoord": {"from": "t"}, "file": {"texture": sha(1)}}}));
    }
    nodes.push(json!({"id": "a", "kind": "add", "inputs": {"a": {"from": "i0"}, "b": {"from": "i1"}}}));
    nodes.push(json!({"id": "b", "kind": "add", "inputs": {"a": {"from": "a"}, "b": {"from": "i2"}}}));
    nodes.push(json!({"id": "c", "kind": "clamp", "inputs": {"in": {"from": "b"}}}));
    assert!(codes(&surface_graph(json!(nodes), json!({"base_color": {"from": "c"}}))).contains(&"samples"));
    // Too many nodes is refused before anything scales with them.
    let nodes: Vec<Value> = (0..=limits::NODES).map(|i| json!({"id": format!("c{i}"), "kind": "constant"})).collect();
    assert_eq!(codes(&surface_graph(json!(nodes), json!({}))), ["node_count"]);
}

#[test]
fn the_validated_copy_drops_layout_for_missing_nodes() {
    let g =
        from_json(json!({"name": "x", "nodes": [{"id": "s", "kind": "standard_surface"}], "output": "s", "layout": {"s": [1, 2], "gone": [3, 4]}}));
    let v = validate(&g).unwrap();
    assert_eq!(v.graph().layout.keys().collect::<Vec<_>>(), ["s"]);
}

#[test]
fn every_later_output_has_a_selection_op() {
    for spec in CATALOGUE {
        for output in spec.outputs.iter().skip(1) {
            let op = format!("{}.{}", spec.kind, output.name);
            assert!(lower::SELECTIONS.iter().any(|s| s.0 == spec.kind && s.1 == output.name && s.2 == op), "{op}");
        }
    }
    assert_eq!(lower::SELECTIONS.len(), CATALOGUE.iter().map(|s| s.outputs.len().saturating_sub(1)).sum::<usize>());
}

/// A small deterministic generator (xorshift), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn number(&mut self, lo: f64, hi: f64) -> f64 {
        match self.below(40) {
            0 => f64::NAN,
            1 => f64::INFINITY,
            2 => -1e300,
            3 => hi + 1.0,
            _ => lo + (hi - lo) * (self.next() % 10_000) as f64 / 10_000.0,
        }
    }
}

/// A random graph: mostly well-typed (so lowering runs), with hostile values, wrong types,
/// unknown names and back edges mixed in.
fn random_graph(rng: &mut Rng) -> MaterialGraph {
    let big = rng.chance(2);
    let count = 1 + rng.below(if big { limits::NODES + 8 } else { 24 });
    let mut nodes: Vec<Value> = Vec::new();
    let mut outputs: Vec<(String, &'static str, PortType)> = Vec::new();
    for i in 0..count {
        let last = i + 1 == count;
        let spec = if last && !rng.chance(5) {
            node_spec(if rng.chance(20) { "surface.mix" } else { "standard_surface" }).unwrap()
        } else {
            &CATALOGUE[rng.below(CATALOGUE.len())]
        };
        let id = if rng.chance(2) { format!("N{i}") } else { format!("n{i}") };
        let mut params = serde_json::Map::new();
        for p in spec.params {
            if rng.chance(50) {
                let v = match p.kind {
                    ParamKind::Choice { options, .. } if !rng.chance(5) => json!(options[rng.below(options.len())]),
                    ParamKind::Int { min, max, .. } if !rng.chance(5) => json!(min + rng.below((max - min + 1) as usize) as i64),
                    _ => json!(rng.next() as i64),
                };
                params.insert(p.name.into(), v);
            }
        }
        let mut inputs = serde_json::Map::new();
        for input in spec.inputs {
            if !rng.chance(60) && input.default != InputDefault::Required {
                continue;
            }
            let compatible: Vec<&(String, &str, PortType)> =
                outputs.iter().filter(|o| o.2 == input.ty || input.ty == PortType::Numeric || o.2 == PortType::Numeric || rng.chance(3)).collect();
            let (lo, hi) = input.range.unwrap_or((-10.0, 10.0));
            let value = if !compatible.is_empty() && rng.chance(55) {
                let (node, output, _) = compatible[rng.below(compatible.len())];
                let from = if rng.chance(50) { format!("{node}.{output}") } else { node.clone() };
                if rng.chance(10) { json!({"from": from, "fallback": rng.number(lo, hi)}) } else { json!({"from": from}) }
            } else if rng.chance(3) {
                json!({"from": format!("n{}", rng.below(count + 2))})
            } else if rng.chance(3) {
                json!({"interface": "nope"})
            } else {
                match input.ty {
                    PortType::Color3 | PortType::Vector3 => json!([rng.number(lo, hi), rng.number(lo, hi), rng.number(lo, hi)]),
                    PortType::Vector2 => json!([rng.number(lo, hi), rng.number(lo, hi)]),
                    PortType::Int => json!(rng.below(100)),
                    PortType::Texture => json!({"texture": format!("{:064x}", rng.next())}),
                    _ => json!(rng.number(lo, hi)),
                }
            };
            // Values that do not serialise as JSON numbers stay out of the text; NaN is tried
            // through the typed path below.
            if !value.is_null() {
                inputs.insert(input.name.into(), value);
            }
        }
        if rng.chance(2) {
            inputs.insert("sheen_weight".into(), json!(1));
        }
        nodes.push(json!({"id": id, "kind": spec.kind, "params": params, "inputs": inputs}));
        for o in spec.outputs {
            outputs.push((id.clone(), o.name, o.ty));
        }
    }
    let output = nodes.last().and_then(|n| n["id"].as_str()).unwrap_or("n0").to_owned();
    let graph = json!({"name": format!("random {}", rng.next() % 1000), "nodes": nodes, "output": output});
    serde_json::from_value(graph)
        .unwrap_or_else(|_| from_json(json!({"name": "x", "nodes": [{"id": "s", "kind": "standard_surface"}], "output": "s"})))
}

/// Everything a consumer may call on a graph, which must never panic, and must agree.
fn exercise(graph: &MaterialGraph) -> bool {
    let Ok(v) = validate(graph) else { return false };
    let ir = lower(&v).unwrap_or_else(|e| panic!("validated but did not lower: {e}"));
    assert!(ir.cost <= limits::COST_UNITS && ir.samples <= limits::TEXTURE_SAMPLES);
    assert!(ir.textures.len() <= limits::TEXTURES_PER_GRAPH && ir.uniform_floats as usize <= limits::UNIFORM_FLOATS);
    for instr in &ir.instrs {
        for arg in &instr.args {
            if let Operand::Value(id) = arg {
                assert!(id.0 < instr.result.0, "SSA order");
            }
        }
    }
    base_response(&v).unwrap_or_else(|e| panic!("validated but no base response: {e}"));
    uniform_words(&ir, &BTreeMap::new()).unwrap();
    true
}

#[test]
fn hostile_graphs_never_panic() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut valid = 0;
    for _ in 0..4000 {
        let mut graph = random_graph(&mut rng);
        // Non-JSON values only a programmatic caller can make.
        if rng.chance(10)
            && let Some(node) = graph.nodes.first_mut()
        {
            let value = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX][rng.below(4)];
            node.inputs.insert("value".into(), graph::InputValue::Constant(Constant::Number(value)));
            graph.layout.insert(node.id.clone(), [value, 0.0]);
        }
        if exercise(&graph) {
            valid += 1;
        }
    }
    assert!(valid > 100, "the generator should reach lowering often, reached it {valid} times");
}

#[test]
fn mutated_fixtures_never_panic() {
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    let names = [
        "black-epoxy-clear-coat",
        "brushed-nickel-silver",
        "electrolytic-sleeve",
        "laser-etched-nickel-silver",
        "pbr-copper",
        "solder-mask-over-copper",
    ];
    let junk = [
        json!(null),
        json!(-1),
        json!(1e308),
        json!("x"),
        json!([]),
        json!([1, 2, 3, 4]),
        json!({"from": "surface"}),
        json!({"interface": "sleeve"}),
        json!({"srgb8": [1, 2, 3]}),
        json!(true),
    ];
    for _ in 0..3000 {
        let mut value = serde_json::to_value(fixture(names[rng.below(names.len())])).unwrap();
        for _ in 0..1 + rng.below(3) {
            // Walk to a random leaf and replace it.
            let mut pointer = String::new();
            let mut at = &value;
            for _ in 0..6 {
                let next = match at {
                    Value::Object(m) if !m.is_empty() => {
                        let k = m.keys().nth(rng.below(m.len())).cloned().unwrap();
                        let next = m.get(&k);
                        pointer.push_str(&format!("/{}", k.replace('~', "~0").replace('/', "~1")));
                        next
                    }
                    Value::Array(a) if !a.is_empty() => {
                        let i = rng.below(a.len());
                        pointer.push_str(&format!("/{i}"));
                        a.get(i)
                    }
                    _ => None,
                };
                match next {
                    Some(n) if rng.chance(80) => at = n,
                    _ => break,
                }
            }
            if let Some(leaf) = value.pointer_mut(&pointer) {
                *leaf = junk[rng.below(junk.len())].clone();
            }
        }
        if let Ok(graph) = serde_json::from_value::<MaterialGraph>(value) {
            exercise(&graph);
        }
    }
}

#[test]
fn a_full_size_graph_validates_and_lowers() {
    // 127 maths nodes in a chain into the surface: the node limit, every one reachable.
    let mut nodes = vec![json!({"id": "c0", "kind": "constant", "inputs": {"value": 0.25}})];
    for i in 1..limits::NODES - 1 {
        nodes.push(json!({"id": format!("c{i}"), "kind": if i % 2 == 0 { "add" } else { "multiply" }, "inputs": {"a": {"from": format!("c{}", i - 1)}, "b": 1}}));
    }
    let last = format!("c{}", limits::NODES - 2);
    let g = surface_graph(json!(nodes), json!({"base_metalness": {"from": last, "fallback": 0.5}}));
    assert_eq!(g.nodes.len(), limits::NODES);
    let start = std::time::Instant::now();
    let ir = ir_of(&g);
    let elapsed = start.elapsed();
    assert_eq!(ir.instrs.len(), limits::NODES - 1);
    assert_eq!(base_response(&validate(&g).unwrap()).unwrap().metallic(), 0.5);
    // A record, not an assertion: the target (§5.4) is ≤ 1 ms optimised.
    eprintln!("128-node graph: validate + lower in {elapsed:?}");
}
