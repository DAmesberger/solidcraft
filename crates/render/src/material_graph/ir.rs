//! Validation and the lowered form both GPU code generators read (docs/material-graph.md §5.1,
//! §5.7). One front end, two back ends: the viewport (uniform pages, WebGL2-safe) and the path
//! tracer (a storage-buffer material table, native only).
//!
//! [`validate`] checks a graph, [`lower`] builds the IR and [`base_response`] reads the output
//! node's constants; the work lives in `validate.rs`, `lower.rs` and `base.rs`. Nothing here
//! evaluates a graph.
//!
//! IR conventions the back ends rely on:
//! - `instrs` are in a canonical order (a post-order walk from the surface operands, inputs in
//!   catalogue order), independent of node ids and their order in the file; unreachable nodes
//!   are left out.
//! - A call's value is its node's **first** output. A multi-output node's other outputs are
//!   selection instructions (`op` = `kind.output`, see [`Instr::kind`]) whose one argument is
//!   the call. The back end decides how a call returns several outputs (a struct).
//! - `geometry.*` nodes are not instructions: their outputs are [`Operand::Geometry`].
//! - Surface nodes are not instructions: [`Ir::surface`] and [`Ir::mix_layers`] hold their
//!   operands. Every constant, default and interface input is a uniform slot, so a value never
//!   changes the shape; [`Ir::slots`] says what each slot holds and [`uniform_words`] fills them.
//! - An operand narrower than its input (a float into a colour) broadcasts.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

use super::catalogue::PortType;
use super::graph::{Constant, MaterialGraph};
use super::sample::SurfaceSample;
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
    pub(super) fn new(graph: MaterialGraph) -> Self {
        Validated { graph }
    }
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
    /// An unconnected optional input (a noise's `texcoord` or `position`); the node's params say
    /// which input it reads. (Added by track A.)
    Absent,
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
    /// The default of an unconnected `geometry_tangent`: a tangent the back end derives from the
    /// geometry (unit, perpendicular to the normal). (Added by track A.)
    Tangent,
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

impl Instr {
    /// The node kind (`separate3` for `separate3.y`).
    pub fn kind(&self) -> &'static str {
        super::lower::op_kind(self.op)
    }
    /// For an output selection, the output it selects (`y` for `separate3.y`).
    pub fn selects(&self) -> Option<&'static str> {
        super::lower::SELECTIONS.iter().find(|s| s.2 == self.op).map(|s| s.1)
    }
}

/// A texture the material reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextureBinding {
    /// SHA-256 of the asset, or the interface input that supplies it.
    pub source: String,
    /// The `image` node's colour space, part of the shape.
    pub srgb: bool,
}

impl TextureBinding {
    /// How `source` names an interface input rather than an asset hash.
    pub const INTERFACE_PREFIX: &'static str = "interface:";

    /// The asset's SHA-256, when a constant supplies the texture.
    pub fn sha256(&self) -> Option<&str> {
        (!self.source.starts_with(Self::INTERFACE_PREFIX)).then_some(self.source.as_str())
    }
    /// The interface input that supplies the texture (a look's value, else its default, else
    /// none: the `image` node's `default` colour).
    pub fn interface(&self) -> Option<&str> {
        self.source.strip_prefix(Self::INTERFACE_PREFIX)
    }
}

/// What one uniform slot holds. (Added by track A: the back ends need it to fill pages.)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UniformSlot {
    /// In floats from the material's first float; a vec2 starts on an even float and a vec3 on
    /// a vec4 boundary, so no value straddles a vec4.
    pub offset: u32,
    pub width: u8,
    /// `Float`, `Int` (a seed, written as its `u32` bits), `Color3`, `Vector2` or `Vector3`.
    #[serde(rename = "type")]
    pub ty: PortType,
    /// The constant, or the interface input's default, linear, `width` components.
    pub value: Vec<f64>,
    /// The interface input a look may override this slot with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// For an interface slot, the range every input it feeds admits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<(f64, f64)>,
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
    /// Every uniform slot, by offset. (Added by track A.)
    pub slots: Vec<UniformSlot>,
}

/// One parameter-blended layer of a `surface.mix` chain.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MixLayer {
    pub mix: Operand,
    pub fg: Vec<(&'static str, Operand)>,
}

/// Check a graph against the catalogue and the limits: known kinds, ports and params, typed
/// connections, no cycles, constants in range, one surface output, cost and texture budgets.
/// Never panics on hostile input. The validated copy drops layout entries for missing nodes.
pub fn validate(graph: &MaterialGraph) -> Result<Validated, GraphError> {
    super::validate::validate(graph).map_err(GraphError::Invalid)
}

/// Lower a validated graph to the IR both code generators read. `uniform_floats` and the slot
/// offsets are assigned here, so the viewport and the path tracer lay out values identically.
pub fn lower(graph: &Validated) -> Result<Ir, GraphError> {
    let analysis = super::validate::analyse(graph.graph()).map_err(GraphError::Invalid)?;
    super::lower::build(&analysis).map_err(GraphError::Invalid)
}

/// The base response: a static read of the output node, constant inputs as they are and driven
/// inputs at their `fallback` (or default). No evaluation. It gives a look's display colour and
/// the drawing used while a shape compiles. For `surface.mix`, the `bg` surface's response.
pub fn base_response(graph: &Validated) -> Result<OpaqueSurface, GraphError> {
    base_sample(graph).map(|s| s.opaque)
}

/// The base response with every `standard_surface` input, read the same way: what the
/// constant-graph pipeline draws while a shape compiles. (Added by track A.)
pub fn base_sample(graph: &Validated) -> Result<SurfaceSample, GraphError> {
    let analysis = super::validate::analyse(graph.graph()).map_err(GraphError::Invalid)?;
    super::base::sample(&analysis).map_err(GraphError::Invalid)
}

/// One material's uniform data: `ir.uniform_floats` 32-bit words (`f32` bits; seeds as `u32`),
/// with interface slots taken from `values` where given, else their defaults. Values are checked
/// for type and range; names that no slot reads (a texture, an unused input) are ignored.
/// (Added by track A.)
pub fn uniform_words(ir: &Ir, values: &BTreeMap<String, Constant>) -> Result<Vec<u32>, GraphError> {
    super::base::uniform_words(ir, values).map_err(GraphError::Invalid)
}
