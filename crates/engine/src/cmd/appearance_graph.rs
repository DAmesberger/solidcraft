//! Material graphs and their texture assets (docs/material-graph.md §7.2). Every command that
//! changes the design is one undo step and atomic: it builds the whole change, checks the result
//! (track A's validation through track B's design checks) and returns an error otherwise, which
//! leaves the design as it was (`Session::execute` restores it).
//!
//! A look that names a graph shows its graph's base response in the standard view, so its
//! `color` is computed here ([`settle_look`]), and every command that changes a graph recomputes
//! the colour of the looks that use it ([`settle`]).
use std::collections::{BTreeMap, BTreeSet};

use super::CommandSpec;
use crate::schema::appearance_graph::{
    GraphChanged, GraphConvertParams, GraphConverted, GraphCreateParams, GraphDeleteParams, GraphDeleteResult, GraphDuplicateParams, GraphEditOp,
    GraphEditParams, GraphGetParams, GraphGetResult, GraphIssueReply, GraphListEntry, GraphListParams, GraphListResult, GraphNodesParams,
    GraphNodesResult, GraphSetParams, GraphUpdateFromLibraryParams, GraphUpdateResult, GraphValidateParams, GraphValidateResult, TextureAddParams,
    TextureAddResult, TextureEntry, TextureListParams, TextureListResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session, params::bad};
use serde_json::{Value, json};
use solidcraft_doc::Document;
use solidcraft_doc::appearance::Look;
use solidcraft_doc::appearance_graph::{self as ag, GraphId, TextureAsset, limits};
use solidcraft_render::material_graph::{
    self as mg, Connection, Constant, GraphError, GraphSource, InputValue, InterfaceInput, Ir, MaterialGraph, Node, ParamValue, PortType,
};

const NODES: TypedContract<GraphNodesParams> = TypedContract::new();
const VALIDATE: TypedContract<GraphValidateParams> = TypedContract::new();
const CREATE: TypedContract<GraphCreateParams> = TypedContract::new();
const CONVERT: TypedContract<GraphConvertParams> = TypedContract::new();
const SET: TypedContract<GraphSetParams> = TypedContract::new();
const EDIT: TypedContract<GraphEditParams> = TypedContract::new();
const GET: TypedContract<GraphGetParams> = TypedContract::new();
const LIST: TypedContract<GraphListParams> = TypedContract::new();
const DUPLICATE: TypedContract<GraphDuplicateParams> = TypedContract::new();
const DELETE: TypedContract<GraphDeleteParams> = TypedContract::new();
const UPDATE: TypedContract<GraphUpdateFromLibraryParams> = TypedContract::new();
const TEXTURE_ADD: TypedContract<TextureAddParams> = TypedContract::new();
const TEXTURE_LIST: TypedContract<TextureListParams> = TypedContract::new();

/// Edits one `appearance.graph.edit` may apply.
const MAX_OPS: usize = 512;

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
        .params("graph → {ok, cost, samples, textures, shape, errors: [{node?, port?, code, message}]}; read-only, the design is not consulted"),
    CommandSpec::new("appearance.graph.create", "New Material Graph", crate::checked!("appearance.graph.create", create))
        .icon("appearance")
        .contract(&CREATE)
        .output(output_schema_for::<GraphChanged>)
        .params(
            "name, id?, graph? | from?: \"standard\" or a saved look {name, color, opacity?, pbr?} → {id, cost}. Default: one standard_surface. \
             The graph must validate and name only the design's textures (appearance.texture.add). One undo step",
        ),
    CommandSpec::new("appearance.graph.convert", "Convert to Material Graph", crate::checked!("appearance.graph.convert", convert))
        .icon("appearance")
        .contract(&CONVERT)
        .output(output_schema_for::<GraphConverted>)
        .params(
            "look: saved look {name, color, opacity, pbr?} (appearance.list), id?, name?, scope?: {bodies?, components?} (appearance.replace) → \
             {id, cost, count}: a new graph that draws the look as it is, and every override that is exactly that look now names it. One undo step",
        ),
    CommandSpec::new("appearance.graph.set", "Replace Material Graph", crate::checked!("appearance.graph.set", set))
        .icon("appearance")
        .contract(&SET)
        .output(output_schema_for::<GraphChanged>)
        .params("id, graph → {id, cost}; replaces the graph as a whole; looks using it get its new standard-view colour. One undo step"),
    CommandSpec::new("appearance.graph.edit", "Edit Material Graph", crate::checked!("appearance.graph.edit", edit))
        .icon("appearance")
        .contract(&EDIT)
        .output(output_schema_for::<GraphChanged>)
        .params(
            "id, ops: [{op: add_node {node, kind} | remove_node {node} | connect {from, node, port, fallback?} | disconnect {node, port} | \
             set_input {node, port, value} | set_param {node, param, value} | set_interface {input}}] (≤ 512) → {id, cost}; all apply or none, one undo step",
        ),
    CommandSpec::new("appearance.graph.get", "Material Graph", crate::checked!("appearance.graph.get", get))
        .icon("inspect")
        .noundo()
        .contract(&GET)
        .output(output_schema_for::<GraphGetResult>)
        .params("id → {id, graph, cost, uses}; read-only"),
    CommandSpec::new("appearance.graph.list", "List Material Graphs", crate::checked!("appearance.graph.list", list))
        .icon("list")
        .noundo()
        .contract(&LIST)
        .output(output_schema_for::<GraphListResult>)
        .params("no parameters → {graphs: [{id, name, uses, source?}]}; read-only"),
    CommandSpec::new("appearance.graph.duplicate", "Duplicate Material Graph", crate::checked!("appearance.graph.duplicate", duplicate))
        .icon("copy")
        .contract(&DUPLICATE)
        .output(output_schema_for::<GraphChanged>)
        .params("id, new_id?, name? (default: \"<name> copy\") → {id, cost}. One undo step"),
    CommandSpec::new("appearance.graph.delete", "Delete Material Graph", crate::checked!("appearance.graph.delete", delete))
        .icon("appearance")
        .contract(&DELETE)
        .output(output_schema_for::<GraphDeleteResult>)
        .params(
            "id, replace_with? → {replaced}; refused while looks use it, unless replace_with names the graph they use instead (their values the \
             new graph does not expose are dropped). One undo step",
        ),
    CommandSpec::new(
        "appearance.graph.update_from_library",
        "Update Material Graph From Library",
        crate::checked!("appearance.graph.update_from_library", update_from_library),
    )
    .icon("library")
    .contract(&UPDATE)
    .output(output_schema_for::<GraphUpdateResult>)
    .params(
        "id, library: the catalog file its source names, preview? → {applied, changes, looks}; preview changes nothing. Applying replaces the \
         graph by the library's (with its textures), one undo step",
    ),
    CommandSpec::new("appearance.texture.add", "Add Texture", crate::checked!("appearance.texture.add", texture_add))
        .icon("appearance")
        .contract(&TEXTURE_ADD)
        .output(output_schema_for::<TextureAddResult>)
        .params(
            "path | data_base64, name? → {texture, width, height}; PNG or JPEG, at most 16384 px a side and 256 MiB, kept at full quality. \
             Adding the same bytes again changes nothing. A texture nothing references is dropped on save",
        ),
    CommandSpec::new("appearance.texture.list", "List Textures", crate::checked!("appearance.texture.list", texture_list))
        .icon("list")
        .noundo()
        .contract(&TEXTURE_LIST)
        .output(output_schema_for::<TextureListResult>)
        .params("no parameters → {textures: [{texture, name, width, height, bytes, uses}]}; read-only"),
];

