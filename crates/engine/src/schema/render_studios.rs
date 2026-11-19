//! Contracts and results of the studio library commands in `cmd/render/studios.rs`
//! (`render.studio.*`). The handlers read these same parameter types; like the handlers, a contract
//! refuses unknown keys. A studio's own fields (`realistic`, `path`) are checked by the studio
//! record itself (`solidcraft_viewport::studio::Studio::validate`), with its own messages.

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A studio record, `render.studio.get`'s `studio`: `{id, name, description?, source?,
/// realistic?, path?}`.
fn studio_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "description": "A studio: `id`, `name`, `description`, `source`, and its rig (`realistic`: `lights` (1 to 4, the key \
            first, each `{id, centre, width_axis, height_axis, half_width, half_height, radiance, grade}` in subject extents B \
            about the studio's centre), `rig_lights`, `cyclorama` `{floor, wall, band, ceiling, spill}`, `room` \
            `{half, floor, ceiling}`, `white_under_key` and `acceptance` `{dark_albedo, highlight, distinct, narrow_face}`) \
            and/or its composition (`path`: `{hdr, intensity, exposure_ev, ground, rig}`).",
        "type": "object",
    })
}

fn rig_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "description": "The studio's rig, replaced whole: `{lights, rig_lights, cyclorama, room, white_under_key, acceptance}` \
            (see `render.studio.get`), or null for none.",
        "type": ["object", "null"],
    })
}

fn composition_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "description": "The studio's composition for a saved path-traced setup, replaced whole: `{hdr, intensity, exposure_ev, \
            ground, rig}`, or null for none.",
        "type": ["object", "null"],
    })
}

fn document_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "description": "A `solidcraft-studios/1` document: `{schema: \"solidcraft-studios/1\", items: [studio, …]}`.",
        "type": ["object", "null"],
        "required": ["schema", "items"],
    })
}

/// List the studio library: built-in studios first, then the user's.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioListParams {
    /// Only studios with a rig the Realistic view and the path tracer light with (`realistic`),
    /// or with a composition for `render.preset.*` (`path`). Default: all.
    #[serde(default)]
    #[schemars(regex(pattern = "^(realistic|path|all)$"))]
    pub kind: Option<String>,
}

/// One studio of the list.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioSummary {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Shipped with the app: read-only; duplicate it to change a copy.
    pub builtin: bool,
    /// Has a rig: usable by the Realistic view and as a path job's `studio`.
    pub realistic: bool,
    /// Has a composition for `render.preset.*`.
    pub path: bool,
}

/// What `render.studio.list` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioListResult {
    pub studios: Vec<StudioSummary>,
    /// The studio the Realistic view lights with now.
    pub realistic_default: String,
}

/// Read one studio whole.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioGetParams {
    pub id: String,
}

/// What `render.studio.get`, `duplicate` and `edit` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioGetResult {
    pub builtin: bool,
    #[schemars(schema_with = "studio_schema")]
    pub studio: Value,
}

/// Copy a studio (built-in or the user's) as a new user studio.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioDuplicateParams {
    pub id: String,
    /// The copy's id: 1 to 128 ASCII letters, digits, hyphens or underscores. Default: the next
    /// free `USER-STUDIO-<n>`.
    #[serde(default)]
    pub new_id: Option<String>,
    /// Default: "<name> copy".
    #[serde(default)]
    pub name: Option<String>,
}

/// Change a user studio. Each field given replaces the studio's; the result is checked whole, and
/// a refused edit changes nothing. Built-in studios are refused.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioEditParams {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "rig_schema")]
    pub realistic: Option<Value>,
    #[serde(default)]
    #[schemars(schema_with = "composition_schema")]
    pub path: Option<Value>,
}

/// Delete a user studio. Built-in studios are refused. When it was the Realistic default, the
/// default returns to Product.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioDeleteParams {
    pub id: String,
}

/// What `render.studio.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioDeleted {
    pub deleted: String,
    pub realistic_default: String,
}

/// Add the studios of a `solidcraft-studios/1` file (or document) to the user's. Checked whole
/// first: an import that fails adds nothing.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioImportParams {
    /// A `.json` studio file.
    #[serde(default)]
    pub path: Option<String>,
    /// The document itself, in place of `path`.
    #[serde(default)]
    #[schemars(schema_with = "document_schema")]
    pub document: Option<Value>,
    /// Replace user studios of the same id. Default: false, such an import is refused.
    #[serde(default)]
    pub replace: bool,
}

/// What `render.studio.import` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioImported {
    pub imported: Vec<String>,
}

/// Write studios (built-in or the user's) to a `solidcraft-studios/1` file.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioExportParams {
    /// A path ending in `.json`.
    pub path: String,
    #[schemars(length(min = 1, max = 256))]
    pub ids: Vec<String>,
    #[serde(default)]
    pub overwrite: bool,
}

/// What `render.studio.export` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioExported {
    pub path: String,
    pub count: usize,
    pub bytes: usize,
}

/// Set or read the studio the Realistic view lights with (a setting, not part of the design).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StudioDefaultParams {
    /// A studio with a rig. Without it the command only reports the default.
    #[serde(default)]
    pub id: Option<String>,
}

/// What `render.studio.default` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StudioDefaultResult {
    pub id: String,
}
