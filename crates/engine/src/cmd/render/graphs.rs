//! Material graphs for one GPU path job (docs/material-graph.md §5.7, §7.3): what the design's
//! looks bind, captured with the job's runtime interface overrides. The design, its history and
//! the saved file never change, as with runtime clear coats.
//!
//! A capture is the path tracer's input in its own shapes (`job::graphs` of track PT): one
//! material per graph and values, with its lowered IR, the values (look values, then overrides)
//! and, per IR texture, its encoded bytes and colour space; and one binding per body or face, by
//! the `render.scene` ordinal and exact name, with the body's placement for object-space
//! projections.
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use solidcraft_doc::appearance::Look;
use solidcraft_doc::appearance_graph::{GraphId, is_texture_key, limits, texture_key};
use solidcraft_render::ProductScene;
use solidcraft_render::material_graph::{self as mg, Constant, Ir, PortType};

use crate::{Result, Session, params::bad};

/// Graph materials (a graph with one set of values) one job may hold: the path back end's
/// `MAX_GRAPH_MATERIALS`.
pub(super) const MAX_MATERIALS: usize = 4096;
/// Runtime overrides one job may carry.
pub(super) const MAX_OVERRIDES: usize = 256;
/// Runtime textures one job may carry, and the encoded bytes of each.
pub(super) const MAX_RUNTIME_TEXTURES: usize = 16;
pub(super) const MAX_RUNTIME_TEXTURE_BYTES: usize = 16 << 20;

/// One runtime override of graph interface values: for the graph `graph` wherever it is bound,
/// or for the body `body_index`/`body_name` (as `render.scene` lists it) whatever graph it shows.
/// Exactly one of the two. The values merge over the looks' own.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Override {
    #[serde(default)]
    graph: Option<String>,
    #[serde(default)]
    body_index: Option<usize>,
    #[serde(default)]
    body_name: Option<String>,
    values: BTreeMap<String, Constant>,
}

/// A graph with one set of values, ready for the path back end.
#[derive(Clone, Debug)]
pub(crate) struct CapturedMaterial {
    pub graph: GraphId,
    pub ir: Arc<Ir>,
    pub values: BTreeMap<String, Constant>,
    /// Per `ir.textures` entry: the encoded image and whether the `image` node reads it as sRGB.
    /// `None`: an interface texture no value supplies, so the node reads its `default` colour.
    pub textures: Vec<Option<(Arc<Vec<u8>>, bool)>>,
}

/// A body (or one of its faces) shaded by a captured material.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CapturedBinding {
    pub body_index: usize,
    pub body_name: String,
    pub face: Option<u32>,
    pub material: usize,
    /// Rows of `world = M·object + t` (`[M | t]`, mm); `None` for an unplaced body.
    pub object_to_world: Option<[[f64; 4]; 3]>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CapturedGraphs {
    pub materials: Vec<CapturedMaterial>,
    pub bindings: Vec<CapturedBinding>,
}

impl CapturedGraphs {
    /// What `render.preflight` and `render.start` report: counts, and per material its graph,
    /// the values it was captured with and its cost.
    pub fn summary(&self) -> serde_json::Value {
        let materials: Vec<_> = self
            .materials
            .iter()
            .map(|m| serde_json::json!({"graph": m.graph.as_str(), "values": m.values, "cost": m.ir.cost, "textures": m.textures.iter().filter(|t| t.is_some()).count()}))
            .collect();
        serde_json::json!({"materials": materials, "bindings": self.bindings.len()})
    }
}

/// Texture bytes by key: the design's, then the job's runtime ones.
struct Bytes<'a> {
    s: &'a Session,
    runtime: BTreeMap<String, Arc<Vec<u8>>>,
    decoded: BTreeMap<String, Arc<Vec<u8>>>,
}

impl Bytes<'_> {
    fn get(&mut self, key: &str, command: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(b) = self.runtime.get(key).or_else(|| self.decoded.get(key)) {
            return Ok(b.clone());
        }
        let asset = self
            .s
            .doc
            .appearances
            .tables
            .textures
            .get(key)
            .ok_or_else(|| bad(command, format!("texture `{}` is neither in the design nor given with the job", short(key))))?;
        let bytes = Arc::new(asset.bytes(key).map_err(|e| bad(command, e.to_string()))?);
        self.decoded.insert(key.to_string(), bytes.clone());
        Ok(bytes)
    }
}

fn short(key: &str) -> &str {
    key.get(..12).unwrap_or(key)
}