fn reply(cmd: &str, reply: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(reply).map_err(|e| bad(cmd, e.to_string()))
}

fn parse<T: serde::de::DeserializeOwned>(cmd: &str, p: &Value) -> Result<T> {
    serde_json::from_value(p.clone()).map_err(|e| bad(cmd, e.to_string()))
}

fn graph_id(cmd: &str, id: &str) -> Result<GraphId> {
    GraphId::new(id).map_err(|e| bad(cmd, e.to_string()))
}

fn plain_name(cmd: &str, name: &str) -> Result<()> {
    if name.is_empty() || name.len() > limits::NAME_BYTES || name.chars().any(char::is_control) {
        return Err(bad(cmd, format!("a graph's name is 1–{} bytes without control characters", limits::NAME_BYTES)));
    }
    Ok(())
}

/// The graph in a command's `graph` parameter.
fn parse_graph(cmd: &str, value: &Value) -> Result<MaterialGraph> {
    serde_json::from_value(value.clone()).map_err(|e| bad(cmd, format!("`graph`: {e}")))
}

/// A refusal that lists a graph's problems, as `appearance.graph.validate` reports them.
fn invalid(cmd: &str, what: &str, e: &GraphError) -> crate::EngineError {
    let detail = match e {
        GraphError::Invalid(issues) => issues
            .iter()
            .take(8)
            .map(|i| {
                let at = match (&i.node, &i.port) {
                    (Some(n), Some(p)) => format!("{n}.{p}: "),
                    (Some(n), None) => format!("{n}: "),
                    (None, Some(p)) => format!("{p}: "),
                    (None, None) => String::new(),
                };
                format!("{at}{} [{}]", i.message, i.code)
            })
            .collect::<Vec<_>>()
            .join("; "),
        GraphError::NotBuilt(part) => format!("{part} is not built yet"),
    };
    bad(cmd, format!("{what} is not a valid material graph (appearance.graph.validate lists the problems): {detail}"))
}

/// Validate and lower: the IR a valid graph compiles to.
fn lowered(graph: &MaterialGraph) -> std::result::Result<Ir, GraphError> {
    mg::validate(graph).and_then(|v| mg::lower(&v))
}

fn cost_of(graph: &MaterialGraph) -> u32 {
    lowered(graph).map_or(0, |ir| ir.cost)
}

/// A new graph id made from `name`: `[a-z0-9_.-]`, and not yet in the table.
fn fresh_id(cmd: &str, doc: &Document, name: &str) -> Result<GraphId> {
    let mut stem = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.') { c } else { '-' };
        if !(c == '-' && (stem.is_empty() || stem.ends_with('-'))) {
            stem.push(c);
        }
    }
    let mut stem: String = stem.trim_end_matches('-').chars().take(limits::ID_BYTES - 6).collect();
    if stem.is_empty() {
        stem.push_str("graph");
    }
    let graphs = &doc.appearances.tables.graphs;
    let free = |id: &str| GraphId::new(id).ok().filter(|id| !graphs.contains_key(id));
    free(&stem)
        .or_else(|| (2..=limits::GRAPHS_PER_DESIGN + 1).find_map(|i| free(&format!("{stem}-{i}"))))
        .ok_or_else(|| bad(cmd, "no free graph id; give `id`"))
}

/// One `standard_surface` with its defaults.
fn standard_graph(name: &str) -> MaterialGraph {
    MaterialGraph {
        name: name.to_string(),
        interface: Vec::new(),
        nodes: vec![Node { id: "surface".into(), kind: "standard_surface".into(), params: BTreeMap::new(), inputs: BTreeMap::new() }],
        output: "surface".into(),
        source: None,
        layout: BTreeMap::new(),
    }
}

