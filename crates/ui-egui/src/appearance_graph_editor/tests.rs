//! The stack editor against the real engine: every command it sends runs on a design that
//! holds the graph under test as `g` (track D's commands), so the editor's commands match the
//! schemas and the engine accepts what the editor builds.

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::render::material_graph::{Constant, InputValue, MaterialGraph};

use super::model::{self, Driver};
use super::{Action, Commands, Editor, Pending, TextureUse};

const FIXTURES: &[(&str, &str, usize)] = &[
    ("black-epoxy-clear-coat", include_str!("../../../render/tests/fixtures/material_graph/black-epoxy-clear-coat.json"), 1),
    ("brushed-nickel-silver", include_str!("../../../render/tests/fixtures/material_graph/brushed-nickel-silver.json"), 1),
    ("electrolytic-sleeve", include_str!("../../../render/tests/fixtures/material_graph/electrolytic-sleeve.json"), 2),
    ("laser-etched-nickel-silver", include_str!("../../../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json"), 2),
    ("pbr-copper", include_str!("../../../render/tests/fixtures/material_graph/pbr-copper.json"), 1),
    ("solder-mask-over-copper", include_str!("../../../render/tests/fixtures/material_graph/solder-mask-over-copper.json"), 1),
];

/// A real 2×1 RGBA PNG (original test data).
const PNG_2X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0, 244, 34, 127, 138, 0, 0, 0, 14, 73, 68, 65,
    84, 120, 156, 99, 56, 161, 161, 241, 31, 132, 1, 18, 51, 4, 47, 169, 127, 204, 104, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn fixture(name: &str) -> MaterialGraph {
    let text = FIXTURES.iter().find(|(n, _, _)| *n == name).map(|(_, t, _)| *t).unwrap();
    serde_json::from_str(text).unwrap()
}

fn catalogue() -> model::Catalogue {
    let reply = Session::default().execute("appearance.graph.nodes", &json!({})).unwrap();
    model::Catalogue::from_reply(&reply).unwrap()
}

/// The real engine with one graph, `g`, recording the changing commands the editor sends.
/// `refuse` stands in for an engine refusal the editor cannot provoke by itself.
struct Fake {
    engine: Session,
    /// The engine's graph `g`, read back after every change.
    graph: MaterialGraph,
    changes: Vec<(String, Value)>,
    refuse: bool,
    /// The revision to report instead of the engine's (to hand the editor to an app).
    revision: Option<u64>,
}

impl Fake {
    fn new(graph: MaterialGraph) -> Fake {
        let mut engine = Session::default();
        engine.param_check = solidcraft_engine::schema::ParamCheck::Strict;
        engine.execute("appearance.graph.create", &json!({"id": "g", "name": graph.name, "graph": graph})).unwrap();
        let graph = Self::read(&mut engine);
        Fake { engine, graph, changes: Vec::new(), refuse: false, revision: None }
    }

    fn read(engine: &mut Session) -> MaterialGraph {
        let reply = engine.execute("appearance.graph.get", &json!({"id": "g"})).unwrap();
        serde_json::from_value(reply["graph"].clone()).unwrap()
    }

    /// Add `bytes` as a texture in the engine; its key.
    fn texture(&mut self, bytes: &[u8]) -> String {
        let reply =
            self.engine.execute("appearance.texture.add", &json!({"data_base64": solidcraft_engine::doc::canvas::base64_encode(bytes)})).unwrap();
        reply["texture"].as_str().unwrap().to_string()
    }
}

impl Commands for Fake {
    fn query(&mut self, id: &str, params: Value) -> Result<Value, String> {
        assert!(id.starts_with("appearance.graph.") || id == "appearance.texture.list", "unexpected query {id}");
        self.engine.execute(id, &params).map_err(|e| e.to_string())
    }

    fn change(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.changes.push((id.to_string(), params.clone()));
        if self.refuse {
            return Err("refused: the graph is over budget".into());
        }
        let reply = self.engine.execute(id, &params).map_err(|e| e.to_string());
        if self.engine.doc.appearances.tables.graphs.keys().any(|k| k.as_str() == "g") {
            self.graph = Self::read(&mut self.engine);
        }
        reply
    }

    fn revision(&self) -> u64 {
        self.revision.unwrap_or(self.engine.revision)
    }
}

