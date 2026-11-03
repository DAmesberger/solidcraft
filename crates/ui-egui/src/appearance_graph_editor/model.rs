//! The stack editor's data: the command replies it reads (catalogue, graph, validation,
//! textures), the layered-stack view of a graph, and the edits it sends back. Nothing here
//! draws or runs a command, so every rule is testable with schema-shaped replies.
//!
//! A graph is *stack-shaped* (docs/material-graph.md §7.1) when its output is one
//! `standard_surface`, or a chain of `surface.mix` nodes whose `fg` is a `standard_surface`
//! and whose `bg` continues the chain down to a base `standard_surface`. Each surface input is
//! a constant, an interface value or a wire from upstream nodes (projection → pattern →
//! maths). Values change through `appearance.graph.edit`; structure (layers, drivers) changes
//! through `appearance.graph.set`, so a driven input keeps its constant as the connection's
//! `fallback` (the `connect` edit has no fallback field). The editor-only `layout` map is
//! carried through every `set` untouched, except for entries of nodes the edit removed.

use std::collections::{BTreeSet, VecDeque};

use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_engine::render::material_graph::{
    Connection, Constant, InputValue, InterfaceInput, MaterialGraph, Node, ParamValue, PortType, limits,
};

/// Node kinds a catalogue reply may list (the v1 catalogue has 49).
const MAX_KINDS: usize = 512;
/// Entries a texture list reply may hold.
const MAX_TEXTURES: usize = limits::TEXTURES_PER_DESIGN * 4;
/// Graphs a list reply may hold.
const MAX_GRAPHS: usize = limits::GRAPHS_PER_DESIGN;

// ---------------------------------------------------------------------------------------------
// Replies.

/// `appearance.graph.nodes`: the node catalogue.
#[derive(Clone, Debug, Deserialize)]
pub struct Catalogue {
    pub nodes: Vec<KindSpec>,
}

/// One node kind of the catalogue reply.
#[derive(Clone, Debug, Deserialize)]
pub struct KindSpec {
    pub kind: String,
    pub inputs: Vec<PortSpec>,
    pub params: Vec<ParamSpec>,
    /// Cost units before expansion (docs/material-graph.md §3.6).
    pub cost: u32,
}

/// One input of a node kind.
#[derive(Clone, Debug, Deserialize)]
pub struct PortSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PortType,
    pub range: Option<(f64, f64)>,
    pub default: PortDefault,
}

/// What an input is when nothing drives it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortDefault {
    Required,
    Optional,
    Geometric,
    Float(f64),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
}

/// A compile-time param.
#[derive(Clone, Debug, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    pub kind: ParamKind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Choice { options: Vec<String>, default: String },
    Int { min: i64, max: i64, default: i64 },
}

impl Catalogue {
    /// The catalogue in an `appearance.graph.nodes` reply.
    pub fn from_reply(reply: &Value) -> Result<Catalogue, String> {
        let catalogue: Catalogue = serde_json::from_value(reply.clone()).map_err(|e| e.to_string())?;
        if catalogue.nodes.len() > MAX_KINDS {
            return Err(format!("the catalogue lists {} node kinds; at most {MAX_KINDS} are read", catalogue.nodes.len()));
        }
        Ok(catalogue)
    }
    pub fn kind(&self, kind: &str) -> Option<&KindSpec> {
        self.nodes.iter().find(|k| k.kind == kind)
    }
    pub fn port(&self, kind: &str, port: &str) -> Option<&PortSpec> {
        self.kind(kind)?.inputs.iter().find(|p| p.name == port)
    }
}

/// `appearance.graph.get`: one graph with its stats.
#[derive(Clone, Debug, PartialEq)]
pub struct Loaded {
    pub id: String,
    pub graph: MaterialGraph,
    pub cost: u32,
    pub uses: u32,
}

