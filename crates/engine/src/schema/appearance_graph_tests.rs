use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::schema::ParamCheck;

const COPPER: &str = include_str!("../../../render/tests/fixtures/material_graph/pbr-copper.json");
const ETCHED: &str = include_str!("../../../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json");
const BRUSHED: &str = include_str!("../../../render/tests/fixtures/material_graph/brushed-nickel-silver.json");

/// A real 2×1 RGBA PNG (original test data, as in the doc crate's tests).
const PNG_2X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0, 244, 34, 127, 138, 0, 0, 0, 14, 73, 68, 65,
    84, 120, 156, 99, 56, 161, 161, 241, 31, 132, 1, 18, 51, 4, 47, 169, 127, 204, 104, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn strict() -> Session {
    Session { param_check: ParamCheck::Strict, ..Session::default() }
}

/// A strict session with one box body, `Can`.
fn with_body() -> Session {
    let mut s = strict();
    s.execute("solid.box", &json!({"body_name": "Can", "length": 10, "width": 20, "height": 5})).unwrap();
    s
}

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn png_base64() -> String {
    solidcraft_doc::canvas::base64_encode(PNG_2X1)
}

/// The design, undo depth and revision: a refused command changes none of them.
fn state(s: &Session) -> (Arc<solidcraft_doc::Document>, usize, u64) {
    (s.doc.clone(), s.undo.len(), s.revision)
}

fn unchanged(s: &Session, before: &(Arc<solidcraft_doc::Document>, usize, u64), what: &str) {
    assert!(Arc::ptr_eq(&before.0, &s.doc), "{what}: the design changed");
    assert_eq!((before.1, before.2), (s.undo.len(), s.revision), "{what}: the history changed");
}

#[test]
fn the_catalogue_is_served() {
    let mut s = strict();
    let out = s.execute("appearance.graph.nodes", &json!({})).unwrap();
    assert_eq!(out["version"], 1);
    let kinds: Vec<&str> = out["nodes"].as_array().unwrap().iter().filter_map(|n| n["kind"].as_str()).collect();
    assert!(kinds.contains(&"standard_surface") && kinds.contains(&"pattern.brushed") && kinds.contains(&"image"));
    assert!(s.execute("appearance.graph.nodes", &json!({"extra": 1})).is_err());
}

#[test]
fn validation_reports_without_touching_the_design() {
    let mut s = strict();
    let report = s.execute("appearance.graph.validate", &json!({"graph": {"name": "x", "nodes": [], "output": "s"}})).unwrap();
    assert_eq!((report["ok"].clone(), report["errors"][0]["code"].clone()), (json!(false), json!("node_count")), "{report}");
    assert!(s.execute("appearance.graph.validate", &json!({"graph": {"name": "x", "nodes": [], "output": "s", "wgsl": "fn f() {}"}})).is_err());
    let ok = s.execute("appearance.graph.validate", &json!({"graph": fixture(BRUSHED)})).unwrap();
    assert_eq!(ok["ok"], true, "{ok}");
    assert!(ok["cost"].as_u64().unwrap() > 0 && !ok["shape"].as_str().unwrap().is_empty());
    assert!(s.doc.appearances.tables.graphs.is_empty());
}

