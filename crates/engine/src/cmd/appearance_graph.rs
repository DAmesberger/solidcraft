//! Material graphs and their texture assets (docs/material-graph.md §7.2). Contract status:
//! the ids, parameters and replies are fixed. `appearance.graph.nodes` runs; every other
//! command checks its parameters and then refuses with "not built yet" (track D builds them on
//! track A's validation and track B's records), leaving the design unchanged.
use super::CommandSpec;
use crate::schema::appearance_graph::{
    GraphChanged, GraphCreateParams, GraphDeleteParams, GraphDeleteResult, GraphDuplicateParams, GraphEditParams, GraphGetParams, GraphGetResult,
    GraphIssueReply, GraphListParams, GraphListResult, GraphNodesParams, GraphNodesResult, GraphSetParams, GraphUpdateFromLibraryParams,
    GraphUpdateResult, GraphValidateParams, GraphValidateResult, TextureAddParams, TextureAddResult, TextureListParams, TextureListResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session, params::bad};
use serde_json::Value;
use solidcraft_render::material_graph::{self as mg, GraphError};

const NODES: TypedContract<GraphNodesParams> = TypedContract::new();
const VALIDATE: TypedContract<GraphValidateParams> = TypedContract::new();
const CREATE: TypedContract<GraphCreateParams> = TypedContract::new();
const SET: TypedContract<GraphSetParams> = TypedContract::new();
const EDIT: TypedContract<GraphEditParams> = TypedContract::new();
const GET: TypedContract<GraphGetParams> = TypedContract::new();
const LIST: TypedContract<GraphListParams> = TypedContract::new();
const DUPLICATE: TypedContract<GraphDuplicateParams> = TypedContract::new();
const DELETE: TypedContract<GraphDeleteParams> = TypedContract::new();
const UPDATE: TypedContract<GraphUpdateFromLibraryParams> = TypedContract::new();
const TEXTURE_ADD: TypedContract<TextureAddParams> = TypedContract::new();
const TEXTURE_LIST: TypedContract<TextureListParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("appearance.graph.nodes", "Material Graph Nodes", crate::checked!("appearance.graph.nodes", nodes))
        .icon("list")
        .noundo()
        .doc_only()
        .contract(&NODES)
        .output(output_schema_for::<GraphNodesResult>)
        .params("no parameters → {version, nodes}: every node kind with typed inputs, ranges, defaults, params, outputs and cost; read-only"),
    CommandSpec::new("appearance.graph.validate", "Validate Material Graph", crate::checked!("appearance.graph.validate", validate))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&VALIDATE)
        .output(output_schema_for::<GraphValidateResult>)
        .params("graph → {ok, cost, samples, textures, shape, errors}; read-only"),
    CommandSpec::new("appearance.graph.create", "New Material Graph", crate::checked!("appearance.graph.create", unbuilt_create))
        .icon("appearance")
        .contract(&CREATE)
        .output(output_schema_for::<GraphChanged>)
        .params("name, id?, graph? → {id, cost}"),
    CommandSpec::new("appearance.graph.set", "Replace Material Graph", crate::checked!("appearance.graph.set", unbuilt_set))
        .icon("appearance")
        .contract(&SET)
        .output(output_schema_for::<GraphChanged>)
        .params("id, graph → {id, cost}; replaces the graph as a whole"),
    CommandSpec::new("appearance.graph.edit", "Edit Material Graph", crate::checked!("appearance.graph.edit", unbuilt_edit))
        .icon("appearance")
        .contract(&EDIT)
        .output(output_schema_for::<GraphChanged>)
        .params("id, ops: [{op: add_node|remove_node|connect|disconnect|set_input|set_param|set_interface, …}] → {id, cost}; all or none"),
    CommandSpec::new("appearance.graph.get", "Material Graph", crate::checked!("appearance.graph.get", unbuilt_get))
        .icon("inspect")
        .noundo()
        .contract(&GET)
        .output(output_schema_for::<GraphGetResult>)
        .params("id → {id, graph, cost, uses}; read-only"),
    CommandSpec::new("appearance.graph.list", "List Material Graphs", crate::checked!("appearance.graph.list", unbuilt_list))
        .icon("list")
        .noundo()
        .contract(&LIST)
        .output(output_schema_for::<GraphListResult>)
        .params("no parameters → {graphs: [{id, name, uses, source?}]}; read-only"),
    CommandSpec::new("appearance.graph.duplicate", "Duplicate Material Graph", crate::checked!("appearance.graph.duplicate", unbuilt_duplicate))
        .icon("copy")
        .contract(&DUPLICATE)
        .output(output_schema_for::<GraphChanged>)
        .params("id, new_id?, name? → {id, cost}"),
    CommandSpec::new("appearance.graph.delete", "Delete Material Graph", crate::checked!("appearance.graph.delete", unbuilt_delete))
        .icon("appearance")
        .contract(&DELETE)
        .output(output_schema_for::<GraphDeleteResult>)
        .params("id, replace_with? → {replaced}; refused while looks use it, unless replace_with"),
    CommandSpec::new(
        "appearance.graph.update_from_library",
        "Update Material Graph From Library",
        crate::checked!("appearance.graph.update_from_library", unbuilt_update),
    )
    .icon("library")
    .contract(&UPDATE)
    .output(output_schema_for::<GraphUpdateResult>)
    .params("id, library, preview? → {applied, changes, looks}; preview changes nothing"),
    CommandSpec::new("appearance.texture.add", "Add Texture", crate::checked!("appearance.texture.add", unbuilt_texture_add))
        .icon("appearance")
        .contract(&TEXTURE_ADD)
        .output(output_schema_for::<TextureAddResult>)
        .params("path | data_base64, name? → {texture, width, height}; PNG or JPEG, at most 16384 px a side and 256 MiB, kept at full quality"),
    CommandSpec::new("appearance.texture.list", "List Textures", crate::checked!("appearance.texture.list", unbuilt_texture_list))
        .icon("list")
        .noundo()
        .contract(&TEXTURE_LIST)
        .output(output_schema_for::<TextureListResult>)
        .params("no parameters → {textures: [{texture, name, width, height, bytes, uses}]}; read-only"),
];