fn editor(fake: &mut Fake) -> Editor {
    let mut ed = Editor::default();
    ed.sync(fake);
    ed
}

fn set_graph(fake: &Fake) -> MaterialGraph {
    let (id, params) = fake.changes.last().unwrap();
    assert_eq!(id, "appearance.graph.set");
    serde_json::from_value(params["graph"].clone()).unwrap()
}

#[test]
fn the_catalogue_reply_reads_every_kind_and_port() {
    let c = catalogue();
    assert_eq!(c.nodes.len(), solidcraft_engine::render::material_graph::CATALOGUE.len());
    let roughness = c.port("standard_surface", "specular_roughness").unwrap();
    assert_eq!(roughness.default, model::PortDefault::Float(0.3));
    assert!(roughness.range.is_some_and(|(lo, hi)| lo > 0.0 && hi == 1.0));
    assert_eq!(c.port("standard_surface", "geometry_normal").unwrap().default, model::PortDefault::Geometric);
    assert!(matches!(&c.kind("pattern.noise").unwrap().params[0].kind, model::ParamKind::Choice { options, .. } if options.len() == 3));
    // Every surface input belongs to exactly one group.
    let surface = c.kind("standard_surface").unwrap();
    for input in &surface.inputs {
        assert_eq!(model::GROUPS.iter().filter(|(_, ports)| ports.contains(&input.name.as_str())).count(), 1, "{}", input.name);
    }
    assert!(model::Catalogue::from_reply(&json!({"nodes": "x"})).is_err());
}

#[test]
fn every_worked_example_is_stack_shaped() {
    for (name, text, layers) in FIXTURES {
        let graph: MaterialGraph = serde_json::from_str(text).unwrap();
        let stack = model::stack(&graph).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(stack.layers.len(), *layers, "{name}");
        assert!(stack.layers[0].mix.is_none());
        assert!(stack.layers[1..].iter().all(|l| l.mix.is_some()));
    }
    let etched = model::stack(&fixture("laser-etched-nickel-silver")).unwrap();
    assert_eq!(etched.layers[0].surface, "plain");
    assert_eq!(etched.layers[1].surface, "etched");
}

#[test]
fn graphs_the_stack_cannot_show_are_named_not_drawn() {
    let mut g = fixture("laser-etched-nickel-silver");
    // fg is a mix: not a stack.
    let mix = g.nodes.iter_mut().find(|n| n.id == "surface").unwrap();
    mix.inputs.insert("fg".into(), InputValue::Connection(model_connection("surface")));
    assert_eq!(model::stack(&g), Err("appearance.graph.not_stack_mix"));
    let mut g = fixture("brushed-nickel-silver");
    g.output = "rough".into();
    assert_eq!(model::stack(&g), Err("appearance.graph.not_stack_output"));
    g.output = "missing".into();
    assert!(model::stack(&g).is_err());
    // A bg cycle through two mixes ends.
    let mut g = fixture("electrolytic-sleeve");
    let mix = g.nodes.iter_mut().find(|n| n.id == "surface").unwrap();
    mix.inputs.insert("bg".into(), InputValue::Connection(model_connection("surface")));
    assert!(model::stack(&g).is_err());
}

fn model_connection(from: &str) -> solidcraft_engine::render::material_graph::Connection {
    solidcraft_engine::render::material_graph::Connection { from: from.into(), fallback: None }
}

#[test]
fn the_editor_lists_creates_and_undoes_through_the_real_engine() {
    let mut app = crate::SolveApp::new(Session::default(), crate::Services::default());
    let before = app.session.undo.len();
    let mut ed = Editor::default();
    ed.sync(&mut app);
    assert!(matches!(&ed.catalogue, Some(Ok(_))));
    assert!(matches!(&ed.graphs, Some(Ok(list)) if list.is_empty()));
    assert!(ed.loaded.is_none());
    ed.apply(&mut app, Action::Create("Brushed".into()));
    assert!(ed.note.is_none(), "{:?}", ed.note);
    assert_eq!(app.session.undo.len(), before + 1);
    ed.sync(&mut app);
    assert_eq!(ed.graph().unwrap().id, "brushed");
    assert!(matches!(&ed.validation, Some(Ok(v)) if v.ok));
}