/// Create, list, get, set, edit, duplicate and delete, each one undo step.
#[test]
fn graphs_live_in_the_design_and_each_change_is_one_undo_step() {
    let mut s = strict();
    let created = s.execute("appearance.graph.create", &json!({"name": "Brushed Nickel/Silver"})).unwrap();
    assert_eq!(created["id"], "brushed-nickel-silver");
    assert!(created["cost"].is_u64());
    // `from: "standard"` is the default graph; an id is taken once.
    let standard = s.execute("appearance.graph.create", &json!({"name": "Brushed Nickel/Silver", "from": "standard"})).unwrap();
    assert_eq!(standard["id"], "brushed-nickel-silver-2");
    assert!(s.execute("appearance.graph.create", &json!({"id": "brushed-nickel-silver", "name": "Again"})).is_err());
    s.execute("appearance.graph.create", &json!({"id": "copper", "name": "Copper", "graph": fixture(COPPER)})).unwrap();
    assert_eq!(s.doc.appearances.tables.graphs.len(), 3);
    assert_eq!(s.undo.len(), 3);

    let list = s.execute("appearance.graph.list", &json!({})).unwrap();
    let ids: Vec<&str> = list["graphs"].as_array().unwrap().iter().map(|g| g["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["brushed-nickel-silver", "brushed-nickel-silver-2", "copper"]);
    let got = s.execute("appearance.graph.get", &json!({"id": "copper"})).unwrap();
    assert_eq!(got["graph"]["name"], "Copper", "create names the graph");
    assert_eq!(got["uses"], 0);

    s.execute("appearance.graph.set", &json!({"id": "brushed-nickel-silver", "graph": fixture(BRUSHED)})).unwrap();
    assert_eq!(s.doc.appearances.tables.graphs.values().next().unwrap().nodes.len(), fixture(BRUSHED)["nodes"].as_array().unwrap().len());
    let edited = s
        .execute(
            "appearance.graph.edit",
            &json!({"id": "copper", "ops": [
                {"op": "set_input", "node": "surface", "port": "coat_weight", "value": 1},
                {"op": "set_input", "node": "surface", "port": "coat_roughness", "value": 0.1}]}),
        )
        .unwrap();
    assert_eq!(edited["id"], "copper");
    let copy = s.execute("appearance.graph.duplicate", &json!({"id": "copper"})).unwrap();
    assert_eq!(copy["id"], "copper-copy");
    assert_eq!(s.doc.appearances.tables.graphs[&gid("copper-copy")].name, "Copper copy");
    s.execute("appearance.graph.delete", &json!({"id": "copper-copy"})).unwrap();
    assert_eq!(s.undo.len(), 7);

    // Undo walks back one command at a time.
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.doc.appearances.tables.graphs.contains_key(&gid("copper-copy")));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!s.doc.appearances.tables.graphs.contains_key(&gid("copper-copy")));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!s.doc.appearances.tables.graphs[&gid("copper")].nodes[0].inputs.contains_key("coat_weight"), "both edits undo together");
    for _ in 0..4 {
        s.execute("edit.undo", &json!({})).unwrap();
    }
    assert!(s.doc.appearances.tables.graphs.is_empty());
    // Reading changes nothing and adds no step.
    let before = state(&s);
    s.execute("appearance.graph.list", &json!({})).unwrap();
    unchanged(&s, &before, "list");
}

fn gid(id: &str) -> solidcraft_doc::appearance_graph::GraphId {
    solidcraft_doc::appearance_graph::GraphId::new(id).unwrap()
}