/// The one-node graph that draws a scalar or `pbr` look as it is (docs/material-graph.md §3.7):
/// its exact sRGB8 colour, and with `pbr` its metalness, roughness and IOR. A scalar look keeps
/// the surface's other defaults.
pub(crate) fn look_graph(name: &str, look: &Look) -> MaterialGraph {
    let mut graph = standard_graph(name);
    if let Some(surface) = graph.nodes.first_mut() {
        let inputs = &mut surface.inputs;
        inputs.insert("base_color".into(), InputValue::Constant(Constant::Srgb8(mg::graph::Srgb8 { srgb8: look.color })));
        if let Some(pbr) = look.pbr {
            inputs.insert("base_metalness".into(), InputValue::Constant(Constant::Number(pbr.metallic())));
            inputs.insert("specular_roughness".into(), InputValue::Constant(Constant::Number(pbr.roughness())));
            inputs.insert("specular_ior".into(), InputValue::Constant(Constant::Number(pbr.dielectric_ior())));
        }
    }
    graph
}

/// How many looks name each graph (every look the design holds, `Document::for_each_look`).
fn uses(doc: &Document) -> BTreeMap<GraphId, u32> {
    let mut uses = BTreeMap::new();
    let _ = doc.for_each_look(&mut |look| {
        if let Some(id) = &look.graph {
            let n: &mut u32 = uses.entry(id.clone()).or_default();
            *n = n.saturating_add(1);
        }
        Ok(())
    });
    uses
}

/// Check a look's `values` against its graph: each a held interface input of a matching type
/// (a texture key for a texture input, a number or vector otherwise) within its input's range.
fn check_values(cmd: &str, look: &Look, graph: &MaterialGraph, ir: &Ir) -> Result<()> {
    for (name, value) in &look.values {
        let Some(input) = graph.interface.iter().find(|i| &i.name == name) else {
            return Err(bad(cmd, format!("look `{}` sets `{name}`, which its material graph does not expose", look.name)));
        };
        let texture = matches!(value, Constant::Texture(_));
        if texture != (input.ty == PortType::Texture) {
            let want = if input.ty == PortType::Texture { "a texture {\"texture\": sha256}" } else { "a number, vector or colour" };
            return Err(bad(cmd, format!("look `{}`: graph input `{name}` takes {want}", look.name)));
        }
    }
    mg::uniform_words(ir, &look.values).map_err(|e| invalid(cmd, &format!("look `{}`'s values", look.name), &e))?;
    Ok(())
}

/// The colour the standard view shows for a graph with these values: the base response's base
/// colour, sRGB8 (docs/material-graph.md §4.1).
fn display_color(cmd: &str, graph: &MaterialGraph, values: &BTreeMap<String, Constant>) -> Result<[u8; 3]> {
    let mut graph = graph.clone();
    for input in &mut graph.interface {
        if let Some(v) = values.get(&input.name) {
            input.default = Some(v.clone());
        }
    }
    let validated = mg::validate(&graph).map_err(|e| invalid(cmd, "the look's graph", &e))?;
    let base = mg::base_response(&validated).map_err(|e| invalid(cmd, "the look's graph", &e))?.base_color();
    Ok([base.x, base.y, base.z].map(solidcraft_geom::srgb::encode8))
}

/// Make a look that names a graph whole before it is assigned: the graph is in the design, its
/// values fit, the opacity rule holds, and `color` is the graph's base colour. A look without a
/// graph is left as it is.
pub(crate) fn settle_look(cmd: &str, doc: &Document, look: &mut Look) -> Result<()> {
    let Some(id) = &look.graph else { return Ok(()) };
    let tables = &doc.appearances.tables;
    tables.check_look(look).map_err(|e| bad(cmd, e.to_string()))?;
    let graph = tables.graphs.get(id).ok_or_else(|| bad(cmd, format!("no material graph `{}` (appearance.graph.list)", id.as_str())))?;
    let ir = lowered(graph).map_err(|e| invalid(cmd, &format!("material graph `{}`", id.as_str()), &e))?;
    check_values(cmd, look, graph, &ir)?;
    look.color = display_color(cmd, graph, &look.values)?;
    Ok(())
}

/// After the graphs `changed` changed (were added, replaced or removed) in a staged copy of the
/// design: check each against the limits, the design's textures and validation; check every
/// look the design holds against the tables (B's checks); and give the looks that name a changed
/// graph its new standard-view colour. `textures`: the texture table changed too, so it is
/// checked as a whole (`Document::check_appearance_graphs`); otherwise its unchanged entries are
/// not re-hashed.
fn settle(cmd: &str, doc: &mut Document, changed: &BTreeSet<GraphId>, textures: bool) -> Result<()> {
    let fail = |e: solidcraft_doc::DocError| bad(cmd, e.to_string());
    {
        let tables = &doc.appearances.tables;
        if textures {
            doc.check_appearance_graphs().map_err(fail)?;
        } else {
            tables.check_counts().map_err(fail)?;
            for id in changed {
                if let Some(graph) = tables.graphs.get(id) {
                    ag::check_graph_in(id, graph, &tables.textures).map_err(fail)?;
                }
            }
            doc.for_each_look(&mut |look| tables.check_look(look)).map_err(fail)?;
        }
    }
    // New colours, one per graph and values.
    let mut colours: BTreeMap<(GraphId, String), [u8; 3]> = BTreeMap::new();
    {
        let doc: &Document = doc;
        let mut work = |look: &Look| -> Result<()> {
            let Some(id) = look.graph.as_ref().filter(|id| changed.contains(*id)) else { return Ok(()) };
            let key = (id.clone(), serde_json::to_string(&look.values).unwrap_or_default());
            if let std::collections::btree_map::Entry::Vacant(slot) = colours.entry(key) {
                let mut settled = look.clone();
                settle_look(cmd, doc, &mut settled)?;
                slot.insert(settled.color);
            }
            Ok(())
        };
        let a = &doc.appearances;
        for look in a.bodies.values().chain(a.components.values()).chain(a.faces.iter().map(|f| &f.look)) {
            work(look)?;
        }
        for look in doc.user_materials.iter().filter_map(|m| m.appearance.as_ref()) {
            work(look)?;
        }
    }
    let colour = |look: &mut Look| {
        if let Some(id) = &look.graph
            && let Some(c) = colours.get(&(id.clone(), serde_json::to_string(&look.values).unwrap_or_default()))
        {
            look.color = *c;
        }
    };
    let a = &mut doc.appearances;
    a.bodies.values_mut().chain(a.components.values_mut()).chain(a.faces.iter_mut().map(|f| &mut f.look)).for_each(colour);
    doc.user_materials.iter_mut().filter_map(|m| m.appearance.as_mut()).for_each(colour);
    Ok(())
}