#[test]
fn a_value_edit_is_one_command_sent_when_it_settles() {
    let mut fake = Fake::new(fixture("brushed-nickel-silver"));
    let mut ed = editor(&mut fake);
    assert_eq!(ed.graph().unwrap().id, "g");
    assert!(matches!(&ed.validation, Some(Ok(v)) if v.ok && v.cost > 0));
    // A drag shows its value without sending anything.
    for x in [0.5, 0.6, 0.7] {
        ed.apply(&mut fake, Action::Pending(Pending::Input { node: "surface".into(), port: "base_metalness".into(), value: Constant::Number(x) }));
    }
    assert!(fake.changes.is_empty());
    ed.commit(&mut fake);
    assert_eq!(fake.changes.len(), 1);
    let (id, params) = &fake.changes[0];
    assert_eq!(id, "appearance.graph.edit");
    assert_eq!(params, &json!({"id": "g", "ops": [{"op": "set_input", "node": "surface", "port": "base_metalness", "value": 0.7}]}));
    ed.commit(&mut fake);
    assert_eq!(fake.changes.len(), 1, "nothing pending, nothing sent");
    ed.sync(&mut fake);
    let surface = model::node(&ed.graph().unwrap().graph, "surface").unwrap();
    assert_eq!(model::shown_constant(surface, "base_metalness"), Some(&Constant::Number(0.7)));
}

#[test]
fn a_refused_edit_leaves_the_replies_and_shows_why() {
    let mut fake = Fake::new(fixture("brushed-nickel-silver"));
    let mut ed = editor(&mut fake);
    let before = ed.loaded.clone();
    fake.refuse = true;
    ed.apply(&mut fake, Action::Pending(Pending::Param { node: "uv".into(), param: "space".into(), value: model_param("world") }));
    ed.commit(&mut fake);
    assert!(ed.note.as_ref().is_some_and(|(n, e)| *e && n.contains("over budget")));
    assert!(ed.pending.is_none());
    ed.sync(&mut fake);
    assert_eq!(ed.loaded, before);
}

fn model_param(text: &str) -> solidcraft_engine::render::material_graph::ParamValue {
    solidcraft_engine::render::material_graph::ParamValue::Text(text.into())
}

#[test]
fn layers_are_added_removed_and_reordered_by_whole_graph_replacement() {
    let mut graph = fixture("laser-etched-nickel-silver");
    graph.layout.insert("plain".into(), [10.0, 20.0]);
    graph.layout.insert("pits".into(), [30.0, 40.0]);
    let mut fake = Fake::new(graph);
    let mut ed = editor(&mut fake);
    let stack = model::stack(&ed.graph().unwrap().graph).unwrap();
    // Add: a new top layer copying the top surface's constants, half mixed, layout kept.
    let added = model::add_layer(&ed.graph().unwrap().graph, &stack).unwrap();
    ed.apply(&mut fake, Action::Set(added));
    let g = set_graph(&fake);
    let s = model::stack(&g).unwrap();
    assert_eq!(s.layers.len(), 3);
    let top = model::node(&g, &s.layers[2].surface).unwrap();
    assert_eq!(model::shown_constant(top, "specular_roughness"), Some(&Constant::Number(0.7)));
    assert!(!top.inputs.contains_key("geometry_normal"), "wires are not copied");
    assert_eq!(g.layout.get("pits"), Some(&[30.0, 40.0]));
    ed.sync(&mut fake);
    // Reorder: the two masked layers swap surfaces and masks.
    let stack = model::stack(&ed.graph().unwrap().graph).unwrap();
    let swapped = model::swap_layers(&ed.graph().unwrap().graph, &stack, 1).unwrap();
    let s2 = model::stack(&swapped).unwrap();
    assert_eq!(s2.layers[1].surface, stack.layers[2].surface);
    assert_eq!(s2.layers[2].surface, stack.layers[1].surface);
    let mask = |g: &MaterialGraph, l: &model::Layer| model::node(g, l.mix.as_deref().unwrap()).unwrap().inputs.get("mix").cloned();
    assert_eq!(mask(&swapped, &s2.layers[2]), mask(&ed.graph().unwrap().graph, &stack.layers[1]));
    assert!(model::swap_layers(&swapped, &s2, 0).is_err(), "the base has no mask and stays at the bottom");
    // Remove the etched layer (in the middle): the top mix now lies on the base, and the
    // mask's image, the pits and their projection go with it; the layout loses their entries.
    let removed = model::remove_layer(&ed.graph().unwrap().graph, &stack, 1).unwrap();
    let s3 = model::stack(&removed).unwrap();
    assert_eq!(s3.layers.iter().map(|l| l.surface.as_str()).collect::<Vec<_>>(), ["plain", stack.layers[2].surface.as_str()]);
    for gone in ["etched", "mask", "pits", "pit_n", "lid", "surface"] {
        assert!(model::node(&removed, gone).is_none(), "{gone}");
    }
    assert_eq!(removed.layout.get("plain"), Some(&[10.0, 20.0]));
    assert!(!removed.layout.contains_key("pits"));
    assert!(model::remove_layer(&removed, &s3, 0).is_err(), "the base stays");
    ed.apply(&mut fake, Action::Set(removed));
    // Removing the top layer moves the output down.
    let mut fake = Fake::new(fixture("laser-etched-nickel-silver"));
    let stack = model::stack(&fake.graph).unwrap();
    let g = model::remove_layer(&fake.graph, &stack, 1).unwrap();
    assert_eq!(g.output, "plain");
    assert_eq!(g.nodes.len(), 1);
    let mut ed = editor(&mut fake);
    ed.apply(&mut fake, Action::Set(g));
    assert_eq!(fake.changes.len(), 1);
}