/// The job's runtime textures, by their SHA-256: base64 PNG or JPEG, bounded.
fn runtime_textures(encoded: &[String], command: &str) -> Result<BTreeMap<String, Arc<Vec<u8>>>> {
    if encoded.len() > MAX_RUNTIME_TEXTURES {
        return Err(bad(command, format!("at most {MAX_RUNTIME_TEXTURES} runtime graph textures")));
    }
    let mut out = BTreeMap::new();
    for data in encoded {
        if data.len() > MAX_RUNTIME_TEXTURE_BYTES.div_ceil(3).saturating_mul(4).saturating_add(4) {
            return Err(bad(command, format!("a runtime graph texture is at most {} MiB", MAX_RUNTIME_TEXTURE_BYTES >> 20)));
        }
        let bytes = solidcraft_doc::canvas::base64_decode(data.trim()).ok_or_else(|| bad(command, "a runtime graph texture is base64"))?;
        let (key, _) = solidcraft_doc::appearance_graph::TextureAsset::from_bytes("runtime", &bytes).map_err(|e| bad(command, e.to_string()))?;
        out.insert(key, Arc::new(bytes));
    }
    Ok(out)
}

/// Check override values against a graph's interface: each a declared input, a texture key for a
/// texture input (one the design or the job holds), a constant of the input's range otherwise.
fn check_override(
    graph: &mg::MaterialGraph,
    ir: &Ir,
    values: &BTreeMap<String, Constant>,
    known: &dyn Fn(&str) -> bool,
    command: &str,
) -> Result<()> {
    if values.len() > limits::INTERFACE_INPUTS {
        return Err(bad(command, format!("an override sets at most {} graph inputs", limits::INTERFACE_INPUTS)));
    }
    for (name, value) in values {
        let input = graph
            .interface
            .iter()
            .find(|i| &i.name == name)
            .ok_or_else(|| bad(command, format!("the graph has no interface input `{}`", name.chars().take(64).collect::<String>())))?;
        match value {
            Constant::Texture(t) if input.ty == PortType::Texture => {
                if !is_texture_key(&t.texture) || !known(&t.texture) {
                    return Err(bad(
                        command,
                        format!("override `{name}` names texture `{}`, which neither the design nor the job holds", short(&t.texture)),
                    ));
                }
            }
            Constant::Texture(_) => return Err(bad(command, format!("override `{name}` takes a number, vector or colour"))),
            _ if input.ty == PortType::Texture => return Err(bad(command, format!("override `{name}` takes {{\"texture\": sha256}}"))),
            _ => {}
        }
    }
    mg::uniform_words(ir, values).map_err(|e| bad(command, format!("override values: {e}")))?;
    Ok(())
}

