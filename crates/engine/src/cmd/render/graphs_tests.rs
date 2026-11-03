use serde_json::json;

use super::*;
use crate::Session;

/// A real 2×1 RGBA PNG (original test data).
const PNG_2X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0, 244, 34, 127, 138, 0, 0, 0, 14, 73, 68, 65,
    84, 120, 156, 99, 56, 161, 161, 241, 31, 132, 1, 18, 51, 4, 47, 169, 127, 204, 104, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// Two boxes; `Label` shows a graph with a colour and a texture interface, `Plain` a scalar look.
fn design() -> (Session, String) {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"body_name": "Label", "length": 10, "width": 10, "height": 2})).unwrap();
    s.execute("solid.box", &json!({"body_name": "Plain", "length": 4, "width": 4, "height": 4})).unwrap();
    let key = s.execute("appearance.texture.add", &json!({"data_base64": solidcraft_doc::canvas::base64_encode(PNG_2X1)})).unwrap()["texture"]
        .as_str()
        .unwrap()
        .to_string();
    let graph = json!({
        "name": "Label",
        "interface": [{"name": "tint", "type": "color3", "default": [0.2, 0.4, 0.6]}, {"name": "mark", "type": "texture"}],
        "nodes": [
            {"id": "uv", "kind": "texcoord.planar"},
            {"id": "print", "kind": "image", "inputs": {"texcoord": {"from": "uv"}, "file": {"interface": "mark"}}},
            {"id": "surface", "kind": "standard_surface", "inputs": {
                "base_color": {"interface": "tint"},
                "specular_roughness": {"from": "print.value", "fallback": 0.4}}}],
        "output": "surface"
    });
    s.execute("appearance.graph.create", &json!({"id": "label", "name": "Label", "graph": graph})).unwrap();
    s.execute("appearance.assign", &json!({"bodies": ["Label"], "graph": "label", "values": {"mark": {"texture": key}}})).unwrap();
    s.execute("appearance.assign", &json!({"bodies": ["Plain"], "color": [9, 9, 9]})).unwrap();
    (s, key)
}

fn ordinal(scene: &ProductScene, name: &str) -> usize {
    scene.meshes.iter().position(|m| m.name == name).unwrap()
}

fn overrides(v: serde_json::Value) -> Vec<Override> {
    serde_json::from_value(v).unwrap()
}

#[test]
fn graph_looks_are_captured_with_their_values_and_textures() {
    let (s, key) = design();
    let scene = crate::product_scene::extract(&s).unwrap();
    let label = ordinal(&scene, "Label");
    // The scene keeps the base response as the body's surface.
    let surface = scene.meshes[label].surface.unwrap();
    assert!((surface.roughness() - 0.4).abs() < 1e-12, "the fallback, not the texture");
    let captured = capture(&s, &scene, &[], &[], "render.start").unwrap().unwrap();
    assert_eq!(captured.materials.len(), 1);
    assert_eq!(
        captured.bindings,
        vec![CapturedBinding { body_index: label, body_name: "Label".into(), face: None, material: 0, object_to_world: None }]
    );
    let m = &captured.materials[0];
    assert_eq!(m.values.len(), 1);
    let (bytes, srgb) = m.textures[0].as_ref().unwrap();
    assert_eq!((bytes.as_slice(), *srgb), (PNG_2X1, true));
    assert_eq!(solidcraft_doc::appearance_graph::texture_key(bytes), key);
    // A design without graph looks captures nothing.
    let plain = Session::default();
    assert!(capture(&plain, &crate::product_scene::extract(&plain).unwrap(), &[], &[], "render.start").unwrap().is_none());
}

#[test]
fn runtime_overrides_merge_over_look_values_and_never_touch_the_design() {
    let (s, key) = design();
    let before = s.doc.clone();
    let scene = crate::product_scene::extract(&s).unwrap();
    let label = ordinal(&scene, "Label");
    // A per-body image given with the job, and a graph-wide tint.
    let mut other = PNG_2X1.to_vec();
    other.extend_from_slice(b"another label");
    let runtime_key = solidcraft_doc::appearance_graph::texture_key(&other);
    let encoded = vec![solidcraft_doc::canvas::base64_encode(&other)];
    let given = overrides(json!([
        {"graph": "label", "values": {"tint": [0.9, 0.1, 0.1]}},
        {"body_index": label, "body_name": "Label", "values": {"mark": {"texture": runtime_key}}}
    ]));
    let captured = capture(&s, &scene, &given, &encoded, "render.start").unwrap().unwrap();
    let m = &captured.materials[0];
    assert_eq!(serde_json::to_value(&m.values).unwrap(), json!({"mark": {"texture": runtime_key}, "tint": [0.9, 0.1, 0.1]}));
    assert_eq!(m.textures[0].as_ref().unwrap().0.as_slice(), other.as_slice());
    assert!(std::sync::Arc::ptr_eq(&before, &s.doc));
    assert!(!s.doc.appearances.tables.textures.contains_key(&runtime_key));
    // Refused: unknown inputs, wrong types, missing textures, a wrong body name, unused images.
    let refused = [
        (json!([{"graph": "label", "values": {"nope": 1}}]), vec![]),
        (json!([{"graph": "label", "values": {"tint": {"texture": key}}}]), vec![]),
        (json!([{"graph": "label", "values": {"mark": {"texture": "c".repeat(64)}}}]), vec![]),
        (json!([{"graph": "label", "values": {"tint": [5.0, 0.0, 0.0]}}]), vec![]),
        (json!([{"graph": "missing", "values": {}}]), vec![]),
        (json!([{"body_index": label, "body_name": "Plain", "values": {}}]), vec![]),
        (json!([{"graph": "label", "body_index": label, "body_name": "Label", "values": {}}]), vec![]),
        (json!([]), encoded.clone()),
        (json!([]), vec!["not base64!".to_string()]),
    ];
    for (given, encoded) in refused {
        assert!(capture(&s, &scene, &overrides(given.clone()), &encoded, "render.start").is_err(), "{given}");
    }
}