#[test]
fn driving_an_input_keeps_its_value_as_the_fallback_and_reuses_the_projection() {
    let c = catalogue();
    let graph = fixture("brushed-nickel-silver");
    let g = model::drive_with(&graph, &c, "surface", "base_metalness", Driver::Noise, None).unwrap();
    let surface = model::node(&g, "surface").unwrap();
    let Some(InputValue::Connection(wire)) = surface.inputs.get("base_metalness") else { panic!("wired") };
    assert_eq!(wire.fallback, Some(Constant::Number(1.0)));
    let chain = model::upstream(&g, wire.source().0);
    assert_eq!(chain.iter().map(|n| n.kind.as_str()).collect::<Vec<_>>(), ["remap", "pattern.noise", "texcoord.planar"]);
    assert_eq!(chain[2].id, "uv", "the graph's projection is reused");
    assert_eq!(model::shown_constant(chain[0], "out_max"), Some(&Constant::Number(1.0)));
    // A colour gets a colour range; a normal a relief; a tangent the pattern's own.
    let g = model::drive_with(&graph, &c, "surface", "coat_color", Driver::Checker, None).unwrap();
    let wire = match model::node(&g, "surface").unwrap().inputs.get("coat_color") {
        Some(InputValue::Connection(w)) => w.clone(),
        _ => panic!("wired"),
    };
    assert_eq!(model::shown_constant(model::node(&g, wire.source().0).unwrap(), "out_max"), Some(&Constant::Vector(vec![1.0, 1.0, 1.0])));
    let g = model::drive_with(&graph, &c, "surface", "geometry_normal", Driver::Noise, None).unwrap();
    assert!(g.nodes.iter().any(|n| n.kind == "normal.from_height"));
    let fresh = fixture("pbr-copper");
    let g =
        model::drive_with(&fresh, &c, model::stack(&fresh).unwrap().layers[0].surface.as_str(), "geometry_tangent", Driver::Brushed, None).unwrap();
    assert!(g.nodes.iter().any(|n| n.kind == "texcoord.planar"), "a projection is added when the graph has none");
    assert!(model::drive_with(&graph, &c, "surface", "base_color", Driver::Image, None).is_err(), "an image needs a texture");
    // Undrive: the fallback comes back as a constant and only what the driver used goes.
    let g = model::undrive(&graph, "surface", "specular_roughness", Some(Constant::Number(0.45))).unwrap();
    assert!(model::node(&g, "rough").is_none());
    assert!(model::node(&g, "lines").is_some(), "still read by the tint and the tangent");
    assert_eq!(model::shown_constant(model::node(&g, "surface").unwrap(), "specular_roughness"), Some(&Constant::Number(0.45)));
}

