//! Material-graph looks in the model view (docs/material-graph.md §5.2, §6): in the Realistic
//! view, the opaque faces whose look names a graph are shaded by it. Their triangles go into the
//! scene's material stream (`GpuScene::graphs`), which the renderer draws over the faces the
//! scene draws anyway (blocks, plans or instances), so those keep their depth, occlusion, edges
//! and hover and selection tints, and planned and instanced bodies keep their GPU paths.
//!
//! A `pbr` look is, in memory, the one-node constant graph of decision O9 ([`pbr_graph`]): its
//! metals reflect the studio here as they do in the Render preview, and every `pbr` look shares
//! that graph's one shape (its values are uniforms).
//!
//! A look reads its graph from `appearances.graphs` by id, with its `values` for the graph's
//! interface. A graph that is missing or does not validate draws as before: the look's `color`
//! (logged once). Lowering is cached by the graph's and values' content, so a value edit costs
//! one `uniform_words`, and the renderer then rewrites only the changed uniform page.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use solidcraft_engine::doc::appearance_graph::Constant;
use solidcraft_engine::render::material_graph::{self as mg, Ir, MaterialGraph, SurfaceSample};

use crate::gpu::GpuScene;

/// A look's graph, lowered once: its IR, its uniform words for the look's values, its base
/// response.
pub(crate) struct Lowered {
    ir: Arc<Ir>,
    words: Vec<u32>,
    base: SurfaceSample,
}

/// Lowered graph looks by content (graph and values), kept across scenes.
#[derive(Default)]
pub(crate) struct GraphLooks {
    lowered: HashMap<u64, Option<Arc<Lowered>>>,
    /// Content keys already reported as failing.
    warned: HashSet<u64>,
    /// Texture assets' encoded bytes by SHA-256 (content-addressed, so kept across designs).
    bytes: HashMap<String, Option<Arc<Vec<u8>>>>,
}

/// At most this many lowered looks are kept; past it the cache starts again.
const MAX_LOOKS: usize = 4096;

fn content_key(graph: &MaterialGraph, values: &BTreeMap<String, Constant>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(graph).unwrap_or_default().hash(&mut h);
    serde_json::to_string(values).unwrap_or_default().hash(&mut h);
    h.finish()
}

fn lower(graph: &MaterialGraph, values: &BTreeMap<String, Constant>) -> Result<Lowered, String> {
    let valid = mg::validate(graph).map_err(|e| e.to_string())?;
    let ir = mg::lower(&valid).map_err(|e| e.to_string())?;
    let base = mg::base_sample(&valid).map_err(|e| e.to_string())?;
    let words = mg::uniform_words(&ir, values).map_err(|e| e.to_string())?;
    Ok(Lowered { ir: Arc::new(ir), words, base })
}

impl GraphLooks {
    /// A texture asset's encoded bytes from the design, read once.
    fn texture_bytes(&mut self, doc: &solidcraft_engine::doc::Document, sha256: &str) -> Option<Arc<Vec<u8>>> {
        if let Some(b) = self.bytes.get(sha256) {
            return b.clone();
        }
        if self.bytes.len() >= MAX_LOOKS {
            self.bytes.clear();
        }
        let bytes = doc.appearances.tables.textures.get(sha256).and_then(|t| t.bytes(sha256).ok()).map(Arc::new);
        self.bytes.insert(sha256.to_owned(), bytes.clone());
        bytes
    }

    /// The lowered graph look, or `None` when it cannot draw as a graph.
    fn get(&mut self, graph: &MaterialGraph, values: &BTreeMap<String, Constant>) -> Option<Arc<Lowered>> {
        let key = content_key(graph, values);
        if let Some(l) = self.lowered.get(&key) {
            return l.clone();
        }
        if self.lowered.len() >= MAX_LOOKS {
            self.lowered.clear();
        }
        let lowered = match lower(graph, values) {
            Ok(l) => Some(Arc::new(l)),
            Err(e) => {
                if self.warned.len() < MAX_LOOKS && self.warned.insert(key) {
                    log::warn!("material graph {:?} draws as its look's colour: {e}", graph.name);
                }
                None
            }
        };
        self.lowered.insert(key, lowered.clone());
        lowered
    }
}

