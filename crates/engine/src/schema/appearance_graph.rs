//! Contracts and result types for the commands in `cmd/appearance_graph.rs`: material graphs
//! and their texture assets (docs/material-graph.md §7.2).

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A material graph in its saved form (docs/material-graph.md §4.1): `name`, `nodes`, `output`,
/// and optionally `interface` and `source`. `appearance.graph.nodes` lists the node kinds.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct GraphValue(pub Value);

impl JsonSchema for GraphValue {
    fn schema_name() -> Cow<'static, str> {
        "MaterialGraph".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "A material graph: name, nodes [{id, kind, params?, inputs?}], output (the surface node's id), interface?, source?. Node kinds, ports and ranges: appearance.graph.nodes.",
            "required": ["name", "nodes", "output"],
            "properties": {
                "name": {"type": "string"},
                "nodes": {"type": "array", "maxItems": 128, "items": {"type": "object", "required": ["id", "kind"]}},
                "output": {"type": "string"},
                "interface": {"type": "array", "maxItems": 32},
                "source": {"type": "object"}
            }
        })
    }
}

/// List the node catalogue: every node kind with its typed inputs (ranges, defaults), params,
/// outputs and cost units. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphNodesParams {}

/// What `appearance.graph.nodes` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphNodesResult {
    /// The catalogue's version; a new node kind or port bumps it.
    pub version: u32,
    /// The node kinds, as `{kind, inputs, params, outputs, cost}`.
    pub nodes: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Check a graph without changing the design. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphValidateParams {
    pub graph: GraphValue,
}

/// One problem with a graph.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphIssueReply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    pub code: String,
    pub message: String,
}

/// What `appearance.graph.validate` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphValidateResult {
    pub ok: bool,
    /// Cost units after expansion, out of 2000.
    pub cost: u32,
    /// Texture samples after expansion, out of 8.
    pub samples: u32,
    /// Distinct textures read, out of 4.
    pub textures: u32,
    /// The shape key as hex: graphs with equal shapes share a GPU pipeline.
    pub shape: String,
    pub errors: Vec<GraphIssueReply>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Add a graph to the design's table.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphCreateParams {
    /// The new graph's id: 1–64 of a–z, 0–9, `_ . -`. Default: made from `name`.
    #[serde(default)]
    pub id: Option<String>,
    /// The graph's name, 1–256 bytes.
    pub name: String,
    /// The graph. Default: one `standard_surface` with its defaults. Its `name` is replaced by
    /// `name`.
    #[serde(default)]
    pub graph: Option<GraphValue>,
    /// Instead of `graph`: `"standard"` (the default graph) or a saved look
    /// `{name, color, opacity?, pbr?}` (as `appearance.list` gives it), giving the one-node graph
    /// that draws that look.
    #[serde(default)]
    pub from: Option<GraphFrom>,
}

/// What `appearance.graph.create` starts from.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct GraphFrom(pub Value);

impl JsonSchema for GraphFrom {
    fn schema_name() -> Cow<'static, str> {
        "MaterialGraphFrom".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "\"standard\" (one standard_surface with its defaults) or a saved look {name, color, opacity?, pbr?} to express as a one-node graph",
            "oneOf": [
                {"type": "string", "enum": ["standard"]},
                {"type": "object", "required": ["color"], "properties": {
                    "name": {"type": "string"}, "color": {}, "opacity": {"type": "number"}, "pbr": {}
                }}
            ]
        })
    }
}

/// Convert a scalar or `pbr` look to a material graph ("Convert to material graph"): a new graph
/// that draws the look as it is, and every override that is exactly that look (within `scope`)
/// then names the graph. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphConvertParams {
    /// The look, as `appearance.list` gives it: `{name, color, opacity, pbr?}`.
    pub look: Value,
    /// The new graph's id. Default: made from `name`.
    #[serde(default)]
    pub id: Option<String>,
    /// The graph's name. Default: the look's name.
    #[serde(default)]
    pub name: Option<String>,
    /// `{bodies?: [names], components?: [names or ids]}`, as `appearance.replace` takes it.
    /// Default: every override with that look. Physical materials' looks are not rewritten.
    #[serde(default)]
    pub scope: Option<Value>,
}