impl Loaded {
    pub fn from_reply(reply: &Value) -> Result<Loaded, String> {
        let id = reply.get("id").and_then(Value::as_str).ok_or("the reply has no graph id")?.to_string();
        let graph: MaterialGraph = serde_json::from_value(reply.get("graph").cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
        if graph.interface.len() > limits::INTERFACE_INPUTS {
            return Err(format!("the graph has {} interface inputs; at most {} are read", graph.interface.len(), limits::INTERFACE_INPUTS));
        }
        if graph.nodes.len() > limits::NODES {
            return Err(format!("the graph has {} nodes; at most {} are read", graph.nodes.len(), limits::NODES));
        }
        let count = |key: &str| reply.get(key).and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
        Ok(Loaded { id, graph, cost: count("cost"), uses: count("uses") })
    }
}

/// One graph of `appearance.graph.list`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct GraphEntry {
    pub id: String,
    pub name: String,
    pub uses: u32,
    #[serde(default)]
    pub source: Option<Value>,
}

pub fn graph_list(reply: &Value) -> Result<Vec<GraphEntry>, String> {
    let graphs: Vec<GraphEntry> = serde_json::from_value(reply.get("graphs").cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
    if graphs.len() > MAX_GRAPHS {
        return Err(format!("the design lists {} graphs; at most {MAX_GRAPHS} are read", graphs.len()));
    }
    Ok(graphs)
}

/// One texture of `appearance.texture.list`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TextureEntry {
    pub texture: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub uses: u32,
}

pub fn texture_list(reply: &Value) -> Result<Vec<TextureEntry>, String> {
    let textures: Vec<TextureEntry> = serde_json::from_value(reply.get("textures").cloned().unwrap_or(Value::Null)).map_err(|e| e.to_string())?;
    if textures.len() > MAX_TEXTURES {
        return Err(format!("the design lists {} textures; at most {MAX_TEXTURES} are read", textures.len()));
    }
    Ok(textures)
}

/// `appearance.graph.validate`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Validation {
    pub ok: bool,
    pub cost: u32,
    pub samples: u32,
    pub textures: u32,
    #[serde(default)]
    pub shape: String,
    #[serde(default)]
    pub errors: Vec<Issue>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Issue {
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub port: Option<String>,
    pub code: String,
    pub message: String,
}

impl Validation {
    pub fn from_reply(reply: &Value) -> Result<Validation, String> {
        let mut v: Validation = serde_json::from_value(reply.clone()).map_err(|e| e.to_string())?;
        v.errors.truncate(limits::NODES * 4);
        Ok(v)
    }
}

// ---------------------------------------------------------------------------------------------
// The stack view.

/// One layer of the stack: a surface, and (above the base) the `surface.mix` that lays it over
/// the layers below with its mask (the mix's `mix` input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub surface: String,
    pub mix: Option<String>,
}

/// The layers of a stack-shaped graph, base first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    pub layers: Vec<Layer>,
}

/// Why a graph is not stack-shaped, as a message id.
pub type NotStack = &'static str;

pub fn node<'a>(graph: &'a MaterialGraph, id: &str) -> Option<&'a Node> {
    graph.nodes.iter().find(|n| n.id == id)
}

fn node_mut<'a>(graph: &'a mut MaterialGraph, id: &str) -> Option<&'a mut Node> {
    graph.nodes.iter_mut().find(|n| n.id == id)
}

/// The node a surface port is wired from, if it names a whole surface node.
fn surface_source<'a>(graph: &'a MaterialGraph, n: &Node, port: &str) -> Option<&'a Node> {
    let InputValue::Connection(c) = n.inputs.get(port)? else { return None };
    let (from, output) = c.source();
    let source = node(graph, from)?;
    (output.is_none_or(|o| o == "surface")).then_some(source)
}