/// Per face of `b`: whether its graph look (its own, else the body's) is see-through (a
/// transmission or an opacity below 1, `appearance_graph::see_through`). Those faces are drawn by
/// the material stream, blended; the standard path draws them as invisible glass.
pub(crate) fn see_through_faces(
    doc: &solidcraft_engine::doc::Document,
    b: &solidcraft_engine::doc::ModelBody,
    face_looks: &HashMap<usize, solidcraft_engine::doc::appearance::Look>,
) -> Vec<bool> {
    let graphs = &doc.appearances.tables.graphs;
    let faces = b.body.face_count();
    if graphs.is_empty() {
        return vec![false; faces];
    }
    let body_look = doc.model_body_look(b);
    (0..faces)
        .map(|f| {
            face_looks.get(&f).or(body_look.as_ref()).is_some_and(|look| {
                look.graph.as_ref().and_then(|id| graphs.get(id)).is_some_and(|g| solidcraft_engine::doc::appearance_graph::see_through(g, look))
            })
        })
        .collect()
}

/// A `pbr` look as the constant graph it is (O9): one `standard_surface` with the look's colour
/// (exact sRGB8), metalness, roughness and dielectric IOR. Always the same four inputs, so every
/// `pbr` look has the same shape.
pub(crate) fn pbr_graph(color: [u8; 3], pbr: &solidcraft_engine::doc::pbr_appearance::PbrParameters) -> Option<MaterialGraph> {
    let graph = serde_json::json!({"name": "pbr", "nodes": [{"id": "surface", "kind": "standard_surface", "inputs": {
        "base_color": {"srgb8": color},
        "base_metalness": pbr.metallic(),
        "specular_roughness": pbr.roughness(),
        "specular_ior": pbr.dielectric_ior(),
    }}], "output": "surface"});
    serde_json::from_value(graph).ok()
}

/// Whether the model view draws graph looks: the Realistic view, with a GPU.
pub(crate) fn active(app: &crate::SolveApp) -> bool {
    app.viewport.gpu.is_some() && crate::view_look::current(app, "model") == crate::view_look::REALISTIC
}

/// The graph faces of body `b` into `sc.graphs`: each face `drawn(face)` keeps whose own look,
/// else the body's, names a graph, from `mesh` (world positions, also used as object positions).
/// Returns how many faces it added.
pub(crate) fn add_body(
    sc: &mut GpuScene,
    looks: &mut GraphLooks,
    doc: &solidcraft_engine::doc::Document,
    b: &solidcraft_engine::doc::ModelBody,
    face_looks: &HashMap<usize, solidcraft_engine::doc::appearance::Look>,
    mesh: &solidcraft_engine::geom::Mesh,
    drawn: impl Fn(usize) -> bool,
) -> usize {
    let graphs = &doc.appearances.tables.graphs;
    let body_look = doc.model_body_look(b);
    let mut materials: HashMap<usize, solidcraft_gpu::material::GraphMaterial> = HashMap::new();
    let mut by_look: HashMap<u64, Option<solidcraft_gpu::material::GraphMaterial>> = HashMap::new();
    for face in 0..b.body.face_count() {
        let Some(look) = face_looks.get(&face).or(body_look.as_ref()) else { continue };
        if !drawn(face) {
            continue;
        }
        let pbr;
        let graph = match (look.graph.as_ref(), look.pbr.as_ref()) {
            (Some(id), _) => match graphs.get(id) {
                Some(g) => g,
                None => continue,
            },
            (None, Some(p)) => match pbr_graph(look.color, p) {
                Some(g) => {
                    pbr = g;
                    &pbr
                }
                None => continue,
            },
            (None, None) => continue,
        };
        let key = content_key(graph, &look.values);
        let material = *by_look.entry(key).or_insert_with(|| {
            let l = looks.get(graph, &look.values)?;
            // The textures it reads: constants, and interface inputs from the look's values over
            // the graph's defaults.
            let mut values: BTreeMap<String, Constant> =
                graph.interface.iter().filter_map(|i| i.default.clone().map(|d| (i.name.clone(), d))).collect();
            values.extend(look.values.iter().map(|(k, v)| (k.clone(), v.clone())));
            let set = solidcraft_gpu::material::texture_keys(&l.ir, &values);
            for k in set.iter().flatten() {
                if !sc.graphs.texture_sources().contains_key(k)
                    && let Some(bytes) = looks.texture_bytes(doc, &k.sha256)
                {
                    sc.graphs.texture_source(k.clone(), bytes);
                }
            }
            sc.graphs.textured_material(&l.ir, &l.words, &l.base, &set).ok()
        });
        if let Some(m) = material {
            materials.insert(face, m);
        }
    }
    if materials.is_empty() {
        return 0;
    }
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let Some(m) = usize::try_from(*f).ok().and_then(|f| materials.get(&f)).copied() else { continue };
        let corners: Option<Vec<([f32; 3], [f32; 3])>> =
            t.iter().map(|k| Some((mesh.positions.get(*k as usize)?.to_f32(), mesh.normals.get(*k as usize)?.to_f32()))).collect();
        for (p, n) in corners.unwrap_or_default() {
            sc.graphs.vertex(m, p, n, p);
        }
    }
    materials.len()
}