/// Check a staged design and make it the session's: a refusal leaves the session untouched
/// (the same design, history and revision).
fn commit(cmd: &str, s: &mut Session, mut doc: Document, changed: &BTreeSet<GraphId>, textures: bool) -> Result<()> {
    settle(cmd, &mut doc, changed, textures)?;
    if doc != *s.doc {
        s.set_doc(doc);
    }
    Ok(())
}

fn changed_reply(cmd: &str, s: &Session, id: &GraphId) -> Result<Value> {
    let cost = s.doc.appearances.tables.graphs.get(id).map_or(0, cost_of);
    reply(cmd, GraphChanged { id: id.as_str().to_string(), cost, param_warnings: Vec::new() })
}

/// Put `graph` in the table as `id` (new or replacing), then check and commit.
fn store(cmd: &str, s: &mut Session, id: &GraphId, graph: MaterialGraph) -> Result<()> {
    if s.doc.appearances.tables.graphs.get(id) == Some(&graph) {
        return Ok(());
    }
    let mut doc = (*s.doc).clone();
    doc.appearances.tables.graphs.insert(id.clone(), graph);
    commit(cmd, s, doc, &BTreeSet::from([id.clone()]), false)
}

fn nodes(_s: &mut Session, _p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.nodes";
    let nodes = mg::CATALOGUE.iter().map(serde_json::to_value).collect::<std::result::Result<Vec<_>, _>>().map_err(|e| bad(cmd, e.to_string()))?;
    reply(cmd, GraphNodesResult { version: mg::CATALOGUE_VERSION, nodes, param_warnings: Vec::new() })
}

fn validate(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.validate";
    let graph = parse_graph(cmd, p.get("graph").unwrap_or(&Value::Null))?;
    let issues = match lowered(&graph) {
        Ok(ir) => {
            let shape = ir.shape.0.iter().map(|b| format!("{b:02x}")).collect();
            let textures = u32::try_from(ir.textures.len()).unwrap_or(u32::MAX);
            return reply(
                cmd,
                GraphValidateResult { ok: true, cost: ir.cost, samples: ir.samples, textures, shape, errors: Vec::new(), param_warnings: Vec::new() },
            );
        }
        Err(GraphError::NotBuilt(what)) => return Err(bad(cmd, format!("{what} is not built yet; nothing was checked"))),
        Err(GraphError::Invalid(issues)) => issues,
    };
    let errors = issues.into_iter().map(|i| GraphIssueReply { node: i.node, port: i.port, code: i.code.to_string(), message: i.message }).collect();
    reply(cmd, GraphValidateResult { ok: false, cost: 0, samples: 0, textures: 0, shape: String::new(), errors, param_warnings: Vec::new() })
}

/// A saved look given as `from` or `look`: `{name?, color, opacity?, pbr?}`, read as
/// `appearance.assign` reads one. It must not already name a graph.
fn saved_look(cmd: &str, value: &Value) -> Result<Look> {
    if value.get("graph").is_some_and(|g| !g.is_null()) {
        return Err(bad(cmd, "the look already names a material graph"));
    }
    let object = value.as_object().ok_or_else(|| bad(cmd, "a look is {name, color, opacity?, pbr?}"))?;
    if let Some(key) = object.keys().find(|k| !matches!(k.as_str(), "name" | "color" | "opacity" | "pbr" | "values" | "graph")) {
        return Err(bad(cmd, format!("a look has no `{key}`; give {{name, color, opacity?, pbr?}}")));
    }
    let mut params = value.clone();
    if let Some(o) = params.as_object_mut() {
        o.remove("graph");
        o.remove("values");
    }
    super::inspect::appearance_look(cmd, &params)?.ok_or_else(|| bad(cmd, "a look needs a color"))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.create";
    let params: GraphCreateParams = parse(cmd, p)?;
    plain_name(cmd, &params.name)?;
    let mut graph = match (&params.graph, &params.from) {
        (Some(_), Some(_)) => return Err(bad(cmd, "give `graph` or `from`, not both")),
        (Some(g), None) => parse_graph(cmd, &g.0)?,
        (None, None) => standard_graph(&params.name),
        (None, Some(from)) if from.0 == json!("standard") => standard_graph(&params.name),
        (None, Some(from)) if from.0.is_object() => look_graph(&params.name, &saved_look(cmd, &from.0)?),
        (None, Some(_)) => return Err(bad(cmd, "`from` is \"standard\" or a saved look {name, color, opacity?, pbr?}")),
    };
    graph.name = params.name.clone();
    let id = new_id(cmd, s, params.id.as_deref(), &params.name)?;
    store(cmd, s, &id, graph)?;
    changed_reply(cmd, s, &id)
}

/// The id a new graph gets: the one given (refused when taken), else one made from `name`.
fn new_id(cmd: &str, s: &Session, given: Option<&str>, name: &str) -> Result<GraphId> {
    let graphs = &s.doc.appearances.tables.graphs;
    if graphs.len() >= limits::GRAPHS_PER_DESIGN {
        return Err(bad(cmd, format!("a design holds at most {} material graphs", limits::GRAPHS_PER_DESIGN)));
    }
    match given {
        Some(id) => {
            let id = graph_id(cmd, id)?;
            if graphs.contains_key(&id) {
                return Err(bad(cmd, format!("material graph `{}` already exists; choose another id", id.as_str())));
            }
            Ok(id)
        }
        None => fresh_id(cmd, &s.doc, name),
    }
}

