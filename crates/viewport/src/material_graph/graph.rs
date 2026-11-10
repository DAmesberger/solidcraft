//! The saved form of a material graph, exactly as a design's `appearances.graphs` holds it
//! (docs/material-graph.md §4.1). Reading a graph only parses it; [`super::validate`] checks it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::catalogue::PortType;

/// A material graph: nodes, one output surface node and an optional interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialGraph {
    pub name: String,
    /// Inputs a look may override (`Look::values`), at most `limits::INTERFACE_INPUTS`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interface: Vec<InterfaceInput>,
    pub nodes: Vec<Node>,
    /// The id of the node whose `surface` output the graph shows.
    pub output: String,
    /// Where a library copy came from. Inert provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<GraphSource>,
    /// Editor-only node positions (node id → [x, y] in canvas units), for a later node canvas.
    /// Renderers, the IR and the shape key ignore it, so moving a node never changes a picture
    /// or recompiles anything. Validation bounds it (`limits::NODES` entries, finite,
    /// |x|, |y| ≤ `limits::LAYOUT_COORD`) and drops entries for nodes that do not exist.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub layout: BTreeMap<String, [f64; 2]>,
}

/// One interface input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceInput {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PortType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Constant>,
}

/// Generic provenance of a graph copied from a library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphSource {
    pub library: String,
    pub entry: String,
    pub revision: u64,
}

/// One node: a catalogue kind, its compile-time params and its inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputValue>,
}

/// A param's value: an option name or a small integer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Int(i64),
    Text(String),
}

/// What drives an input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    /// Another node's output: `{"from": "node"}` (its first output) or `{"from": "node.output"}`.
    Connection(Connection),
    /// An interface input: `{"interface": "name"}`.
    Interface(InterfaceRef),
    /// A constant.
    Constant(Constant),
}

/// A connection, with the constant non-evaluating consumers use in its place (the base
/// response, docs/material-graph.md §2). Without `fallback`, the input's default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Constant>,
}

/// A reference to an interface input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceRef {
    pub interface: String,
}

/// A constant value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Constant {
    Number(f64),
    /// Two or three components: a vector2, vector3 or linear colour.
    Vector(Vec<f64>),
    /// An exact sRGB8 colour, decoded as `srgb8_to_linear` decodes a look's colour.
    Srgb8(Srgb8),
    /// A texture asset by its SHA-256.
    Texture(TextureRef),
}

/// `{"srgb8": [r, g, b]}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Srgb8 {
    pub srgb8: [u8; 3],
}

/// `{"texture": "<sha256>"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureRef {
    pub texture: String,
}

impl Connection {
    /// The node id and output name (`None`: the node's first output).
    pub fn source(&self) -> (&str, Option<&str>) {
        match self.from.split_once('.') {
            Some((node, output)) if !node.is_empty() && !output.is_empty() && !output.contains('.') => (node, Some(output)),
            _ => (self.from.as_str(), None),
        }
    }
}