/// The layers of `graph`, or why it is not stack-shaped.
pub fn stack(graph: &MaterialGraph) -> Result<Stack, NotStack> {
    let mut layers = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = node(graph, &graph.output).ok_or("appearance.graph.not_stack_output")?;
    // Each step consumes a distinct node, so a cycle ends the walk.
    for _ in 0..=graph.nodes.len() {
        if !seen.insert(current.id.as_str()) {
            return Err("appearance.graph.not_stack_cycle");
        }
        match current.kind.as_str() {
            "standard_surface" => {
                layers.push(Layer { surface: current.id.clone(), mix: None });
                layers.reverse();
                return Ok(Stack { layers });
            }
            "surface.mix" => {
                let fg = surface_source(graph, current, "fg").ok_or("appearance.graph.not_stack_mix")?;
                if fg.kind != "standard_surface" || !seen.insert(fg.id.as_str()) {
                    return Err("appearance.graph.not_stack_mix");
                }
                layers.push(Layer { surface: fg.id.clone(), mix: Some(current.id.clone()) });
                current = surface_source(graph, current, "bg").ok_or("appearance.graph.not_stack_mix")?;
            }
            _ => return Err("appearance.graph.not_stack_output"),
        }
    }
    Err("appearance.graph.not_stack_cycle")
}

/// The surface-input groups, in the order the editor shows them, with the catalogue's
/// `standard_surface` inputs each holds.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("appearance.graph.group_base", &["base_color", "base_metalness", "base_weight"]),
    (
        "appearance.graph.group_specular",
        &["specular_roughness", "specular_ior", "specular_weight", "specular_color", "specular_roughness_anisotropy", "specular_rotation"],
    ),
    ("appearance.graph.group_coat", &["coat_weight", "coat_roughness", "coat_ior", "coat_color", "geometry_coat_normal"]),
    ("appearance.graph.group_transmission", &["transmission_weight", "transmission_color", "transmission_depth_mm"]),
    ("appearance.graph.group_emission", &["emission_luminance", "emission_color"]),
    ("appearance.graph.group_geometry", &["geometry_normal", "geometry_tangent", "geometry_opacity"]),
];

/// What drives an input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drive<'a> {
    /// Nothing: the catalogue default.
    Default,
    Constant(&'a Constant),
    Interface(&'a str),
    Wired(&'a Connection),
}

pub fn drive<'a>(n: &'a Node, port: &str) -> Drive<'a> {
    match n.inputs.get(port) {
        None => Drive::Default,
        Some(InputValue::Constant(c)) => Drive::Constant(c),
        Some(InputValue::Interface(i)) => Drive::Interface(&i.interface),
        Some(InputValue::Connection(c)) => Drive::Wired(c),
    }
}

/// The nodes upstream of `from`, nearest first, each once; surfaces are never part of a chain.
pub fn upstream<'a>(graph: &'a MaterialGraph, from: &str) -> Vec<&'a Node> {
    let mut out: Vec<&Node> = Vec::new();
    let mut queue = VecDeque::from([from.to_string()]);
    while let Some(id) = queue.pop_front() {
        if out.len() >= graph.nodes.len() {
            break;
        }
        let Some(n) = node(graph, &id) else { continue };
        if out.iter().any(|o| o.id == n.id) || matches!(n.kind.as_str(), "standard_surface" | "surface.mix") {
            continue;
        }
        out.push(n);
        for input in n.inputs.values() {
            if let InputValue::Connection(c) = input {
                queue.push_back(c.source().0.to_string());
            }
        }
    }
    out
}

/// How many inputs across the graph read `id`'s outputs.
pub fn readers(graph: &MaterialGraph, id: &str) -> usize {
    graph.nodes.iter().flat_map(|n| n.inputs.values()).filter(|i| matches!(i, InputValue::Connection(c) if c.source().0 == id)).count()
}

/// Every node the output reads, directly or through others.
pub fn reachable(graph: &MaterialGraph) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([graph.output.clone()]);
    while let Some(id) = queue.pop_front() {
        if seen.len() > graph.nodes.len() || !seen.insert(id.clone()) {
            continue;
        }
        if let Some(n) = node(graph, &id) {
            for input in n.inputs.values() {
                if let InputValue::Connection(c) = input {
                    queue.push_back(c.source().0.to_string());
                }
            }
        }
    }
    seen.retain(|id| node(graph, id).is_some());
    seen
}