#[test]
fn placed_copies_bind_with_their_placement() {
    let mut s = Session::default();
    let part = s.execute("component.create", &json!({"name": "Part", "grounded": false})).unwrap();
    s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10, "body_name": "Block"})).unwrap();
    s.execute("appearance.graph.create", &json!({"id": "plain", "name": "Plain"})).unwrap();
    s.execute("appearance.assign", &json!({"bodies": ["Block"], "graph": "plain"})).unwrap();
    s.execute("component.activate", &json!({"component": 0})).unwrap();
    s.execute("occurrence.move", &json!({"occurrence": part["occurrence"], "translate": [10, 0, 0]})).unwrap();
    let scene = crate::product_scene::extract(&s).unwrap();
    let captured = capture(&s, &scene, &[], &[], "render.start").unwrap().unwrap();
    let placement = captured.bindings[0].object_to_world.unwrap();
    assert_eq!(placement[0], [1.0, 0.0, 0.0, 10.0]);
}

/// What track PT's `GraphMaterial::new` and `GraphMaterials::new` check, so the adapter that
/// hands a capture to the path tracer cannot fail on a capture this module makes.
fn assert_path_back_end_shapes(c: &CapturedGraphs) {
    use solidcraft_render::material_graph::limits;
    assert!(!c.materials.is_empty() && c.materials.len() <= MAX_MATERIALS);
    assert!(!c.bindings.is_empty() && c.bindings.len() <= 65_536);
    for m in &c.materials {
        let words = mg::uniform_words(&m.ir, &m.values).expect("the values make uniform words");
        assert!(words.len() <= limits::UNIFORM_FLOATS && m.ir.textures.len() <= limits::TEXTURES_PER_GRAPH);
        assert_eq!(m.textures.len(), m.ir.textures.len(), "one texture entry per image binding");
        for (t, b) in m.textures.iter().zip(&m.ir.textures) {
            if let Some((bytes, srgb)) = t {
                assert_eq!(*srgb, b.srgb, "the colour space of its image node");
                assert!(solidcraft_doc::canvas::image_info(bytes).is_some(), "an encoded PNG or JPEG");
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for b in &c.bindings {
        assert!(b.body_index < 65_536 && b.material < c.materials.len());
        assert!(!b.body_name.is_empty() && b.body_name.len() <= 1024 && !b.body_name.chars().any(char::is_control));
        assert!(seen.insert((b.body_index, b.face)), "one binding per body or face");
        if let Some(m) = &b.object_to_world {
            assert!(m.iter().flatten().all(|x| x.is_finite() && x.abs() <= 1e7));
        }
    }
}

#[test]
fn captures_meet_the_path_back_ends_constructor_rules() {
    let (mut s, _) = design();
    // A face with its own graph look beside its body's.
    s.execute("appearance.graph.create", &json!({"id": "plain", "name": "Plain"})).unwrap();
    s.execute("appearance.assign", &json!({"faces": [[5, 5, 2]], "body": "Label", "graph": "plain"})).unwrap();
    let scene = crate::product_scene::extract(&s).unwrap();
    let captured = capture(&s, &scene, &[], &[], "render.start").unwrap().unwrap();
    assert_eq!(captured.materials.len(), 2);
    assert!(captured.bindings.iter().any(|b| b.face.is_some()));
    assert_path_back_end_shapes(&captured);
    let label = ordinal(&scene, "Label");
    let given = overrides(json!([{"body_index": label, "body_name": "Label", "values": {"tint": [0.1, 0.2, 0.3]}}]));
    assert_path_back_end_shapes(&capture(&s, &scene, &given, &[], "render.start").unwrap().unwrap());
}