fn reply(cmd: &str, reply: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(reply).map_err(|e| bad(cmd, e.to_string()))
}

fn not_built(cmd: &str, track: &str) -> EngineError {
    EngineError::Other(format!("`{cmd}` is not built yet (material graph track {track}); nothing was changed"))
}

fn nodes(_s: &mut Session, _p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.nodes";
    let nodes = mg::CATALOGUE.iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>().map_err(|e| bad(cmd, e.to_string()))?;
    reply(cmd, GraphNodesResult { version: mg::CATALOGUE_VERSION, nodes, param_warnings: Vec::new() })
}

fn validate(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.validate";
    let graph: mg::MaterialGraph = serde_json::from_value(p.get("graph").cloned().unwrap_or(Value::Null)).map_err(|e| bad(cmd, e.to_string()))?;
    let issues = match mg::validate(&graph).and_then(|v| mg::lower(&v)) {
        Ok(ir) => {
            let shape = ir.shape.0.iter().map(|b| format!("{b:02x}")).collect();
            let textures = u32::try_from(ir.textures.len()).unwrap_or(u32::MAX);
            return reply(
                cmd,
                GraphValidateResult { ok: true, cost: ir.cost, samples: ir.samples, textures, shape, errors: Vec::new(), param_warnings: Vec::new() },
            );
        }
        Err(GraphError::NotBuilt(what)) => return Err(EngineError::Other(format!("`{cmd}`: {what} is not built yet; nothing was checked"))),
        Err(GraphError::Invalid(issues)) => issues,
    };
    let errors = issues.into_iter().map(|i| GraphIssueReply { node: i.node, port: i.port, code: i.code.to_string(), message: i.message }).collect();
    reply(cmd, GraphValidateResult { ok: false, cost: 0, samples: 0, textures: 0, shape: String::new(), errors, param_warnings: Vec::new() })
}

fn unbuilt_create(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.create", "D"))
}
fn unbuilt_set(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.set", "D"))
}
fn unbuilt_edit(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.edit", "D"))
}
fn unbuilt_get(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.get", "D"))
}
fn unbuilt_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.list", "D"))
}
fn unbuilt_duplicate(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.duplicate", "D"))
}
fn unbuilt_delete(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.delete", "D"))
}
fn unbuilt_update(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.graph.update_from_library", "D"))
}
fn unbuilt_texture_add(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.texture.add", "D"))
}
fn unbuilt_texture_list(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(not_built("appearance.texture.list", "D"))
}