fn convert(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.convert";
    let params: GraphConvertParams = parse(cmd, p)?;
    let look = saved_look(cmd, &params.look)?;
    let name = params.name.clone().unwrap_or_else(|| look.name.clone());
    plain_name(cmd, &name)?;
    let id = new_id(cmd, s, params.id.as_deref(), &name)?;
    let scope = params.scope.as_ref().map(|v| super::inspect::replace_scope(s, cmd, v)).transpose()?;
    s.doc.appearances.check_budget().map_err(|e| bad(cmd, e.to_string()))?;
    let mut doc = (*s.doc).clone();
    doc.appearances.tables.graphs.insert(id.clone(), look_graph(&name, &look));
    let mut to = Look { graph: Some(id.clone()), pbr: None, values: BTreeMap::new(), ..look.clone() };
    settle_look(cmd, &doc, &mut to)?;
    let count = match doc.appearances.replacing(&look, &to, scope.as_ref()).map_err(|e| bad(cmd, e.to_string()))? {
        Some((appearances, count)) => {
            doc.appearances = appearances;
            count
        }
        None => 0,
    };
    commit(cmd, s, doc, &BTreeSet::from([id.clone()]), false)?;
    let cost = s.doc.appearances.tables.graphs.get(&id).map_or(0, cost_of);
    reply(cmd, GraphConverted { id: id.as_str().to_string(), cost, count: u64::try_from(count).unwrap_or(u64::MAX), param_warnings: Vec::new() })
}

fn existing(cmd: &str, s: &Session, id: &str) -> Result<(GraphId, MaterialGraph)> {
    let id = graph_id(cmd, id)?;
    let graph = s
        .doc
        .appearances
        .tables
        .graphs
        .get(&id)
        .cloned()
        .ok_or_else(|| bad(cmd, format!("no material graph `{}` (appearance.graph.list)", id.as_str())))?;
    Ok((id, graph))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.set";
    let params: GraphSetParams = parse(cmd, p)?;
    let (id, _) = existing(cmd, s, &params.id)?;
    let graph = parse_graph(cmd, &params.graph.0)?;
    store(cmd, s, &id, graph)?;
    changed_reply(cmd, s, &id)
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.edit";
    let params: GraphEditParams = parse(cmd, p)?;
    if params.ops.is_empty() || params.ops.len() > MAX_OPS {
        return Err(bad(cmd, format!("give 1–{MAX_OPS} edits")));
    }
    let (id, mut graph) = existing(cmd, s, &params.id)?;
    for (i, op) in params.ops.iter().enumerate() {
        apply_op(&mut graph, op).map_err(|why| bad(cmd, format!("edit {i}: {why}")))?;
    }
    store(cmd, s, &id, graph)?;
    changed_reply(cmd, s, &id)
}

fn node_mut<'a>(graph: &'a mut MaterialGraph, id: &str) -> std::result::Result<&'a mut Node, String> {
    graph.nodes.iter_mut().find(|n| n.id == id).ok_or_else(|| format!("no node `{}`", shown(id)))
}