/// A refused command leaves the design and its history exactly as they were.
#[test]
fn refused_changes_leave_everything_unchanged() {
    let mut s = with_body();
    s.execute("appearance.graph.create", &json!({"id": "copper", "name": "Copper", "graph": fixture(COPPER)})).unwrap();
    s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "copper"})).unwrap();
    let before = state(&s);
    let refused: Vec<(&str, Value)> = vec![
        // The last edit is out of range: none of them applies.
        (
            "appearance.graph.edit",
            json!({"id": "copper", "ops": [
                {"op": "set_input", "node": "surface", "port": "coat_weight", "value": 1},
                {"op": "set_input", "node": "surface", "port": "specular_roughness", "value": 7}]}),
        ),
        ("appearance.graph.edit", json!({"id": "copper", "ops": [{"op": "set_input", "node": "nope", "port": "base_color", "value": 1}]})),
        ("appearance.graph.edit", json!({"id": "copper", "ops": [{"op": "add_node", "node": "n", "kind": "shader.custom"}]})),
        ("appearance.graph.edit", json!({"id": "copper", "ops": [{"op": "remove_node", "node": "surface"}]})),
        ("appearance.graph.edit", json!({"id": "copper", "ops": [{"op": "disconnect", "node": "surface", "port": "base_color"}]})),
        ("appearance.graph.edit", json!({"id": "copper", "ops": [{"op": "connect", "from": "ghost", "node": "surface", "port": "base_color"}]})),
        ("appearance.graph.edit", json!({"id": "copper", "ops": []})),
        ("appearance.graph.edit", json!({"id": "missing", "ops": [{"op": "remove_node", "node": "n"}]})),
        ("appearance.graph.set", json!({"id": "copper", "graph": {"name": "x", "nodes": [], "output": "s"}})),
        ("appearance.graph.set", json!({"id": "missing", "graph": fixture(COPPER)})),
        // A texture the design does not hold.
        (
            "appearance.graph.edit",
            json!({"id": "copper", "ops": [{"op": "set_interface", "input": {"name": "mark", "type": "texture", "default": {"texture": "a".repeat(64)}}}]}),
        ),
        ("appearance.graph.create", json!({"name": ""})),
        ("appearance.graph.create", json!({"name": "x", "id": "Bad Id"})),
        ("appearance.graph.create", json!({"name": "x", "graph": fixture(COPPER), "from": "standard"})),
        ("appearance.graph.create", json!({"name": "x", "from": "chrome"})),
        // Used by a look, and no replacement named.
        ("appearance.graph.delete", json!({"id": "copper"})),
        ("appearance.graph.delete", json!({"id": "copper", "replace_with": "copper"})),
        ("appearance.graph.duplicate", json!({"id": "copper", "new_id": "copper"})),
        ("appearance.texture.add", json!({"data_base64": "!!!"})),
        ("appearance.texture.add", json!({"data_base64": solidcraft_doc::canvas::base64_encode(b"GIF89a not a png")})),
        ("appearance.texture.add", json!({"data_base64": png_base64(), "path": "/tmp/x.png"})),
        ("appearance.texture.add", json!({})),
        ("appearance.assign", json!({"bodies": ["Can"], "graph": "missing"})),
        ("appearance.assign", json!({"bodies": ["Can"], "graph": "copper", "pbr": {"metallic": 1, "roughness": 0.3, "dielectric_ior": 1.5}})),
        ("appearance.assign", json!({"bodies": ["Can"], "graph": "copper", "values": {"undeclared": 1}})),
        ("appearance.assign", json!({"bodies": ["Can"], "values": {"a": 1}, "color": [1, 2, 3]})),
    ];
    for (id, params) in refused {
        assert!(s.execute(id, &params).is_err(), "{id} {params} was accepted");
        unchanged(&s, &before, &format!("{id} {params}"));
    }
}

#[test]
fn assigned_graph_looks_take_the_graphs_colour_and_follow_its_edits() {
    let mut s = with_body();
    s.execute("appearance.graph.create", &json!({"id": "copper", "name": "Copper", "graph": fixture(COPPER)})).unwrap();
    let assigned = s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "copper", "color": [1, 1, 1]})).unwrap();
    let look = &s.doc.appearances.bodies["Can"];
    // The fixture's base colour is `{"srgb8": [204, 122, 80]}`: exactly the look's colour.
    assert_eq!((look.color, look.name.as_str(), look.opacity), ([204, 122, 80], "copper", 1.0));
    assert_eq!(assigned["appearance"]["graph"], "copper");
    assert_eq!(assigned["appearance"]["color"], "#cc7a50");
    assert_eq!(s.execute("appearance.graph.get", &json!({"id": "copper"})).unwrap()["uses"], 1);
    // Editing the graph's colour recolours the look, in the same step.
    let steps = s.undo.len();
    s.execute(
        "appearance.graph.edit",
        &json!({"id": "copper", "ops": [{"op": "set_input", "node": "surface", "port": "base_color", "value": {"srgb8": [10, 20, 30]}}]}),
    )
    .unwrap();
    assert_eq!(s.doc.appearances.bodies["Can"].color, [10, 20, 30]);
    assert_eq!(s.undo.len(), steps + 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.doc.appearances.bodies["Can"].color, [204, 122, 80]);
    // A driven input shows its fallback in the standard view.
    s.execute(
        "appearance.graph.edit",
        &json!({"id": "copper", "ops": [
            {"op": "add_node", "node": "uv", "kind": "texcoord.triplanar"},
            {"op": "add_node", "node": "grain", "kind": "pattern.noise"},
            {"op": "add_node", "node": "tint", "kind": "convert.float_to_color3"},
            {"op": "connect", "from": "uv", "node": "grain", "port": "texcoord"},
            {"op": "connect", "from": "grain.value", "node": "tint", "port": "in"},
            {"op": "connect", "from": "tint", "node": "surface", "port": "base_color", "fallback": {"srgb8": [90, 60, 30]}}]}),
    )
    .unwrap_or_else(|e| panic!("{e}: {}", s.execute("appearance.graph.get", &json!({"id": "copper"})).unwrap()));
    assert_eq!(s.doc.appearances.bodies["Can"].color, [90, 60, 30]);
    let base = &s.doc.appearances.tables.graphs[&gid("copper")].nodes[0].inputs["base_color"];
    assert_eq!(serde_json::to_value(base).unwrap(), json!({"from": "tint", "fallback": {"srgb8": [90, 60, 30]}}));
    // Removing the pattern returns the input to its default.
    let remove = |n: &str| json!({"op": "remove_node", "node": n});
    s.execute("appearance.graph.edit", &json!({"id": "copper", "ops": [remove("tint"), remove("grain"), remove("uv")]})).unwrap();
    assert!(!s.doc.appearances.tables.graphs[&gid("copper")].nodes[0].inputs.contains_key("base_color"));
    // Copy and paste keep the graph.
    s.execute("appearance.copy", &json!({"target": {"body": "Can"}})).unwrap();
    s.execute("appearance.assign", &json!({"bodies": ["Can"], "color": [5, 5, 5]})).unwrap();
    s.execute("appearance.paste", &json!({"bodies": ["Can"]})).unwrap();
    assert_eq!(s.doc.appearances.bodies["Can"].graph, Some(gid("copper")));
    // Deleting with a replacement moves the look.
    s.execute("appearance.graph.create", &json!({"id": "plain", "name": "Plain"})).unwrap();
    let deleted = s.execute("appearance.graph.delete", &json!({"id": "copper", "replace_with": "plain"})).unwrap();
    assert_eq!(deleted["replaced"], 1);
    assert_eq!(s.doc.appearances.bodies["Can"].graph, Some(gid("plain")));
    // The saved design reads back.
    let text = s.doc.to_json();
    solidcraft_doc::Document::from_json(&text).unwrap();
}