/// What `appearance.graph.convert` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphConverted {
    pub id: String,
    pub cost: u32,
    /// Overrides that now name the graph.
    pub count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Replace a graph in the design's table as a whole.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphSetParams {
    pub id: String,
    pub graph: GraphValue,
}

/// One edit of `appearance.graph.edit`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphEditOp {
    /// Add a node with its defaults.
    AddNode { node: String, kind: String },
    /// Remove a node and every connection to or from it.
    RemoveNode { node: String },
    /// Connect `from` (`node` or `node.output`) to `node`'s `port`. `fallback` is the constant
    /// the base response (the standard view's colour) reads in its place; omitted, the port's
    /// default.
    Connect {
        from: String,
        node: String,
        port: String,
        #[serde(default)]
        fallback: Option<Value>,
    },
    /// Drop the connection into `node`'s `port`, which returns to its default.
    Disconnect { node: String, port: String },
    /// Set `node`'s `port` to a constant (a number, a 2- or 3-vector, `{"srgb8": [r,g,b]}` or
    /// `{"texture": sha256}`), or bind it to an interface input (`{"interface": name}`).
    SetInput { node: String, port: String, value: Value },
    /// Set a compile-time param (an option name or an integer).
    SetParam { node: String, param: String, value: Value },
    /// Add or replace an interface input (`{name, type, default?}`).
    SetInterface { input: Value },
}

/// Apply edits to one graph: all apply, or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphEditParams {
    pub id: String,
    /// At most 512 edits.
    pub ops: Vec<GraphEditOp>,
}

/// What a command that changes a graph returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphChanged {
    pub id: String,
    /// Cost units after expansion.
    pub cost: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Read one graph. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphGetParams {
    pub id: String,
}

/// What `appearance.graph.get` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphGetResult {
    pub id: String,
    pub graph: Value,
    pub cost: u32,
    /// How many looks use it.
    pub uses: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// List the design's graphs. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphListParams {}

/// One graph of `appearance.graph.list`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphListEntry {
    pub id: String,
    pub name: String,
    pub uses: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
}

/// What `appearance.graph.list` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphListResult {
    pub graphs: Vec<GraphListEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Copy a graph under a new id.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphDuplicateParams {
    pub id: String,
    /// Default: made from the new name.
    #[serde(default)]
    pub new_id: Option<String>,
    /// Default: the source's name with " copy".
    #[serde(default)]
    pub name: Option<String>,
}

/// Remove a graph. Refused while a look uses it, unless `replace_with` names the graph those
/// looks use instead.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphDeleteParams {
    pub id: String,
    #[serde(default)]
    pub replace_with: Option<String>,
}

/// What `appearance.graph.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphDeleteResult {
    /// Looks moved to `replace_with`.
    pub replaced: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Update a library copy from its library. With `preview`, only report what would change.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphUpdateFromLibraryParams {
    pub id: String,
    /// The library file the graph's `source.library` names.
    pub library: String,
    #[serde(default)]
    pub preview: bool,
}

/// What `appearance.graph.update_from_library` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphUpdateResult {
    /// Whether the design changed (false for a preview).
    pub applied: bool,
    /// The inputs, params and nodes that differ.
    pub changes: Vec<String>,
    /// Looks the update affects.
    pub looks: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Add a PNG or JPEG texture asset (at most 16384 px a side and 256 MiB; kept at full quality) to the design.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextureAddParams {
    /// A file to read. Give this or `data_base64`.
    #[serde(default)]
    pub path: Option<String>,
    /// The file's bytes, base64.
    #[serde(default)]
    pub data_base64: Option<String>,
    /// Default: the file name.
    #[serde(default)]
    pub name: Option<String>,
}

/// What `appearance.texture.add` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TextureAddResult {
    /// The asset's id: the SHA-256 of its bytes.
    pub texture: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// List the design's texture assets. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextureListParams {}

/// One texture of `appearance.texture.list`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TextureEntry {
    pub texture: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub uses: u32,
}

/// What `appearance.texture.list` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TextureListResult {
    pub textures: Vec<TextureEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "appearance_graph_tests.rs"]
mod tests;