// ---------------------------------------------------------------------------------------------
// Values.

/// A constant's components as linear floats (an sRGB8 colour decoded), or `None` for a texture.
pub fn components(c: &Constant) -> Option<Vec<f64>> {
    match c {
        Constant::Number(x) => Some(vec![*x]),
        Constant::Vector(v) => Some(v.iter().take(3).copied().collect()),
        Constant::Srgb8(s) => Some(s.srgb8.iter().map(|b| solidcraft_engine::render::surface::srgb8_to_linear(*b)).collect()),
        Constant::Texture(_) => None,
    }
}

/// An input's value for editing: its constant, else the catalogue default, as `width` floats
/// (a single float broadcasts).
pub fn value_of(c: Option<&Constant>, spec: &PortSpec, width: usize) -> Vec<f64> {
    let from_default = || match spec.default {
        PortDefault::Float(x) => vec![x],
        PortDefault::Vec2(v) => v.to_vec(),
        PortDefault::Vec3(v) => v.to_vec(),
        _ => vec![0.0],
    };
    let mut v = c.and_then(components).filter(|v| !v.is_empty() && v.iter().all(|x| x.is_finite())).unwrap_or_else(from_default);
    if v.len() == 1 && width > 1 {
        v = vec![v.first().copied().unwrap_or(0.0); width];
    }
    v.resize(width, 0.0);
    v
}

/// The constant to send for `values` (one float, or a 2- or 3-vector), clamped to the port's range.
pub fn constant(values: &[f64], range: Option<(f64, f64)>) -> Constant {
    let clamp = |x: f64| {
        let x = if x.is_finite() { x } else { 0.0 };
        range.map_or(x, |(lo, hi)| x.clamp(lo.min(hi), hi.max(lo)))
    };
    match values {
        [x] => Constant::Number(clamp(*x)),
        v => Constant::Vector(v.iter().map(|x| clamp(*x)).collect()),
    }
}

/// How many float components a port of `ty` edits (0: not a numeric field).
pub fn width(ty: PortType) -> usize {
    match ty {
        PortType::Float | PortType::Int | PortType::Numeric => 1,
        PortType::Vector2 => 2,
        PortType::Color3 | PortType::Vector3 => 3,
        PortType::Texcoord | PortType::Normal | PortType::Texture | PortType::Surface => 0,
    }
}

/// The unit a port's value is shown in: the catalogue's naming (`_mm`, `_per_mm`) and its angle
/// inputs (radians in the file, degrees on screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    Mm,
    PerMm,
    Degrees,
}

pub fn unit(port: &str) -> Unit {
    if port.ends_with("_per_mm") {
        Unit::PerMm
    } else if port.ends_with("_mm") {
        Unit::Mm
    } else if matches!(port, "rotation" | "specular_rotation" | "hue") {
        Unit::Degrees
    } else {
        Unit::None
    }
}

// ---------------------------------------------------------------------------------------------
// Edits by value: `appearance.graph.edit` ops.

pub fn set_input(node: &str, port: &str, value: &Constant) -> Value {
    json!({"op": "set_input", "node": node, "port": port, "value": value})
}

pub fn set_param(node: &str, param: &str, value: &ParamValue) -> Value {
    json!({"op": "set_param", "node": node, "param": param, "value": value})
}

pub fn set_interface(input: &InterfaceInput) -> Value {
    json!({"op": "set_interface", "input": input})
}

pub fn edit(id: &str, ops: Vec<Value>) -> Value {
    json!({"id": id, "ops": ops})
}

// ---------------------------------------------------------------------------------------------
// Edits of structure: a new whole graph for `appearance.graph.set`.

