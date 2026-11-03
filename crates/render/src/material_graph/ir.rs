//! Validation and the lowered form both GPU code generators read (docs/material-graph.md §5.1,
//! §5.7). One front end, two back ends: the viewport (uniform pages, WebGL2-safe) and the path
//! tracer (a storage-buffer material table, native only).
//!
//! Contract status: the types below are fixed; [`validate`], [`lower`] and [`base_response`]
//! refuse with [`GraphError::NotBuilt`] until track A lands them.

use std::fmt;

use serde::Serialize;

use super::catalogue::PortType;
use super::graph::MaterialGraph;
use crate::surface::OpaqueSurface;

/// One problem with a graph, located where possible. `code` is stable, `message` is for people.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GraphIssue {
    /// The node id, or `None` for the graph as a whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The input, param or output name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    pub code: &'static str,
    pub message: String,
}

/// Why a graph cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// The graph breaks the catalogue or the limits; every issue found.
    Invalid(Vec<GraphIssue>),
    /// The step is part of the contract but its implementation has not landed yet.
    NotBuilt(&'static str),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::Invalid(issues) => {
                let first = issues.first().map(|i| i.message.as_str()).unwrap_or("invalid material graph");
                if issues.len() > 1 { write!(f, "{first} (and {} more)", issues.len() - 1) } else { f.write_str(first) }
            }
            GraphError::NotBuilt(what) => write!(f, "not built yet: {what}"),
        }
    }
}

impl std::error::Error for GraphError {}

/// A graph that passed validation. Only [`validate`] makes one, so holding one proves it.
#[derive(Clone, Debug, PartialEq)]
pub struct Validated {
    graph: MaterialGraph,
}

impl Validated {
    pub fn graph(&self) -> &MaterialGraph {
        &self.graph
    }
}

/// Everything about a graph that changes generated code, hashed: node kinds, params, topology,
/// which inputs are connected and which are constants, texture colour spaces. Values never.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ShapeKey(pub [u8; 16]);

/// An SSA value in the IR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ValueId(pub u32);

/// Where an operand comes from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operand {
    /// An earlier instruction's result.
    Value(ValueId),
    /// A constant or interface value: `width` floats at `offset` in the material's uniform
    /// slots (vec4-aligned per value).
    Uniform { offset: u32, width: u8 },
    /// A texture binding, by its index in [`Ir::textures`].
    Texture(u8),
    /// Supplied by the geometry: position, normal or the tangent of the active texcoord.
    Geometry(GeometryInput),
}

/// Inputs the geometry supplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryInput {
    ObjectPosition,
    WorldPosition,
    ObjectNormal,
    WorldNormal,
    /// |n·v|.
    Facing,
}

/// One instruction: a node output, with its resolved type.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Instr {
    pub result: ValueId,
    /// The catalogue kind (`pattern.noise`) or, for an output selection, `kind.output`.
    pub op: &'static str,
    /// The resolved type (never [`PortType::Numeric`]).
    pub ty: PortType,
    /// Compile-time params, in catalogue order (choice index or integer).
    pub params: Vec<i64>,
    /// Operands in the catalogue's input order.
    pub args: Vec<Operand>,
    /// Whether the value carries its slope (it reaches a normal input).
    pub slope: bool,
    /// Expanded three times for a triplanar texcoord.
    pub triplanar: bool,
}

/// A texture the material reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextureBinding {
    /// SHA-256 of the asset, or the interface input that supplies it.
    pub source: String,
    /// The `image` node's colour space, part of the shape.
    pub srgb: bool,
}

/// A validated graph lowered for code generation: straight-line SSA in topological order,
/// ending in the surface node's operands.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ir {
    pub instrs: Vec<Instr>,
    /// The surface node's inputs, by catalogue name, in catalogue order.
    pub surface: Vec<(&'static str, Operand)>,
    /// For a `surface.mix` output: the blend factor and the two surfaces' operands (v1 blends
    /// parameters, decision O3). Empty otherwise.
    pub mix_layers: Vec<MixLayer>,
    /// Floats of uniform data one material of this graph needs (≤ `limits::UNIFORM_FLOATS`).
    pub uniform_floats: u32,
    pub textures: Vec<TextureBinding>,
    /// Cost units after expansion (≤ `limits::COST_UNITS`).
    pub cost: u32,
    /// Texture samples after expansion (≤ `limits::TEXTURE_SAMPLES`).
    pub samples: u32,
    /// Whether the generated code reads the object-space position (a 40-byte vertex).
    pub object_space: bool,
    pub shape: ShapeKey,
}

/// One parameter-blended layer of a `surface.mix` chain.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MixLayer {
    pub mix: Operand,
    pub fg: Vec<(&'static str, Operand)>,
}

/// Check a graph against the catalogue and the limits: known kinds, ports and params, typed
/// connections, no cycles, constants in range, one surface output, cost and texture budgets.
/// Never panics on hostile input.
pub fn validate(graph: &MaterialGraph) -> Result<Validated, GraphError> {
    let _ = graph;
    Err(GraphError::NotBuilt("material graph validation (track A)"))
}

/// Lower a validated graph to the IR both code generators read. `uniform_floats` and the slot
/// offsets are assigned here, so the viewport and the path tracer lay out values identically.
pub fn lower(graph: &Validated) -> Result<Ir, GraphError> {
    let _ = graph;
    Err(GraphError::NotBuilt("material graph lowering (track A)"))
}

/// The base response: a static read of the output node, constant inputs as they are and driven
/// inputs at their `fallback` (or default). No evaluation. It gives a look's display colour and
/// the drawing used while a shape compiles. For `surface.mix`, the `bg` surface's response.
pub fn base_response(graph: &Validated) -> Result<OpaqueSurface, GraphError> {
    let _ = graph;
    Err(GraphError::NotBuilt("material graph base response (track A)"))
}