/// An id or name as it may appear in a message: bounded, without control characters.
fn shown(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// Apply one edit to a working copy. The ops' structure is checked here; whether the result is a
/// valid graph is validation's business, once all ops have applied.
fn apply_op(graph: &mut MaterialGraph, op: &GraphEditOp) -> std::result::Result<(), String> {
    match op {
        GraphEditOp::AddNode { node, kind } => {
            if graph.nodes.iter().any(|n| &n.id == node) {
                return Err(format!("node `{}` already exists", shown(node)));
            }
            if mg::node_spec(kind).is_none() {
                return Err(format!("no node kind `{}` (appearance.graph.nodes)", shown(kind)));
            }
            if graph.nodes.len() >= limits::NODES {
                return Err(format!("a graph has at most {} nodes", limits::NODES));
            }
            graph.nodes.push(Node { id: node.clone(), kind: kind.clone(), params: BTreeMap::new(), inputs: BTreeMap::new() });
        }
        GraphEditOp::RemoveNode { node } => {
            if &graph.output == node {
                return Err(format!("node `{}` is the graph's output", shown(node)));
            }
            let before = graph.nodes.len();
            graph.nodes.retain(|n| &n.id != node);
            if graph.nodes.len() == before {
                return Err(format!("no node `{}`", shown(node)));
            }
            for n in &mut graph.nodes {
                n.inputs.retain(|_, v| !matches!(v, InputValue::Connection(c) if c.source().0 == node));
            }
            graph.layout.remove(node);
        }
        GraphEditOp::Connect { from, node, port, fallback } => {
            let fallback = fallback
                .as_ref()
                .filter(|f| !f.is_null())
                .map(|f| serde_json::from_value::<Constant>(f.clone()))
                .transpose()
                .map_err(|_| "`fallback` is a constant: a number, a 2- or 3-vector, {\"srgb8\": [r,g,b]} or {\"texture\": sha256}".to_string())?;
            node_mut(graph, node)?.inputs.insert(port.clone(), InputValue::Connection(Connection { from: from.clone(), fallback }));
        }
        GraphEditOp::Disconnect { node, port } => {
            let n = node_mut(graph, node)?;
            if !matches!(n.inputs.get(port), Some(InputValue::Connection(_))) {
                return Err(format!("`{}.{}` is not connected", shown(node), shown(port)));
            }
            n.inputs.remove(port);
        }
        GraphEditOp::SetInput { node, port, value } => {
            let value: InputValue = serde_json::from_value(value.clone()).map_err(|_| {
                "`value` is a constant (a number, a 2- or 3-vector, {\"srgb8\": [r,g,b]}, {\"texture\": sha256}) or {\"interface\": name}".to_string()
            })?;
            if matches!(value, InputValue::Connection(_)) {
                return Err("use `connect` to drive an input from a node".into());
            }
            node_mut(graph, node)?.inputs.insert(port.clone(), value);
        }
        GraphEditOp::SetParam { node, param, value } => {
            let value: ParamValue =
                serde_json::from_value(value.clone()).map_err(|_| "a param's `value` is an option name or an integer".to_string())?;
            node_mut(graph, node)?.params.insert(param.clone(), value);
        }
        GraphEditOp::SetInterface { input } => {
            let input: InterfaceInput = serde_json::from_value(input.clone()).map_err(|e| format!("`input` is {{name, type, default?}}: {e}"))?;
            match graph.interface.iter_mut().find(|i| i.name == input.name) {
                Some(old) => *old = input,
                None => {
                    if graph.interface.len() >= limits::INTERFACE_INPUTS {
                        return Err(format!("a graph has at most {} interface inputs", limits::INTERFACE_INPUTS));
                    }
                    graph.interface.push(input);
                }
            }
        }
    }
    Ok(())
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.get";
    let params: GraphGetParams = parse(cmd, p)?;
    let (id, graph) = existing(cmd, s, &params.id)?;
    let uses = uses(&s.doc).get(&id).copied().unwrap_or(0);
    let cost = cost_of(&graph);
    let graph = serde_json::to_value(&graph).map_err(|e| bad(cmd, e.to_string()))?;
    reply(cmd, GraphGetResult { id: id.as_str().to_string(), graph, cost, uses, param_warnings: Vec::new() })
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.list";
    let _: GraphListParams = parse(cmd, p)?;
    let uses = uses(&s.doc);
    let graphs = s
        .doc
        .appearances
        .tables
        .graphs
        .iter()
        .map(|(id, g)| GraphListEntry {
            id: id.as_str().to_string(),
            name: g.name.clone(),
            uses: uses.get(id).copied().unwrap_or(0),
            source: g.source.as_ref().and_then(|s| serde_json::to_value(s).ok()),
        })
        .collect();
    reply(cmd, GraphListResult { graphs, param_warnings: Vec::new() })
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.duplicate";
    let params: GraphDuplicateParams = parse(cmd, p)?;
    let (_, mut graph) = existing(cmd, s, &params.id)?;
    let name = match params.name {
        Some(name) => name,
        None => {
            let mut name = graph.name.clone();
            while name.len() > limits::NAME_BYTES - 5 {
                name.pop();
            }
            name + " copy"
        }
    };
    plain_name(cmd, &name)?;
    let id = new_id(cmd, s, params.new_id.as_deref(), &name)?;
    graph.name = name;
    store(cmd, s, &id, graph)?;
    changed_reply(cmd, s, &id)
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.delete";
    let params: GraphDeleteParams = parse(cmd, p)?;
    let (id, _) = existing(cmd, s, &params.id)?;
    let used = uses(&s.doc).get(&id).copied().unwrap_or(0);
    let target = match &params.replace_with {
        Some(other) => {
            let (other, graph) = existing(cmd, s, other)?;
            if other == id {
                return Err(bad(cmd, "`replace_with` names the graph being deleted"));
            }
            Some((other, graph))
        }
        None if used > 0 => {
            return Err(bad(cmd, format!("{used} looks use material graph `{}`; give `replace_with` or reassign them first", id.as_str())));
        }
        None => None,
    };
    let mut replaced = 0u32;
    let mut doc = (*s.doc).clone();
    if let Some((other, graph)) = &target {
        let mut move_look = |look: &mut Look| {
            if look.graph.as_ref() == Some(&id) {
                look.graph = Some(other.clone());
                look.values.retain(|name, _| graph.interface.iter().any(|i| &i.name == name));
                replaced = replaced.saturating_add(1);
            }
        };
        let a = &mut doc.appearances;
        a.bodies.values_mut().chain(a.components.values_mut()).chain(a.faces.iter_mut().map(|f| &mut f.look)).for_each(&mut move_look);
        doc.user_materials.iter_mut().filter_map(|m| m.appearance.as_mut()).for_each(&mut move_look);
    }
    doc.appearances.tables.graphs.remove(&id);
    // Looks left naming the deleted graph (in linked designs' baselines or configuration cells)
    // fail the check, which refuses the whole deletion.
    let changed: BTreeSet<GraphId> = target.into_iter().map(|(other, _)| other).collect();
    commit(cmd, s, doc, &changed, false)?;
    reply(cmd, GraphDeleteResult { replaced, param_warnings: Vec::new() })
}

/// A graph's content revision: the first 64 bits of the SHA-256 of its JSON without its
/// provenance and editor layout. Equal content, equal revision.
pub(crate) fn content_revision(graph: &MaterialGraph) -> u64 {
    let mut bare = graph.clone();
    bare.source = None;
    bare.layout.clear();
    let text = serde_json::to_vec(&bare).unwrap_or_default();
    let key = ag::texture_key(&text);
    key.get(..16).and_then(|hex| u64::from_str_radix(hex, 16).ok()).unwrap_or(0)
}

/// Copy library graph `entry` and the textures it names from a catalog into the design as `id`,
/// recording its source, and check the result. Refused when `id` holds a different graph: an
/// existing graph is never silently replaced (§4.2).
pub(crate) fn import_graph(cmd: &str, doc: &mut Document, catalog: &solidcraft_io::catalog::Catalog, entry: &GraphId, id: &GraphId) -> Result<()> {
    let mut graph = catalog.graphs.get(entry).cloned().ok_or_else(|| bad(cmd, format!("the library has no material graph `{}`", entry.as_str())))?;
    graph.source = Some(GraphSource { library: catalog.id.clone(), entry: entry.as_str().to_string(), revision: content_revision(&graph) });
    if let Some(held) = doc.appearances.tables.graphs.get(id) {
        if content_revision(held) == content_revision(&graph) {
            return Ok(());
        }
        return Err(bad(cmd, format!("the design already has a different material graph `{}`; choose another id for the copy", id.as_str())));
    }
    copy_textures(cmd, doc, catalog, ag::graph_textures(&graph).map(str::to_string).collect())?;
    doc.appearances.tables.graphs.insert(id.clone(), graph);
    settle(cmd, doc, &BTreeSet::from([id.clone()]), true)
}

/// Add a built graph and the textures it reads as `id`, recording `source`: one checked change.
/// The same graph already held under `id` is reused (its source and layout aside); a different
/// one is refused, never replaced.
pub(crate) fn add_graph(cmd: &str, s: &mut Session, id: &GraphId, mut graph: MaterialGraph, textures: Vec<(String, TextureAsset)>) -> Result<()> {
    if let Some(held) = s.doc.appearances.tables.graphs.get(id) {
        if content_revision(held) == content_revision(&graph) {
            return Ok(());
        }
        return Err(bad(cmd, format!("the design already has a different material graph `{}`; give `graph_id` to choose another id", id.as_str())));
    }
    if s.doc.appearances.tables.graphs.len() >= limits::GRAPHS_PER_DESIGN {
        return Err(bad(cmd, format!("a design holds at most {} material graphs", limits::GRAPHS_PER_DESIGN)));
    }
    let revision = content_revision(&graph);
    if let Some(source) = &mut graph.source {
        source.revision = revision;
    }
    let mut doc = (*s.doc).clone();
    for (key, texture) in textures {
        doc.appearances.tables.textures.entry(key).or_insert(texture);
    }
    doc.appearances.tables.graphs.insert(id.clone(), graph);
    commit(cmd, s, doc, &BTreeSet::from([id.clone()]), true)
}

/// Copy the textures `keys` names from a catalog into the design (those it lacks).
pub(crate) fn copy_textures(cmd: &str, doc: &mut Document, catalog: &solidcraft_io::catalog::Catalog, keys: BTreeSet<String>) -> Result<()> {
    let missing: Vec<String> = keys.into_iter().filter(|k| !doc.appearances.tables.textures.contains_key(k)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut copies = Vec::new();
    for key in missing {
        let texture = catalog.textures.get(&key).ok_or_else(|| bad(cmd, format!("the library lacks texture `{}`", shown(&key))))?;
        copies.push((key, texture.clone()));
    }
    doc.appearances.tables.textures.extend(copies);
    Ok(())
}

/// What differs between two graphs, input by input (provenance and layout ignored).
fn differences(old: &MaterialGraph, new: &MaterialGraph) -> Vec<String> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push("name".to_string());
    }
    if old.output != new.output {
        out.push("output".to_string());
    }
    let ifaces =
        |g: &MaterialGraph| g.interface.iter().map(|i| (i.name.clone(), serde_json::to_string(i).unwrap_or_default())).collect::<BTreeMap<_, _>>();
    diff_maps(&mut out, "interface ", &ifaces(old), &ifaces(new));
    let nodes = |g: &MaterialGraph| g.nodes.iter().map(|n| (n.id.clone(), n.clone())).collect::<BTreeMap<_, _>>();
    let (a, b) = (nodes(old), nodes(new));
    for id in a.keys().filter(|k| !b.contains_key(*k)) {
        out.push(format!("node {id} removed"));
    }
    for (id, n) in &b {
        let Some(o) = a.get(id) else {
            out.push(format!("node {id} added ({})", n.kind));
            continue;
        };
        if o.kind != n.kind {
            out.push(format!("node {id}: kind {} → {}", o.kind, n.kind));
            continue;
        }
        let params = |n: &Node| n.params.iter().map(|(k, v)| (k.clone(), serde_json::to_string(v).unwrap_or_default())).collect::<BTreeMap<_, _>>();
        let inputs = |n: &Node| n.inputs.iter().map(|(k, v)| (k.clone(), serde_json::to_string(v).unwrap_or_default())).collect::<BTreeMap<_, _>>();
        diff_maps(&mut out, &format!("{id}."), &params(o), &params(n));
        diff_maps(&mut out, &format!("{id}."), &inputs(o), &inputs(n));
    }
    out
}

