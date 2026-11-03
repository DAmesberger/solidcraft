//! Material-graph looks in the model view (docs/material-graph.md §5.2, §6): in the Realistic
//! view, the opaque faces whose look names a graph are shaded by it. Their triangles go into the
//! scene's material stream (`GpuScene::graphs`), which the renderer draws over the faces the
//! scene draws anyway (blocks, plans or instances), so those keep their depth, occlusion, edges
//! and hover and selection tints, and planned and instanced bodies keep their GPU paths.
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

/// Whether the model view draws graph looks: the Realistic view, with a GPU.
pub(crate) fn active(app: &crate::SolveApp) -> bool {
    app.viewport.gpu.is_some() && crate::view_look::current(app, "model") == crate::view_look::REALISTIC
}

/// The graph faces of body `b` into `sc.graphs`: each opaque face (`opaque(face)`) whose own look,
/// else the body's, names a graph, from `mesh` (world positions, also used as object positions).
/// Returns how many faces it added.
pub(crate) fn add_body(
    sc: &mut GpuScene,
    looks: &mut GraphLooks,
    doc: &solidcraft_engine::doc::Document,
    b: &solidcraft_engine::doc::ModelBody,
    face_looks: &HashMap<usize, solidcraft_engine::doc::appearance::Look>,
    mesh: &solidcraft_engine::geom::Mesh,
    opaque: impl Fn(usize) -> bool,
) -> usize {
    let graphs = &doc.appearances.tables.graphs;
    if graphs.is_empty() {
        return 0;
    }
    let body_look = doc.model_body_look(b);
    let mut materials: HashMap<usize, solidcraft_gpu::material::GraphMaterial> = HashMap::new();
    let mut by_look: HashMap<u64, Option<solidcraft_gpu::material::GraphMaterial>> = HashMap::new();
    for face in 0..b.body.face_count() {
        let Some(look) = face_looks.get(&face).or(body_look.as_ref()) else { continue };
        let Some(id) = look.graph.as_ref() else { continue };
        if !opaque(face) {
            continue;
        }
        let Some(graph) = graphs.get(id) else { continue };
        let key = content_key(graph, &look.values);
        let material = *by_look.entry(key).or_insert_with(|| {
            let l = looks.get(graph, &look.values)?;
            sc.graphs.material(&l.ir, &l.words, &l.base).ok()
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
