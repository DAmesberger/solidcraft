//! Material graphs: a data-only description of a surface, made of typed nodes (projections,
//! patterns, maths) that feed one standard surface node. The node vocabulary follows the
//! MaterialX standard library and the surface inputs follow OpenPBR Surface (public
//! specifications; names only, no code). Design: `docs/material-graph.md`.
//!
//! This module is the contract the GPU back ends (viewport and path tracer), the design format,
//! the commands and the editor build against:
//!
//! - [`graph`]: the saved form, as the design file holds it;
//! - [`catalogue`]: every node kind with its typed ports, ranges, defaults and cost;
//! - [`ir`]: validation and the lowered, typed form the code generators read;
//! - [`sample`]: the surface node's output type, whose constant core is [`OpaqueSurface`].
//!
//! Nothing here shades: product code never evaluates a graph on the CPU. A CPU reference
//! interpreter exists only in tests, as an oracle for GPU frames.
//!
//! [`OpaqueSurface`]: crate::surface::OpaqueSurface

mod base;
pub mod catalogue;
pub mod graph;
pub mod ir;
mod lower;
/// The test-only f64 reference interpreter (decision O12): `cfg(test)` or the `test-reference`
/// feature, which only dev-dependencies may enable (`cargo xtask layers` checks).
#[cfg(any(test, feature = "test-reference"))]
pub mod reference;
pub mod sample;
mod validate;

pub use catalogue::{CATALOGUE, CATALOGUE_VERSION, InputDefault, InputSpec, NodeSpec, OutputSpec, ParamKind, ParamSpec, PortType, node_spec};
pub use graph::{Connection, Constant, GraphSource, InputValue, InterfaceInput, InterfaceRef, MaterialGraph, Node, ParamValue};
pub use ir::{GraphError, GraphIssue, Ir, ShapeKey, UniformSlot, Validated, base_response, base_sample, lower, uniform_words, validate};
pub use sample::SurfaceSample;

/// Bounds every graph is checked against when it is read. Hostile input: these keep the
/// validator, the code generators and the GPU bounded.
pub mod limits {
    /// Graphs one design may hold.
    pub const GRAPHS_PER_DESIGN: usize = 4096;
    /// Nodes in one graph.
    pub const NODES: usize = 128;
    /// Connections in one graph; each input has at most one.
    pub const CONNECTIONS: usize = 512;
    /// Interface inputs a graph exposes.
    pub const INTERFACE_INPUTS: usize = 32;
    /// Bytes in a node or interface id (`[a-z0-9_.-]`).
    pub const ID_BYTES: usize = 64;
    /// Bytes in a graph or texture name (UTF-8, no control characters).
    pub const NAME_BYTES: usize = 256;
    /// Cost units after triplanar and slope expansion (docs/material-graph.md §3.6).
    pub const COST_UNITS: u32 = 2000;
    /// Texture samples after expansion.
    pub const TEXTURE_SAMPLES: u32 = 8;
    /// Distinct textures one graph reads.
    pub const TEXTURES_PER_GRAPH: usize = 4;
    /// Uniform floats one material occupies (64 vec4).
    pub const UNIFORM_FLOATS: usize = 256;
    /// Pixels per texture side a file may hold: a safety bound, not a quality cap (the largest
    /// 2D texture any current GPU holds). Renderers choose the resident size (§4.3).
    pub const TEXTURE_SIDE: u32 = 16384;
    /// Encoded bytes of one texture asset (the container's per-part cap).
    pub const TEXTURE_BYTES: usize = 256 << 20;
    /// Texture assets one design may hold (hostile-input bound; the starter pack's 240 maps fit).
    pub const TEXTURES_PER_DESIGN: usize = 256;
    /// Decoded bytes of all of a design's textures together, checked from the headers first.
    pub const DECODED_TEXTURE_BYTES: usize = 1 << 30;
    /// Codec scratch allowed on top of an image's decoded size (`image::Limits::max_alloc`).
    pub const CODEC_SCRATCH_BYTES: usize = 64 << 20;
    /// Octaves of a noise node (unrolled in generated code).
    pub const NOISE_OCTAVES: i64 = 8;
    /// Largest editor-layout coordinate.
    pub const LAYOUT_COORD: f64 = 1e6;
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod validate_tests;