/// A pattern the editor can drive an input with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Noise,
    Brushed,
    Weave,
    Checker,
    Grid,
    Gradient,
    Image,
}

impl Driver {
    pub const ALL: [Driver; 7] = [Driver::Noise, Driver::Brushed, Driver::Weave, Driver::Checker, Driver::Grid, Driver::Gradient, Driver::Image];
    pub fn kind(self) -> &'static str {
        match self {
            Driver::Noise => "pattern.noise",
            Driver::Brushed => "pattern.brushed",
            Driver::Weave => "pattern.weave",
            Driver::Checker => "pattern.checker",
            Driver::Grid => "pattern.grid",
            Driver::Gradient => "pattern.gradient",
            Driver::Image => "image",
        }
    }
}

/// A node id not yet in `graph`, made from `base` (`[a-z0-9_-]`, no dots: a dot separates an
/// output name in a connection).
pub fn fresh_id(graph: &MaterialGraph, base: &str) -> String {
    let mut stem: String = base
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' { c } else { '_' })
        .collect();
    stem.truncate(limits::ID_BYTES - 6);
    if stem.is_empty() {
        stem.push('n');
    }
    if node(graph, &stem).is_none() {
        return stem;
    }
    (2..=limits::NODES * 4).map(|i| format!("{stem}_{i}")).find(|id| node(graph, id).is_none()).unwrap_or(stem)
}

fn add(graph: &mut MaterialGraph, base: &str, kind: &str) -> String {
    let id = fresh_id(graph, base);
    graph.nodes.push(Node { id: id.clone(), kind: kind.into(), params: Default::default(), inputs: Default::default() });
    id
}

fn wire(graph: &mut MaterialGraph, node: &str, port: &str, from: &str, fallback: Option<Constant>) {
    if let Some(n) = node_mut(graph, node) {
        n.inputs.insert(port.into(), InputValue::Connection(Connection { from: from.into(), fallback }));
    }
}

fn set_constant(graph: &mut MaterialGraph, node: &str, port: &str, value: Constant) {
    if let Some(n) = node_mut(graph, node) {
        n.inputs.insert(port.into(), InputValue::Constant(value));
    }
}

/// Remove the nodes `before` reached that `graph` no longer reaches, and their layout entries.
fn prune(graph: &mut MaterialGraph, before: &BTreeSet<String>) {
    let now = reachable(graph);
    let dropped: BTreeSet<String> = before.iter().filter(|id| !now.contains(*id)).cloned().collect();
    graph.nodes.retain(|n| !dropped.contains(&n.id));
    graph.layout.retain(|id, _| !dropped.contains(id));
}

fn bounded(graph: &MaterialGraph) -> Result<(), &'static str> {
    if graph.nodes.len() > limits::NODES { Err("appearance.graph.too_many_nodes") } else { Ok(()) }
}

/// Lay a new surface over the stack, masked by a constant half mix. The new surface starts as
/// a copy of the top surface's constants.
pub fn add_layer(graph: &MaterialGraph, stack: &Stack) -> Result<MaterialGraph, &'static str> {
    let mut g = graph.clone();
    let top = stack.layers.last().and_then(|l| node(graph, &l.surface)).ok_or("appearance.graph.not_stack_output")?;
    let inputs = top.inputs.iter().filter(|(_, v)| matches!(v, InputValue::Constant(_))).map(|(k, v)| (k.clone(), v.clone())).collect();
    let surface = fresh_id(&g, "layer");
    g.nodes.push(Node { id: surface.clone(), kind: "standard_surface".into(), params: Default::default(), inputs });
    let mix = add(&mut g, &format!("{surface}_mix"), "surface.mix");
    let below = g.output.clone();
    wire(&mut g, &mix, "fg", &surface, None);
    wire(&mut g, &mix, "bg", &below, None);
    set_constant(&mut g, &mix, "mix", Constant::Number(0.5));
    g.output = mix;
    bounded(&g)?;
    Ok(g)
}