fn diff_maps(out: &mut Vec<String>, prefix: &str, old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) {
    for (k, v) in old {
        match new.get(k) {
            None => out.push(format!("{prefix}{k} removed")),
            Some(w) if w != v => out.push(format!("{prefix}{k}: {v} → {w}")),
            _ => {}
        }
    }
    for (k, w) in new.iter().filter(|(k, _)| !old.contains_key(*k)) {
        out.push(format!("{prefix}{k} added: {w}"));
    }
}

fn update_from_library(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.update_from_library";
    let params: GraphUpdateFromLibraryParams = parse(cmd, p)?;
    let (id, graph) = existing(cmd, s, &params.id)?;
    let source = graph.source.clone().ok_or_else(|| bad(cmd, format!("material graph `{}` was not copied from a library", id.as_str())))?;
    if params.library.is_empty() || params.library.len() > 4096 || params.library.chars().any(char::is_control) {
        return Err(bad(cmd, "invalid library path"));
    }
    solidcraft_io::vfs::check(&params.library).map_err(|e| bad(cmd, e.to_string()))?;
    let catalog = solidcraft_io::catalog::Catalog::read(&params.library).map_err(|e| bad(cmd, e.to_string()))?;
    if catalog.id != source.library {
        return Err(bad(cmd, format!("this library is `{}`, but the graph came from `{}`", shown(&catalog.id), shown(&source.library))));
    }
    let entry = graph_id(cmd, &source.entry)?;
    let new = catalog.graphs.get(&entry).ok_or_else(|| bad(cmd, format!("the library no longer has material graph `{}`", entry.as_str())))?;
    let changes = differences(&graph, new);
    let looks = uses(&s.doc).get(&id).copied().unwrap_or(0);
    if params.preview || changes.is_empty() && content_revision(new) == source.revision {
        return reply(cmd, GraphUpdateResult { applied: false, changes, looks, param_warnings: Vec::new() });
    }
    let mut copy = new.clone();
    copy.source = Some(GraphSource { library: catalog.id.clone(), entry: source.entry.clone(), revision: content_revision(new) });
    copy.layout = if new.layout.is_empty() { graph.layout.clone() } else { new.layout.clone() };
    let mut doc = (*s.doc).clone();
    copy_textures(cmd, &mut doc, &catalog, ag::graph_textures(&copy).map(str::to_string).collect())?;
    doc.appearances.tables.graphs.insert(id.clone(), copy);
    commit(cmd, s, doc, &BTreeSet::from([id]), true)?;
    reply(cmd, GraphUpdateResult { applied: true, changes, looks, param_warnings: Vec::new() })
}