#[test]
fn an_added_texture_drives_an_image_and_textures_bind_interfaces() {
    let mut fake = Fake::new(fixture("brushed-nickel-silver"));
    let mut ed = editor(&mut fake);
    let use_ = TextureUse::Drive { node: "surface".into(), port: "base_color".into() };
    assert_eq!(ed.apply(&mut fake, Action::AddTexture(use_.clone())), Some(use_.clone()), "the picker opens");
    let dir = solidcraft_engine::io::vfs::temp_dir().join(format!("solidcraft-graph-editor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mark.png").to_string_lossy().to_string();
    std::fs::write(&path, PNG_2X1).unwrap();
    ed.add_texture(&mut fake, &path, use_);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(fake.changes[0], ("appearance.texture.add".into(), json!({"path": path})));
    let key = solidcraft_engine::doc::appearance_graph::texture_key(PNG_2X1);
    let g = set_graph(&fake);
    let image = g.nodes.iter().find(|n| n.kind == "image").unwrap();
    assert!(matches!(image.inputs.get("file"), Some(InputValue::Constant(Constant::Texture(t))) if t.texture == key));
    assert_eq!(fake.graph, g, "the engine took the driven graph");
    // Laser-etched's mark is an interface input: its default is set by set_interface.
    let mut fake = Fake::new(fixture("laser-etched-nickel-silver"));
    let key = fake.texture(PNG_2X1);
    let mut ed = editor(&mut fake);
    let input = fake.graph.interface[0].clone();
    ed.apply(&mut fake, Action::Texture(key.clone(), TextureUse::Interface(input)));
    assert!(ed.note.is_none(), "{:?}", ed.note);
    let (id, params) = fake.changes.last().unwrap();
    assert_eq!(id, "appearance.graph.edit");
    assert_eq!(params["ops"][0]["op"], "set_interface");
    assert_eq!(params["ops"][0]["input"]["default"], json!({"texture": key}));
}

#[test]
fn graphs_are_chosen_created_duplicated_and_replaced_as_json() {
    let mut fake = Fake::new(fixture("pbr-copper"));
    let mut ed = editor(&mut fake);
    ed.apply(&mut fake, Action::Create("  ".into()));
    assert!(fake.changes.is_empty(), "an empty name is not sent");
    ed.apply(&mut fake, Action::Create("Anodised".into()));
    assert_eq!(fake.changes[0], ("appearance.graph.create".into(), json!({"name": "Anodised"})));
    ed.apply(&mut fake, Action::Duplicate);
    assert_eq!(fake.changes[1], ("appearance.graph.duplicate".into(), json!({"id": "g"})));
    ed.apply(&mut fake, Action::Json("{not json".into()));
    assert_eq!(fake.changes.len(), 2);
    assert!(ed.note.as_ref().is_some_and(|(_, e)| *e));
    let text = serde_json::to_string(&fixture("brushed-nickel-silver")).unwrap();
    ed.sync(&mut fake);
    ed.apply(&mut fake, Action::Json(text));
    assert_eq!(set_graph(&fake), fixture("brushed-nickel-silver"));
}

#[test]
fn values_read_srgb_constants_defaults_and_clamp_to_range() {
    let c = catalogue();
    let color = c.port("standard_surface", "base_color").unwrap();
    let srgb: Constant = serde_json::from_value(json!({"srgb8": [255, 0, 128]})).unwrap();
    let v = model::value_of(Some(&srgb), color, 3);
    assert!((v[0] - 1.0).abs() < 1e-9 && v[1] == 0.0 && v[2] > 0.2 && v[2] < 0.23);
    assert_eq!(model::value_of(None, color, 3), vec![0.8; 3]);
    assert_eq!(model::value_of(Some(&Constant::Number(0.5)), color, 3), vec![0.5; 3], "a float broadcasts");
    assert_eq!(model::constant(&[f64::NAN], Some((0.0, 1.0))), Constant::Number(0.0));
    assert_eq!(model::constant(&[7.0, -1.0, 0.5], Some((0.0, 1.0))), Constant::Vector(vec![1.0, 0.0, 0.5]));
    assert_eq!(model::unit("density_per_mm"), model::Unit::PerMm);
    assert_eq!(model::unit("radius_mm"), model::Unit::Mm);
    assert_eq!(model::unit("specular_rotation"), model::Unit::Degrees);
    let mut g = fixture("pbr-copper");
    for _ in 0..3 {
        let id = model::fresh_id(&g, "Base Colour.Range!");
        assert!(id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'), "{id}");
        assert!(model::node(&g, &id).is_none());
        g.nodes.push(solidcraft_engine::render::material_graph::Node {
            id,
            kind: "constant".into(),
            params: Default::default(),
            inputs: Default::default(),
        });
    }
}

/// Hostile replies never panic the editor: wrong shapes become messages.
#[test]
fn hostile_replies_become_messages() {
    for reply in [json!(null), json!({"graphs": 3}), json!({"graphs": [{"id": 1}]}), json!([1, 2])] {
        assert!(model::graph_list(&reply).is_err());
        assert!(model::texture_list(&reply).is_err());
        assert!(model::Loaded::from_reply(&reply).is_err());
        assert!(model::Validation::from_reply(&reply).is_err());
    }
    let mut nodes: Vec<Value> = (0..200).map(|i| json!({"id": format!("n{i}"), "kind": "constant"})).collect();
    nodes.push(json!({"id": "s", "kind": "standard_surface"}));
    let reply = json!({"id": "g", "graph": {"name": "x", "nodes": nodes, "output": "s"}});
    assert!(model::Loaded::from_reply(&reply).is_err(), "more nodes than a graph may hold");
}

/// Draw the section for every worked example and both looks of stack and summary, with the
/// engine's replies; nothing is sent while nobody touches it.
#[test]
fn the_section_draws_every_example_without_sending_anything() {
    let ctx = crate::fonts::test_context();
    for (name, _, _) in FIXTURES {
        let mut fake = Fake::new(fixture(name));
        let ed = editor(&mut fake);
        let mut actions = Vec::new();
        ctx.run_ui(Default::default(), |ui| {
            ui_scope(ui, |ui| actions = super::body(ui, &ed, false, None));
        })
        .textures_delta
        .clear();
        assert!(actions.is_empty(), "{name}: {actions:?}");
        assert!(fake.changes.is_empty());
    }
    // Not stack-shaped: a summary, no layer rows. Track A's v1 rules make every graph the engine
    // holds stack-shaped (a mix's `fg` is a standard surface), so this one is served as replies.
    let mut g = fixture("brushed-nickel-silver");
    g.output = "rough".into();
    let mut canned = Canned(g);
    let mut ed = Editor::default();
    ed.sync(&mut canned);
    ctx.run_ui(Default::default(), |ui| ui_scope(ui, |ui| assert!(super::body(ui, &ed, false, None).is_empty()))).textures_delta.clear();
}

/// Replies for a graph the engine would not hold; it changes nothing.
struct Canned(MaterialGraph);

impl Commands for Canned {
    fn query(&mut self, id: &str, params: Value) -> Result<Value, String> {
        match id {
            "appearance.graph.nodes" => Session::default().execute(id, &params).map_err(|e| e.to_string()),
            "appearance.graph.list" => Ok(json!({"graphs": [{"id": "g", "name": self.0.name, "uses": 0}]})),
            "appearance.graph.get" => Ok(json!({"id": "g", "graph": self.0, "cost": 0, "uses": 0})),
            "appearance.graph.validate" => Session::default().execute(id, &params).map_err(|e| e.to_string()),
            "appearance.texture.list" => Ok(json!({"textures": []})),
            other => Err(format!("unexpected query {other}")),
        }
    }
    fn change(&mut self, id: &str, _params: Value) -> Result<Value, String> {
        Err(format!("{id} is not sent while nobody touches the section"))
    }
    fn revision(&self) -> u64 {
        1
    }
}

fn ui_scope(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    crate::ui_kit::scope(ui, add);
}

/// The swatch's scene is drawn by the viewport's product pipelines on the GPU. Skips without
/// an adapter. `SOLIDCRAFT_SWATCH_PNG` names a file to keep the picture in.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_swatch_is_drawn_by_the_gpu_product_pipelines() {
    use solidcraft_engine::geom::Vec3;
    use solidcraft_engine::render::surface::OpaqueSurface;
    let surface = OpaqueSurface::new(Vec3::new(0.8, 0.77, 0.7), 1.0, 0.35, 1.5).unwrap();
    let scene = super::swatch::scene(surface);
    assert_eq!(scene.pbr.len() % crate::gpu::PBR_SIZE, 0);
    assert!(scene.pbr.len() / crate::gpu::PBR_SIZE >= 3 * 2 * 24 * 48);
    assert_ne!(super::swatch::key(surface), super::swatch::key(OpaqueSurface::new(Vec3::new(0.8, 0.77, 0.7), 0.0, 0.35, 1.5).unwrap()));
    let size = 256;
    let camera = super::swatch::camera();
    let eye = camera.eye();
    let Some(rgba) = crate::gpu::render_product_offscreen(
        scene,
        None,
        camera.view_proj(1.0, super::swatch::RADIUS).to_f32(),
        camera.back().to_f32(),
        [eye.x as f32, eye.y as f32, eye.z as f32, 1.0],
        size,
        false,
    ) else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    if let Ok(path) = std::env::var("SOLIDCRAFT_SWATCH_PNG") {
        image::RgbaImage::from_raw(size, size, rgba.clone()).unwrap().save(path).unwrap();
    }
    // The sphere's and the coupon's centres are drawn, not the white clear.
    let to_pixel = |p: [f64; 3]| {
        let c = camera.view_proj(1.0, super::swatch::RADIUS).apply(Vec3::new(p[0], p[1], p[2]));
        let (x, y) = ((c[0] / c[3] * 0.5 + 0.5) * f64::from(size), (0.5 - c[1] / c[3] * 0.5) * f64::from(size));
        ((y as u32).min(size - 1) * size + (x as u32).min(size - 1)) as usize * 4
    };
    for centre in [[-1.15, 0.0, 1.0], [1.2, -0.3, 0.85]] {
        let i = to_pixel(centre);
        assert_ne!(&rgba[i..i + 3], &[255, 255, 255], "{centre:?}");
    }
}

/// The Appearance dialog with the editor filled from the engine's replies, rendered from the
/// app's egui meshes. `SOLIDCRAFT_GRAPH_EDITOR_PNG` names a directory to keep the pictures in.
#[test]
fn the_dialog_shows_the_stack_editor_in_both_themes() {
    use std::collections::HashMap;
    let previous = crate::theme::is_dark();
    for dark in [false, true] {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.call("ui.resize", json!({"width": 1100, "height": 1500}));
        h.call("ui.set", json!({"dark": dark}));
        h.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10})).unwrap();
        h.app.session.selection = vec![solidcraft_engine::Sel::Body { name: "Body1".into() }];
        h.app.start("appearance.assign");
        let mut fake = Fake::new(fixture("laser-etched-nickel-silver"));
        fake.revision = Some(h.app.session.revision);
        fake.texture(PNG_2X1);
        let mut ed = editor(&mut fake);
        ed.layer = 1;
        if let Some(crate::dialogs::Kind::Appearance(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            *k.material = ed;
        }
        h.ctx = egui::Context::default();
        crate::fonts::setup_fonts(&h.ctx);
        crate::theme::apply(&h.ctx);
        let mut textures: HashMap<egui::TextureId, egui::ColorImage> = HashMap::new();
        let mut output = None;
        for _ in 0..10 {
            let out = h.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 1500.0))),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    h.app.logic(ui.ctx());
                    h.app.ui(ui);
                },
            );
            let mut out = out;
            crate::ui_kit::test_textures_update(&mut textures, &out.textures_delta);
            out.textures_delta.clear();
            output = Some(out);
        }
        let Some(crate::dialogs::Kind::Appearance(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("appearance dialog") };
        assert!(k.material.graph().is_some(), "the replies are kept: the design did not change");
        let output = output.unwrap();
        if let Ok(dir) = std::env::var("SOLIDCRAFT_GRAPH_EDITOR_PNG") {
            let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 1500.0));
            let image = crate::ui_kit::test_raster(&h.ctx, output.shapes, output.pixels_per_point, bounds, &textures);
            image.save(std::path::Path::new(&dir).join(format!("graph-editor-{}.png", if dark { "dark" } else { "light" }))).unwrap();
        }
    }
    crate::theme::set_dark(previous);
}