/// Capture the graph looks of `scene`'s bodies and faces with `overrides`. `None` when nothing
/// in the scene shows a graph (and no override was given).
pub(crate) fn capture(
    s: &Session,
    scene: &ProductScene,
    overrides: &[Override],
    encoded: &[String],
    command: &str,
) -> Result<Option<CapturedGraphs>> {
    if overrides.len() > MAX_OVERRIDES {
        return Err(bad(command, format!("at most {MAX_OVERRIDES} runtime graph overrides")));
    }
    let tables = &s.doc.appearances.tables;
    let runtime = runtime_textures(encoded, command)?;
    let known = |key: &str| tables.textures.contains_key(key) || runtime.contains_key(key);
    // Overrides: by graph, and by body ordinal (checked against the scene's exact name).
    let mut by_graph: BTreeMap<GraphId, BTreeMap<String, Constant>> = BTreeMap::new();
    let mut by_body: BTreeMap<usize, BTreeMap<String, Constant>> = BTreeMap::new();
    for o in overrides {
        match (&o.graph, o.body_index, &o.body_name) {
            (Some(g), None, None) => {
                let id = GraphId::new(g).map_err(|e| bad(command, e.to_string()))?;
                if !tables.graphs.contains_key(&id) {
                    return Err(bad(command, format!("no material graph `{}` in the design", id.as_str())));
                }
                if by_graph.insert(id, o.values.clone()).is_some() {
                    return Err(bad(command, "one override per graph"));
                }
            }
            (None, Some(i), Some(name)) => {
                if scene.meshes.get(i).is_none_or(|m| &m.name != name) {
                    return Err(bad(command, "graph override body ordinal and exact name must both match render.scene"));
                }
                if by_body.insert(i, o.values.clone()).is_some() {
                    return Err(bad(command, "one override per body"));
                }
            }
            _ => return Err(bad(command, "a graph override names `graph`, or `body_index` with `body_name`")),
        }
    }
    let world = s.world_state();
    let frames = s.body_frames();
    let bodies: Vec<_> = world.bodies.iter().filter(|b| s.body_visible(&b.name)).collect();
    let mut out = CapturedGraphs::default();
    let mut index: BTreeMap<(GraphId, String), usize> = BTreeMap::new();
    let mut irs: BTreeMap<GraphId, Arc<Ir>> = BTreeMap::new();
    let mut bytes = Bytes { s, runtime: runtime.clone(), decoded: BTreeMap::new() };
    let mut material = |look: &Look, body: usize, out: &mut CapturedGraphs| -> Result<Option<usize>> {
        let Some(id) = &look.graph else { return Ok(None) };
        let graph = tables.graphs.get(id).ok_or_else(|| bad(command, format!("a look names missing material graph `{}`", id.as_str())))?;
        let ir = match irs.get(id) {
            Some(ir) => ir.clone(),
            None => {
                let ir = Arc::new(
                    mg::validate(graph).and_then(|v| mg::lower(&v)).map_err(|e| bad(command, format!("material graph `{}`: {e}", id.as_str())))?,
                );
                irs.insert(id.clone(), ir.clone());
                ir
            }
        };
        let mut values = look.values.clone();
        for o in [by_graph.get(id), by_body.get(&body)].into_iter().flatten() {
            check_override(graph, &ir, o, &known, command)?;
            values.extend(o.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let key = (id.clone(), serde_json::to_string(&values).unwrap_or_default());
        if let Some(&m) = index.get(&key) {
            return Ok(Some(m));
        }
        let mut textures = Vec::with_capacity(ir.textures.len());
        for binding in &ir.textures {
            let key = match binding.sha256() {
                Some(sha) => Some(sha.to_string()),
                None => {
                    let name = binding.source.strip_prefix(mg::ir::TextureBinding::INTERFACE_PREFIX).unwrap_or(&binding.source);
                    let given = values.get(name).or_else(|| graph.interface.iter().find(|i| i.name == name).and_then(|i| i.default.as_ref()));
                    match given {
                        Some(Constant::Texture(t)) => Some(t.texture.clone()),
                        _ => None,
                    }
                }
            };
            textures.push(match key {
                Some(key) => Some((bytes.get(&key, command)?, binding.srgb)),
                None => None,
            });
        }
        if out.materials.len() >= MAX_MATERIALS {
            return Err(bad(command, "too many graph materials in one job"));
        }
        out.materials.push(CapturedMaterial { graph: id.clone(), ir, values, textures });
        let m = out.materials.len() - 1;
        index.insert(key, m);
        Ok(Some(m))
    };
    for (i, mesh) in scene.meshes.iter().enumerate() {
        let Some(body) = bodies.get(i).filter(|b| b.name == mesh.name) else {
            return Err(bad(command, "the render scene and the model disagree; take a new render.scene"));
        };
        let placement = frames
            .iter()
            .find(|(world, _, _)| world == &body.name)
            .map(|(_, _, m)| std::array::from_fn::<_, 3, _>(|row| [m[0][row], m[1][row], m[2][row], m[3][row]]));
        let identity = |p: &[[f64; 4]; 3]| (0..3).all(|r| (0..4).all(|c| p[r][c] == if r == c { 1.0 } else { 0.0 }));
        let object_to_world = placement.filter(|p| !identity(p));
        let mut bind = |face: Option<u32>, look: Option<Look>, out: &mut CapturedGraphs| -> Result<()> {
            if let Some(m) = look.as_ref().map(|l| material(l, i, out)).transpose()?.flatten() {
                out.bindings.push(CapturedBinding { body_index: i, body_name: mesh.name.clone(), face, material: m, object_to_world });
            }
            Ok(())
        };
        bind(None, s.doc.model_body_look(body), &mut out)?;
        for (face, look) in s.doc.face_colors(body) {
            let face = u32::try_from(face).map_err(|_| bad(command, "render face index exceeds the mesh contract"))?;
            bind(Some(face), Some(look), &mut out)?;
        }
    }
    if out.materials.is_empty() {
        if !overrides.is_empty() {
            return Err(bad(command, "graph overrides were given, but no body in the scene shows a material graph"));
        }
        return Ok(None);
    }
    // Runtime textures nothing reads are a mistake in the request.
    if let Some(unused) = runtime.keys().find(|k| !out.materials.iter().flat_map(|m| &m.textures).flatten().any(|(b, _)| texture_key(b) == **k)) {
        return Err(bad(command, format!("runtime graph texture `{}` is not read by any override", short(unused))));
    }
    Ok(Some(out))
}

#[cfg(test)]
#[path = "graphs_tests.rs"]
mod tests;