#[test]
fn look_values_set_the_interface_and_are_checked() {
    let mut s = with_body();
    let graph = json!({
        "name": "Anodised",
        "interface": [{"name": "tint", "type": "color3", "default": {"srgb8": [40, 90, 160]}}, {"name": "mark", "type": "texture"}],
        "nodes": [{"id": "surface", "kind": "standard_surface", "inputs": {"base_color": {"interface": "tint"}, "base_metalness": 1}}],
        "output": "surface"
    });
    s.execute("appearance.graph.create", &json!({"id": "anodised", "name": "Anodised", "graph": graph})).unwrap();
    s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "anodised"})).unwrap();
    assert_eq!(s.doc.appearances.bodies["Can"].color, [40, 90, 160], "the interface default");
    s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "anodised", "values": {"tint": {"srgb8": [200, 30, 30]}}, "name": "Red"}))
        .unwrap();
    let look = &s.doc.appearances.bodies["Can"];
    assert_eq!((look.color, look.name.as_str()), ([200, 30, 30], "Red"));
    let before = state(&s);
    for values in [json!({"tint": 0.5, "x": 1}), json!({"tint": {"texture": "b".repeat(64)}}), json!({"mark": 0.5}), json!({"tint": [2.0, 0.0]})] {
        assert!(s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "anodised", "values": values})).is_err(), "{values}");
        unchanged(&s, &before, &values.to_string());
    }
    // A texture value names a held texture.
    let added = s.execute("appearance.texture.add", &json!({"data_base64": png_base64(), "name": "mark.png"})).unwrap();
    let key = added["texture"].as_str().unwrap().to_string();
    s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "anodised", "values": {"mark": {"texture": key}}})).unwrap();
    let list = s.execute("appearance.texture.list", &json!({})).unwrap();
    assert_eq!(list["textures"][0]["uses"], 1);
    // Removing the interface input a look sets is refused.
    let before = state(&s);
    let bare = json!({"name": "Anodised", "nodes": [{"id": "surface", "kind": "standard_surface"}], "output": "surface"});
    assert!(s.execute("appearance.graph.set", &json!({"id": "anodised", "graph": bare})).is_err());
    unchanged(&s, &before, "set dropping a used interface input");
    // A see-through graph needs look opacity 1.
    let glass =
        json!({"name": "Glass", "nodes": [{"id": "surface", "kind": "standard_surface", "inputs": {"transmission_weight": 1}}], "output": "surface"});
    s.execute("appearance.graph.create", &json!({"id": "glass", "name": "Glass", "graph": glass})).unwrap();
    assert!(s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "glass", "opacity": 0.5})).is_err());
    s.execute("appearance.assign", &json!({"bodies": ["Can"], "graph": "glass"})).unwrap();
}