fn texture_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.texture.add";
    let params: TextureAddParams = parse(cmd, p)?;
    let (bytes, file_name) = match (&params.path, &params.data_base64) {
        (Some(path), None) => {
            if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                return Err(bad(cmd, "invalid texture path"));
            }
            solidcraft_io::vfs::check(path).map_err(|e| bad(cmd, e.to_string()))?;
            let bytes = solidcraft_io::vfs::read(path, limits::TEXTURE_BYTES).map_err(|e| bad(cmd, e.to_string()))?;
            let name = std::path::Path::new(path).file_name().and_then(|n| n.to_str()).map(str::to_string);
            (bytes, name)
        }
        (None, Some(data)) => {
            if data.len() > limits::TEXTURE_BYTES.div_ceil(3).saturating_mul(4).saturating_add(4) {
                return Err(bad(cmd, format!("a texture is at most {} MiB", limits::TEXTURE_BYTES >> 20)));
            }
            let bytes = solidcraft_doc::canvas::base64_decode(data.trim()).ok_or_else(|| bad(cmd, "`data_base64` is not base64"))?;
            (bytes, None)
        }
        _ => return Err(bad(cmd, "give `path` or `data_base64`, not both")),
    };
    let name = params.name.or(file_name).unwrap_or_else(|| "texture".to_string());
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    if name.is_empty() || name.len() > limits::NAME_BYTES {
        return Err(bad(cmd, format!("a texture's name is 1–{} bytes", limits::NAME_BYTES)));
    }
    let (key, asset) = TextureAsset::from_bytes(&name, &bytes).map_err(|e| bad(cmd, e.to_string()))?;
    let (width, height) = (asset.width, asset.height);
    if !s.doc.appearances.tables.textures.contains_key(&key) {
        if s.doc.appearances.tables.textures.len() >= limits::TEXTURES_PER_DESIGN {
            return Err(bad(cmd, format!("a design holds at most {} textures", limits::TEXTURES_PER_DESIGN)));
        }
        // The decoded budget, from the headers (the new asset is already checked).
        let mut decoded = asset.checked(&key).map_err(|e| bad(cmd, e.to_string()))?.1;
        for t in s.doc.appearances.tables.textures.values() {
            decoded = decoded.saturating_add(decoded_size(t));
        }
        if decoded > limits::DECODED_TEXTURE_BYTES {
            return Err(bad(cmd, format!("the design's textures would decode to more than {} MiB", limits::DECODED_TEXTURE_BYTES >> 20)));
        }
        s.doc_mut().appearances.tables.textures.insert(key.clone(), asset);
    }
    reply(cmd, TextureAddResult { texture: key, width, height, param_warnings: Vec::new() })
}

/// The decoded size a held texture's header promises, as `TextureAsset::checked` counts it (RGBA8
/// 4 B/px, 16-bit grey PNG data 2 B/px). It was checked when it was added or read, so only the
/// header is decoded here, not the whole image.
fn decoded_size(t: &TextureAsset) -> usize {
    let header = t.data.get(..44).and_then(solidcraft_doc::canvas::base64_decode).unwrap_or_default();
    let per_pixel = if t.format == "png" && header.get(24) == Some(&16) && header.get(25) == Some(&0) { 2 } else { 4 };
    (t.width as usize).saturating_mul(t.height as usize).saturating_mul(per_pixel)
}

fn texture_list(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.texture.list";
    let _: TextureListParams = parse(cmd, p)?;
    let doc = &s.doc;
    let mut uses: BTreeMap<&str, u32> = BTreeMap::new();
    for graph in doc.appearances.tables.graphs.values() {
        let named: BTreeSet<&str> = ag::graph_textures(graph).collect();
        for key in named {
            let n = uses.entry(key).or_default();
            *n = n.saturating_add(1);
        }
    }
    let mut look_uses: BTreeMap<String, u32> = BTreeMap::new();
    let _ = doc.for_each_look(&mut |look| {
        for key in ag::look_textures(look) {
            let n = look_uses.entry(key.to_string()).or_default();
            *n = n.saturating_add(1);
        }
        Ok(())
    });
    let textures = doc
        .appearances
        .tables
        .textures
        .iter()
        .map(|(key, t)| {
            let data = t.data.trim_end_matches('=').len();
            TextureEntry {
                texture: key.clone(),
                name: t.name.clone(),
                width: t.width,
                height: t.height,
                bytes: u64::try_from(data.saturating_mul(3) / 4).unwrap_or(u64::MAX),
                uses: uses.get(key.as_str()).copied().unwrap_or(0).saturating_add(look_uses.get(key).copied().unwrap_or(0)),
            }
        })
        .collect();
    reply(cmd, TextureListResult { textures, param_warnings: Vec::new() })
}
