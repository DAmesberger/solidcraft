use std::collections::BTreeSet;

use solidcore_record::Vec3;

use super::catalogue::{InputDefault, ParamKind, PortType};
use super::graph::{Constant, InputValue, ParamValue};
use super::*;
use crate::surface::OpaqueSurface;

/// The worked examples of docs/material-graph.md §3.8 and a `pbr`-equivalent graph. Track A's
/// validation tests and the back ends' golden tests read the same files.
const FIXTURES: &[(&str, &str)] = &[
    ("black-epoxy-clear-coat", include_str!("../../tests/fixtures/material_graph/black-epoxy-clear-coat.json")),
    ("brushed-nickel-silver", include_str!("../../tests/fixtures/material_graph/brushed-nickel-silver.json")),
    ("electrolytic-sleeve", include_str!("../../tests/fixtures/material_graph/electrolytic-sleeve.json")),
    ("laser-etched-nickel-silver", include_str!("../../tests/fixtures/material_graph/laser-etched-nickel-silver.json")),
    ("pbr-copper", include_str!("../../tests/fixtures/material_graph/pbr-copper.json")),
    ("solder-mask-over-copper", include_str!("../../tests/fixtures/material_graph/solder-mask-over-copper.json")),
];

#[test]
fn catalogue_is_consistent() {
    let mut kinds = BTreeSet::new();
    for spec in CATALOGUE {
        assert!(kinds.insert(spec.kind), "{} twice", spec.kind);
        assert!(!spec.outputs.is_empty(), "{} has no output", spec.kind);
        let mut names = BTreeSet::new();
        for input in spec.inputs {
            assert!(names.insert(input.name), "{}.{} twice", spec.kind, input.name);
            if let (Some((lo, hi)), InputDefault::Float(d)) = (input.range, input.default) {
                assert!(lo <= d && d <= hi, "{}.{} default {d} outside [{lo}, {hi}]", spec.kind, input.name);
            }
            if let (Some((lo, hi)), InputDefault::Vec3(d)) = (input.range, input.default) {
                assert!(d.iter().all(|c| lo <= *c && *c <= hi), "{}.{} default outside range", spec.kind, input.name);
            }
            assert!(input.ty != PortType::Surface || spec.kind == "surface.mix", "only surface.mix takes surfaces");
        }
        for param in spec.params {
            match param.kind {
                ParamKind::Choice { options, default } => assert!(options.contains(&default), "{}.{}", spec.kind, param.name),
                ParamKind::Int { min, max, default } => assert!(min <= default && default <= max, "{}.{}", spec.kind, param.name),
            }
        }
    }
    let surface = node_spec("standard_surface").map(|s| s.inputs.len());
    assert_eq!(surface, Some(22));
    assert_eq!(CATALOGUE_VERSION, 1);
}

/// The contract-level check the fixtures must pass already: every kind, input, param and
/// connection names something the catalogue has. Full validation (types, cycles, ranges,
/// budgets) is track A's.
#[test]
fn worked_examples_parse_and_name_catalogue_ports() {
    for (name, text) in FIXTURES {
        let graph: MaterialGraph = serde_json::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ids: BTreeSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids.len(), graph.nodes.len(), "{name}: duplicate ids");
        assert!(ids.contains(graph.output.as_str()), "{name}: output");
        for node in &graph.nodes {
            let spec = node_spec(&node.kind).unwrap_or_else(|| panic!("{name}: unknown kind {}", node.kind));
            for (param, value) in &node.params {
                let p = spec.param(param).unwrap_or_else(|| panic!("{name}: {}.{param}", node.kind));
                match (p.kind, value) {
                    (ParamKind::Choice { options, .. }, ParamValue::Text(t)) => assert!(options.contains(&t.as_str()), "{name}: {param}={t}"),
                    (ParamKind::Int { min, max, .. }, ParamValue::Int(i)) => assert!(min <= *i && *i <= max, "{name}: {param}={i}"),
                    _ => panic!("{name}: {}.{param} has the wrong kind of value", node.kind),
                }
            }
            for (input, value) in &node.inputs {
                assert!(spec.input(input).is_some(), "{name}: {}.{input}", node.kind);
                if let InputValue::Connection(c) = value {
                    let (from, output) = c.source();
                    let source = graph.nodes.iter().find(|n| n.id == from).unwrap_or_else(|| panic!("{name}: from {from}"));
                    let source_spec = node_spec(&source.kind).unwrap_or_else(|| panic!("{name}: {}", source.kind));
                    if let Some(output) = output {
                        assert!(source_spec.output(output).is_some(), "{name}: {from}.{output}");
                    }
                }
                if let InputValue::Interface(r) = value {
                    assert!(graph.interface.iter().any(|i| i.name == r.interface), "{name}: interface {}", r.interface);
                }
            }
        }
        let again: MaterialGraph = serde_json::from_value(serde_json::to_value(&graph).unwrap()).unwrap();
        assert_eq!(again, graph, "{name}: round trip");
    }
}