#[test]
fn textures_are_added_once_listed_and_read_from_files() {
    let mut s = strict();
    let added = s.execute("appearance.texture.add", &json!({"data_base64": png_base64()})).unwrap();
    assert_eq!((added["width"].clone(), added["height"].clone()), (json!(2), json!(1)));
    let key = added["texture"].as_str().unwrap().to_string();
    assert_eq!(key, solidcraft_doc::appearance_graph::texture_key(PNG_2X1));
    let before = state(&s);
    // The same bytes again: no change, no step.
    let again = s.execute("appearance.texture.add", &json!({"data_base64": png_base64(), "name": "other.png"})).unwrap();
    assert_eq!(again["texture"], key.as_str());
    unchanged(&s, &before, "adding the same texture again");
    let listed = s.execute("appearance.texture.list", &json!({})).unwrap();
    assert_eq!(listed["textures"][0], json!({"texture": key, "name": "texture", "width": 2, "height": 1, "bytes": PNG_2X1.len(), "uses": 0}));
    // From a file, named after it.
    let dir = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-graph-texture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mark.png");
    let mut other = PNG_2X1.to_vec();
    other.extend_from_slice(b"trailing bytes make another key");
    std::fs::write(&path, &other).unwrap();
    let from_file = s.execute("appearance.texture.add", &json!({"path": path.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let key2 = from_file["texture"].as_str().unwrap();
    assert_eq!(s.doc.appearances.tables.textures[key2].name, "mark.png");
    // A graph can now name it; a texture nothing names is dropped on save.
    let graph = json!({"name": "Label", "nodes": [
        {"id": "uv", "kind": "texcoord.planar"},
        {"id": "label", "kind": "image", "inputs": {"texcoord": {"from": "uv"}, "file": {"texture": key2}}},
        {"id": "surface", "kind": "standard_surface", "inputs": {"base_color": {"from": "label.color", "fallback": [0.5, 0.5, 0.5]}}}],
        "output": "surface"});
    s.execute("appearance.graph.create", &json!({"id": "label", "name": "Label", "graph": graph})).unwrap();
    let saved = solidcraft_doc::Document::from_json(&s.doc.without_unreferenced_textures().to_json()).unwrap();
    assert_eq!(saved.appearances.tables.textures.keys().collect::<Vec<_>>(), vec![key2]);
}

#[test]
fn converting_a_look_keeps_its_picture_and_rewrites_its_overrides() {
    let mut s = with_body();
    s.execute("solid.box", &json!({"body_name": "Lid", "length": 4, "width": 4, "height": 1})).unwrap();
    let pbr = json!({"metallic": 1.0, "roughness": 0.25, "dielectric_ior": 1.5});
    s.execute("appearance.assign", &json!({"bodies": ["Can", "Lid"], "name": "Copper", "color": [204, 122, 80], "pbr": pbr})).unwrap();
    let look = json!({"name": "Copper", "color": "#cc7a50", "opacity": 1.0, "pbr": pbr});
    let steps = s.undo.len();
    let converted = s.execute("appearance.graph.convert", &json!({"look": look})).unwrap();
    assert_eq!((converted["id"].clone(), converted["count"].clone()), (json!("copper"), json!(2)), "{converted}");
    assert_eq!(s.undo.len(), steps + 1, "one step");
    let can = &s.doc.appearances.bodies["Can"];
    assert_eq!((can.color, can.name.as_str(), can.pbr, can.graph.clone()), ([204, 122, 80], "Copper", None, Some(gid("copper"))));
    // The one-node graph is the look's `pbr` exactly (§3.7), as the fixture spells it.
    let copper: solidcraft_doc::appearance_graph::MaterialGraph = serde_json::from_str(COPPER).unwrap();
    assert_eq!(s.doc.appearances.tables.graphs[&gid("copper")].nodes, copper.nodes);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.doc.appearances.bodies["Can"].pbr.is_some() && s.doc.appearances.tables.graphs.is_empty());
    // A scalar look, scoped to one body; `create {from: look}` makes the graph alone.
    s.execute("appearance.assign", &json!({"bodies": ["Lid"], "color": [10, 200, 30], "opacity": 0.5})).unwrap();
    let scalar = json!({"name": "Custom", "color": [10, 200, 30], "opacity": 0.5});
    let converted = s.execute("appearance.graph.convert", &json!({"look": scalar, "name": "Green", "scope": {"bodies": ["Lid"]}})).unwrap();
    assert_eq!(converted["count"], 1);
    let lid = &s.doc.appearances.bodies["Lid"];
    assert_eq!((lid.color, lid.opacity), ([10, 200, 30], 0.5), "an opaque graph keeps the legacy blend");
    let created = s.execute("appearance.graph.create", &json!({"name": "Green", "from": scalar})).unwrap();
    assert_eq!(created["id"], "green-2");
    let before = state(&s);
    let already = json!({"name": "Custom", "color": [10, 200, 30], "opacity": 0.5, "graph": "green"});
    assert!(s.execute("appearance.graph.convert", &json!({"look": already})).is_err());
    unchanged(&s, &before, "converting a graph look");
}

#[test]
fn library_copies_carry_their_graph_and_update_from_their_library() {
    use solidcraft_io::catalog::{Catalog, Definition, Entry, LibraryLook, Provenance, V3_FORMAT};
    let mut graph: solidcraft_doc::appearance_graph::MaterialGraph = serde_json::from_str(ETCHED).unwrap();
    graph.name = "Etched".into();
    let (key, texture) = solidcraft_doc::appearance_graph::TextureAsset::from_bytes("mark.png", PNG_2X1).unwrap();
    let look = |graph: &str| LibraryLook {
        name: "Etched lid".into(),
        color: [200, 198, 190],
        opacity: 1.0,
        pbr: None,
        graph: Some(gid(graph)),
        values: [("mark".to_string(), serde_json::from_value(json!({"texture": key})).unwrap())].into(),
    };
    let provenance = Provenance { license: "MIT OR Apache-2.0".into(), author: None, source_url: None };
    let catalog = |graph_data: &solidcraft_doc::appearance_graph::MaterialGraph| Catalog {
        format: V3_FORMAT.into(),
        id: "user.graphs".into(),
        name: "Graph test library".into(),
        entries: vec![Entry::new("user.graphs", Definition::Appearance(look("etched")), provenance.clone()).unwrap()],
        graphs: [(gid("etched"), graph_data.clone())].into(),
        textures: [(key.clone(), texture.clone())].into(),
    };
    let path = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-graph-library-{}.json", std::process::id()));
    let path_text = path.to_string_lossy().to_string();
    std::fs::write(&path, catalog(&graph).encode().unwrap()).unwrap();
    let mut s = with_body();
    let inspected = s.execute("library.inspect", &json!({"path": path_text})).unwrap();
    let entry = &inspected["catalog"]["entries"][0];
    let apply = json!({"path": path_text, "entry": entry["id"], "revision": entry["revision"], "targets": {"bodies": ["Can"]}});
    s.execute("library.apply", &apply).unwrap();
    let held = &s.doc.appearances.tables.graphs[&gid("etched")];
    let source = held.source.clone().unwrap();
    assert_eq!((source.library.as_str(), source.entry.as_str()), ("user.graphs", "etched"));
    assert!(s.doc.appearances.tables.textures.contains_key(&key));
    assert_eq!(s.doc.appearances.bodies["Can"].graph, Some(gid("etched")));
    // Applying again reuses the same graph; a different graph under that id needs a new id.
    s.execute("library.apply", &apply).unwrap();
    s.execute(
        "appearance.graph.edit",
        &json!({"id": "etched", "ops": [{"op": "set_input", "node": "plain", "port": "specular_roughness", "value": 0.3}]}),
    )
    .unwrap();
    let before = state(&s);
    assert!(s.execute("library.apply", &apply).is_err());
    unchanged(&s, &before, "a colliding graph id");
    let mut renamed = apply.clone();
    renamed["graph_id"] = json!("etched-library");
    s.execute("library.apply", &renamed).unwrap();
    assert_eq!(s.doc.appearances.bodies["Can"].graph, Some(gid("etched-library")));

    // The library changes: preview, then apply.
    let mut newer = graph.clone();
    for node in newer.nodes.iter_mut().filter(|n| n.id == "etched") {
        node.inputs.insert("specular_roughness".into(), serde_json::from_value(json!(0.5)).unwrap());
    }
    std::fs::write(&path, catalog(&newer).encode().unwrap()).unwrap();
    let before = state(&s);
    let preview = s.execute("appearance.graph.update_from_library", &json!({"id": "etched-library", "library": path_text, "preview": true})).unwrap();
    unchanged(&s, &before, "preview");
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["changes"], json!(["etched.specular_roughness: 0.7 → 0.5"]));
    assert_eq!(preview["looks"], 1);
    let applied = s.execute("appearance.graph.update_from_library", &json!({"id": "etched-library", "library": path_text})).unwrap();
    assert_eq!(applied["applied"], true);
    assert_eq!(s.undo.len(), before.1 + 1);
    let updated = &s.doc.appearances.tables.graphs[&gid("etched-library")];
    assert_eq!(updated.nodes, newer.nodes);
    assert_ne!(updated.source.as_ref().unwrap().revision, source.revision);
    // Up to date: nothing to apply.
    let again = s.execute("appearance.graph.update_from_library", &json!({"id": "etched-library", "library": path_text})).unwrap();
    assert_eq!((again["applied"].clone(), again["changes"].clone()), (json!(false), json!([])));
    // Exporting: v3 by default when a look names a graph; v2 explicitly refuses the graph.
    let out = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-graph-export-{}.json", std::process::id()));
    let out_text = out.to_string_lossy().to_string();
    let before = state(&s);
    assert!(s.execute("library.export", &json!({"path": out_text, "format": "solidcraft-library/2"})).is_err());
    let exported = s.execute("library.export", &json!({"path": out_text})).unwrap();
    assert_eq!(exported["catalog"]["format"], V3_FORMAT);
    assert!(Catalog::read(&out_text).unwrap().graphs.contains_key(&gid("etched-library")));
    s.execute("library.export", &json!({"path": out_text, "format": V3_FORMAT, "overwrite": true})).unwrap();
    unchanged(&s, &before, "export");
    let _ = std::fs::remove_file(&out);
    // A graph not from this library is refused.
    s.execute("appearance.graph.create", &json!({"id": "own", "name": "Own"})).unwrap();
    assert!(s.execute("appearance.graph.update_from_library", &json!({"id": "own", "library": path_text})).is_err());
    let _ = std::fs::remove_file(&path);
}

/// The new parameters are in the published schemas.
#[test]
fn schemas_describe_the_new_parameters() {
    use super::{GraphConvertParams, GraphCreateParams, GraphEditParams};
    use crate::schema::{input_schema_for, libraries::LibraryApplyParams};
    let text = |s: schemars::Schema| s.as_value().to_string();
    assert!(text(input_schema_for::<GraphEditParams>()).contains("fallback"));
    assert!(text(input_schema_for::<GraphCreateParams>()).contains("standard"));
    assert!(text(input_schema_for::<GraphConvertParams>()).contains("scope"));
    assert!(text(input_schema_for::<LibraryApplyParams>()).contains("graph_id"));
    let mut s = strict();
    assert!(s.execute("appearance.graph.convert", &json!({"look": {"color": [1, 2, 3]}, "extra": 1})).is_err());
    assert!(
        s.execute("appearance.graph.edit", &json!({"id": "a", "ops": [{"op": "connect", "from": "x", "node": "y", "port": "z", "weight": 1}]}))
            .is_err()
    );
}

/// Hostile parameters never panic and never change the design.
#[test]
fn hostile_parameters_are_refused_whole() {
    let mut s = with_body();
    s.execute("appearance.graph.create", &json!({"id": "copper", "name": "Copper", "graph": fixture(COPPER)})).unwrap();
    let before = state(&s);
    let long = "x".repeat(100_000);
    let deep = (0..200).fold(json!(1), |v, _| json!([v]));
    let hostile = [
        json!(null),
        json!([]),
        json!("text"),
        json!({"id": long}),
        json!({"id": "copper", "ops": [{"op": "set_input", "node": "surface", "port": long, "value": deep}]}),
        json!({"id": "copper", "ops": vec![json!({"op": "set_param", "node": "surface", "param": "p", "value": 1}); 513]}),
        json!({"id": "copper", "ops": [{"op": "set_interface", "input": {"name": long, "type": "float"}}]}),
        json!({"id": "copper", "ops": [{"op": "set_input", "node": "surface", "port": "base_color", "value": [f64::MAX, -1e308, 1]}]}),
        json!({"id": "copper", "graph": {"name": "x", "nodes": vec![json!({"id": "n", "kind": "standard_surface"}); 4096], "output": "n"}}),
        json!({"name": long}),
        json!({"name": "x", "from": {"color": "#zzzzzz"}}),
        json!({"look": {"color": [1, 2, 3], "pbr": {"metallic": 9}}}),
        json!({"data_base64": "A".repeat(1000)}),
        json!({"path": "\u{0}"}),
        json!({"id": "copper", "library": "/nonexistent/library.json"}),
    ];
    let ids = [
        "appearance.graph.create",
        "appearance.graph.convert",
        "appearance.graph.set",
        "appearance.graph.edit",
        "appearance.graph.duplicate",
        "appearance.graph.delete",
        "appearance.graph.update_from_library",
        "appearance.texture.add",
    ];
    for id in ids {
        for params in &hostile {
            let _ = s.execute(id, params);
            unchanged(&s, &before, &format!("{id} with hostile params"));
        }
    }
    for id in ["appearance.graph.get", "appearance.graph.list", "appearance.graph.validate", "appearance.texture.list"] {
        for params in &hostile {
            let _ = s.execute(id, params);
        }
    }
    unchanged(&s, &before, "reads");
}

/// `render.library.appearance.apply` with the `graph` variant: a Selene entry's baseline as a
/// one-node graph everywhere, and a `SURF-*` recipe's maps in bundled builds.
#[test]
fn library_recipes_apply_as_graphs() {
    let mut s = with_body();
    let selene = s.execute("render.library.appearance.apply", &json!({"id": "SELENE-AP-001", "variant": "graph", "bodies": ["Can"]})).unwrap();
    assert_eq!(selene["graph"], "selene-ap-001");
    let look = &s.doc.appearances.bodies["Can"];
    assert_eq!((look.graph.clone(), look.color), (Some(gid("selene-ap-001")), [0x71, 0x78, 0x82]), "the baseline colour, exactly");
    let source = s.doc.appearances.tables.graphs[&gid("selene-ap-001")].source.clone().unwrap();
    assert_eq!((source.library.as_str(), source.entry.as_str()), ("solidcraft-selene", "SELENE-AP-001"));
    // `uniform_opaque` is unchanged: it refuses the graph variant's fields.
    let before = state(&s);
    assert!(
        s.execute(
            "render.library.appearance.apply",
            &json!({"id": "SELENE-AP-001", "variant": "uniform_opaque", "bodies": ["Can"], "graph_id": "x"})
        )
        .is_err()
    );
    assert!(s.execute("render.library.appearance.apply", &json!({"id": "SELENE-AP-001", "variant": "graph", "bodies": ["Nope"]})).is_err());
    unchanged(&s, &before, "refused library applies");
    let surf = s.execute("render.library.appearance.apply", &json!({"id": "SURF-001", "variant": "graph", "bodies": ["Can"]}));
    if !cfg!(feature = "bundled-render-library") {
        assert!(surf.is_err());
        return;
    }
    let surf = surf.unwrap();
    assert_eq!(surf["maps"], json!(["base_color", "normal_open_gl"]));
    assert!(surf["omitted"].as_array().unwrap().contains(&json!("height_displacement")));
    assert_eq!(s.doc.appearances.tables.textures.len(), 2);
    let planar = s
        .execute(
            "render.library.appearance.apply",
            &json!({"id": "SURF-001", "variant": "graph", "bodies": ["Can"], "graph_id": "surf-001-planar", "frame": {"u": [100, 0, 0], "v": [0, 100, 0]}}),
        )
        .unwrap();
    assert_eq!(planar["maps"].as_array().unwrap().len(), 4);
    assert_eq!(s.doc.appearances.tables.textures.len(), 4);
    let text = s.doc.without_unreferenced_textures().to_json();
    solidcraft_doc::Document::from_json(&text).unwrap();
}