/// Remove layer `index` (not the base) with its mask and whatever only it used.
pub fn remove_layer(graph: &MaterialGraph, stack: &Stack, index: usize) -> Result<MaterialGraph, &'static str> {
    let layer = stack.layers.get(index).ok_or("appearance.graph.no_layer")?;
    let mix_id = layer.mix.as_deref().ok_or("appearance.graph.base_layer")?;
    let mix = node(graph, mix_id).ok_or("appearance.graph.no_layer")?;
    let Some(InputValue::Connection(below)) = mix.inputs.get("bg") else { return Err("appearance.graph.not_stack_mix") };
    let below = below.clone();
    let before = reachable(graph);
    let mut g = graph.clone();
    match stack.layers.get(index + 1).and_then(|l| l.mix.clone()) {
        Some(above) => {
            if let Some(n) = node_mut(&mut g, &above) {
                n.inputs.insert("bg".into(), InputValue::Connection(below));
            }
        }
        None => g.output = below.source().0.to_string(),
    }
    prune(&mut g, &before);
    Ok(g)
}

/// Swap masked layers `index` and `index + 1` (each with its own mask).
pub fn swap_layers(graph: &MaterialGraph, stack: &Stack, index: usize) -> Result<MaterialGraph, &'static str> {
    let (Some(lower), Some(upper)) = (stack.layers.get(index), stack.layers.get(index + 1)) else { return Err("appearance.graph.no_layer") };
    let (Some(lower_mix), Some(upper_mix)) = (lower.mix.as_deref(), upper.mix.as_deref()) else { return Err("appearance.graph.base_layer") };
    let take = |id: &str, port: &str| node(graph, id).and_then(|n| n.inputs.get(port).cloned());
    let (lower_fg, lower_mask) = (take(lower_mix, "fg"), take(lower_mix, "mix"));
    let (upper_fg, upper_mask) = (take(upper_mix, "fg"), take(upper_mix, "mix"));
    let mut g = graph.clone();
    for (id, fg, mask) in [(lower_mix, upper_fg, upper_mask), (upper_mix, lower_fg, lower_mask)] {
        if let Some(n) = node_mut(&mut g, id) {
            for (port, value) in [("fg", fg), ("mix", mask)] {
                match value {
                    Some(v) => n.inputs.insert(port.into(), v),
                    None => n.inputs.remove(port),
                };
            }
        }
    }
    Ok(g)
}

/// The constant an input shows: its own, else its connection's fallback.
pub fn shown_constant<'a>(n: &'a Node, port: &str) -> Option<&'a Constant> {
    match n.inputs.get(port)? {
        InputValue::Constant(c) => Some(c),
        InputValue::Connection(c) => c.fallback.as_ref(),
        InputValue::Interface(_) => None,
    }
}

/// The first projection in the graph whose `texcoord` output a pattern can read.
fn projection(graph: &MaterialGraph) -> Option<String> {
    graph.nodes.iter().find(|n| matches!(n.kind.as_str(), "texcoord.planar" | "texcoord.triplanar" | "texcoord.cylindrical")).map(|n| n.id.clone())
}