#[test]
fn an_editor_layout_is_optional_and_round_trips() {
    let text = r#"{"name":"x","nodes":[{"id":"s","kind":"standard_surface"}],"output":"s","layout":{"s":[120.0,-40.5]}}"#;
    let graph: MaterialGraph = serde_json::from_str(text).unwrap();
    assert_eq!(graph.layout.get("s"), Some(&[120.0, -40.5]));
    let saved = serde_json::to_value(&graph).unwrap();
    assert_eq!(saved["layout"]["s"][0], 120.0);
    let plain: MaterialGraph = serde_json::from_str(FIXTURES[4].1).unwrap();
    assert!(serde_json::to_value(&plain).unwrap().get("layout").is_none(), "no layout, no key");
}

#[test]
fn saved_form_refuses_unknown_fields() {
    let text = r#"{"name":"x","nodes":[{"id":"s","kind":"standard_surface","shader":"fn f(){}"}],"output":"s"}"#;
    assert!(serde_json::from_str::<MaterialGraph>(text).is_err());
    let text = r#"{"name":"x","nodes":[],"output":"s","wgsl":"…"}"#;
    assert!(serde_json::from_str::<MaterialGraph>(text).is_err());
}

#[test]
fn constants_take_every_documented_form() {
    let c: Constant = serde_json::from_str(r#"{"srgb8":[204,122,80]}"#).unwrap();
    assert_eq!(c, Constant::Srgb8(graph::Srgb8 { srgb8: [204, 122, 80] }));
    assert!(matches!(serde_json::from_str::<Constant>("0.5").unwrap(), Constant::Number(_)));
    assert!(matches!(serde_json::from_str::<Constant>("[0.1,0.2,0.3]").unwrap(), Constant::Vector(_)));
    assert!(matches!(serde_json::from_str::<Constant>(r#"{"texture":"ab"}"#).unwrap(), Constant::Texture(_)));
    let c: graph::Connection = serde_json::from_str(r#"{"from":"lines.tangent"}"#).unwrap();
    assert_eq!(c.source(), ("lines", Some("tangent")));
}

/// Track A's steps are built: the fixtures validate (its own tests are in `validate_tests.rs`).
#[test]
fn the_fixtures_validate() {
    for (name, text) in FIXTURES {
        let graph: MaterialGraph = serde_json::from_str(text).unwrap();
        assert!(validate(&graph).is_ok(), "{name}");
    }
}

#[test]
fn an_opaque_surface_is_a_sample_with_default_extensions() {
    let opaque = OpaqueSurface::new(Vec3::new(0.6, 0.2, 0.1), 1.0, 0.25, 1.5).unwrap();
    let sample = SurfaceSample::from(opaque);
    assert_eq!(sample.as_opaque(), Some(opaque));
    let tinted = SurfaceSample { coat_color: Vec3::new(0.1, 0.5, 0.1), ..sample };
    assert_eq!(tinted.as_opaque(), None);
}
