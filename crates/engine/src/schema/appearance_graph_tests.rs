use serde_json::json;

use crate::Session;
use crate::schema::ParamCheck;

fn strict() -> Session {
    Session { param_check: ParamCheck::Strict, ..Session::default() }
}

const UNBUILT: &[(&str, &str)] = &[
    ("appearance.graph.create", r#"{"name":"Brushed"}"#),
    ("appearance.graph.set", r#"{"id":"a","graph":{"name":"x","nodes":[],"output":"s"}}"#),
    ("appearance.graph.edit", r#"{"id":"a","ops":[{"op":"remove_node","node":"n"}]}"#),
    ("appearance.graph.get", r#"{"id":"a"}"#),
    ("appearance.graph.list", "{}"),
    ("appearance.graph.duplicate", r#"{"id":"a"}"#),
    ("appearance.graph.delete", r#"{"id":"a"}"#),
    ("appearance.graph.update_from_library", r#"{"id":"a","library":"lib.json"}"#),
    ("appearance.texture.add", r#"{"path":"logo.png"}"#),
    ("appearance.texture.list", "{}"),
];

#[test]
fn the_catalogue_is_served() {
    let mut s = strict();
    let out = s.execute("appearance.graph.nodes", &json!({})).unwrap();
    assert_eq!(out["version"], 1);
    let kinds: Vec<&str> = out["nodes"].as_array().unwrap().iter().filter_map(|n| n["kind"].as_str()).collect();
    assert!(kinds.contains(&"standard_surface") && kinds.contains(&"pattern.brushed") && kinds.contains(&"image"));
    assert!(s.execute("appearance.graph.nodes", &json!({"extra": 1})).is_err());
}

/// Unbuilt commands check their parameters, then refuse without changing the design or its
/// history.
#[test]
fn unbuilt_commands_refuse_and_change_nothing() {
    let mut s = strict();
    let before = s.doc.to_json();
    for (id, params) in UNBUILT {
        let params: serde_json::Value = serde_json::from_str(params).unwrap();
        let error = s.execute(id, &params).unwrap_err().to_string();
        assert!(error.contains("not built yet"), "{id}: {error}");
    }
    assert_eq!(s.doc.to_json(), before);
    assert!(s.execute("appearance.graph.edit", &json!({"id": "a", "ops": [{"op": "paint"}]})).is_err());
    // Validation is built (track A): an empty graph is reported invalid, not refused.
    let report = s.execute("appearance.graph.validate", &json!({"graph": {"name": "x", "nodes": [], "output": "s"}})).unwrap();
    assert_eq!((report["ok"].clone(), report["errors"][0]["code"].clone()), (json!(false), json!("node_count")), "{report}");
    assert!(s.execute("appearance.graph.validate", &json!({"graph": {"name": "x", "nodes": [], "output": "s", "wgsl": "fn f() {}"}})).is_err());
}