/// Drive `node`'s `port` with `driver`: a projection (the graph's first, or a new planar one),
/// the pattern and, for numeric inputs, a remap whose range ends at the input's present value.
/// The connection keeps that value as its fallback, so the base response does not change.
/// Normal inputs get a relief from the pattern; `texture` names an image's file.
pub fn drive_with(
    graph: &MaterialGraph,
    catalogue: &Catalogue,
    node_id: &str,
    port: &str,
    driver: Driver,
    texture: Option<&str>,
) -> Result<MaterialGraph, &'static str> {
    let n = node(graph, node_id).ok_or("appearance.graph.no_node")?;
    let spec = catalogue.port(&n.kind, port).ok_or("appearance.graph.no_port")?;
    let present = shown_constant(n, port).cloned();
    let before = reachable(graph);
    let mut g = graph.clone();
    let uv = match projection(&g) {
        Some(id) => id,
        None => add(&mut g, "uv", "texcoord.planar"),
    };
    let pattern = add(&mut g, &format!("{port}_{}", driver.kind().rsplit('.').next().unwrap_or("pattern")), driver.kind());
    wire(&mut g, &pattern, "texcoord", &uv, None);
    if driver == Driver::Image {
        match texture {
            Some(sha) => set_constant(
                &mut g,
                &pattern,
                "file",
                Constant::Texture(solidcraft_engine::render::material_graph::graph::TextureRef { texture: sha.into() }),
            ),
            None => return Err("appearance.graph.pick_texture"),
        }
    }
    match spec.ty {
        PortType::Normal => {
            let source = if driver == Driver::Image { "normal.map" } else { "normal.from_height" };
            let relief = add(&mut g, &format!("{port}_relief"), source);
            let (input, output) =
                if driver == Driver::Image { ("color", format!("{pattern}.color")) } else { ("height", format!("{pattern}.value")) };
            wire(&mut g, &relief, input, &output, None);
            wire(&mut g, node_id, port, &relief, None);
        }
        PortType::Vector3 => {
            // A tangent: the projection's own (a brushed pattern supplies the one it runs along).
            let from = if driver == Driver::Brushed { format!("{pattern}.tangent") } else { format!("{uv}.tangent") };
            wire(&mut g, node_id, port, &from, None);
        }
        PortType::Color3 if driver == Driver::Image => wire(&mut g, node_id, port, &format!("{pattern}.color"), present),
        PortType::Float | PortType::Color3 => {
            let w = width(spec.ty);
            let high = value_of(present.as_ref(), spec, w);
            let low: Vec<f64> = high.iter().map(|x| x * 0.8).collect();
            let output = format!("{pattern}.value");
            let remap = add(&mut g, &format!("{port}_range"), "remap");
            wire(&mut g, &remap, "in", &output, None);
            set_constant(&mut g, &remap, "out_min", constant(&low, spec.range));
            set_constant(&mut g, &remap, "out_max", constant(&high, spec.range));
            wire(&mut g, node_id, port, &remap, present.or_else(|| Some(constant(&high, spec.range))));
        }
        _ => return Err("appearance.graph.cannot_drive"),
    }
    // A projection the graph had but no longer used stays; one this edit added is used.
    prune(&mut g, &before);
    bounded(&g)?;
    Ok(g)
}

/// Make `node`'s `port` a constant again (`value`), removing the nodes only its driver used.
pub fn undrive(graph: &MaterialGraph, node_id: &str, port: &str, value: Option<Constant>) -> Result<MaterialGraph, &'static str> {
    node(graph, node_id).ok_or("appearance.graph.no_node")?;
    let before = reachable(graph);
    let mut g = graph.clone();
    if let Some(n) = node_mut(&mut g, node_id) {
        match value {
            Some(v) => n.inputs.insert(port.into(), InputValue::Constant(v)),
            None => n.inputs.remove(port),
        };
    }
    prune(&mut g, &before);
    Ok(g)
}

/// The `appearance.graph.set` parameters for `graph`.
pub fn set(id: &str, graph: &MaterialGraph) -> Value {
    json!({"id": id, "graph": graph})
}

/// A param's current value, else its default.
pub fn param_value(n: &Node, spec: &ParamSpec) -> ParamValue {
    match (n.params.get(&spec.name), &spec.kind) {
        (Some(v), _) => v.clone(),
        (None, ParamKind::Choice { default, .. }) => ParamValue::Text(default.clone()),
        (None, ParamKind::Int { default, .. }) => ParamValue::Int(*default),
    }
}

/// How much of the budget a value is, for the cost meter.
pub fn fraction(used: u32, budget: u32) -> f32 {
    if budget == 0 { 1.0 } else { (f64::from(used) / f64::from(budget)).clamp(0.0, 1.0) as f32 }
}
