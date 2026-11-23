//! Appearance-graph looks in the model view (docs/appearance-graph.md §5.2, §6): in the Realistic
//! view, the opaque faces whose look names a graph are shaded by it. Their triangles go into the
//! scene's appearance stream (`GpuScene::graphs`), which the renderer draws over the faces the
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
use solidcraft_viewport::appearance_graph::{self as mg, AppearanceGraph, Ir, SurfaceSample};

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
    bytes: HashMap<String, Arc<Vec<u8>>>,
}

/// At most this many lowered looks are kept; past it the cache starts again.
const MAX_LOOKS: usize = mg::limits::GRAPHS_PER_DESIGN;

fn content_key(graph: &AppearanceGraph, values: &BTreeMap<String, Constant>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(graph).unwrap_or_default().hash(&mut h);
    serde_json::to_string(values).unwrap_or_default().hash(&mut h);
    h.finish()
}

fn lower(graph: &AppearanceGraph, values: &BTreeMap<String, Constant>) -> Result<Lowered, String> {
    let valid = mg::validate(graph).map_err(|e| e.to_string())?;
    let ir = mg::lower(&valid).map_err(|e| e.to_string())?;
    // The look's values, not the graph's defaults: this is what shows until the shape compiles.
    let base = mg::look_base_sample(graph, values).map_err(|e| e.to_string())?;
    let words = mg::uniform_words(&ir, values).map_err(|e| e.to_string())?;
    Ok(Lowered { ir: Arc::new(ir), words, base })
}

/// Texture bindings from graph defaults and a particular look’s overrides.
pub(crate) fn texture_set(graph: &AppearanceGraph, ir: &Ir, values: &BTreeMap<String, Constant>) -> solidcraft_viewport_gpu::appearance::TextureSet {
    let mut resolved: BTreeMap<_, _> =
        graph.interface.iter().filter_map(|input| input.default.clone().map(|value| (input.name.clone(), value))).collect();
    resolved.extend(values.iter().map(|(name, value)| (name.clone(), value.clone())));
    solidcraft_viewport_gpu::appearance::texture_keys(ir, &resolved)
}

impl GraphLooks {
    /// A texture asset's encoded bytes from the design, read once.
    pub(crate) fn texture_bytes(&mut self, doc: &solidcraft_engine::doc::Document, sha256: &str) -> Option<Arc<Vec<u8>>> {
        let asset = doc.appearances.tables.textures.get(sha256)?;
        if let Some(b) = self.bytes.get(sha256) {
            return Some(b.clone());
        }
        if self.bytes.len() >= mg::limits::TEXTURES_PER_DESIGN {
            self.bytes.clear();
        }
        let bytes = Arc::new(asset.bytes(sha256).ok()?);
        self.bytes.insert(sha256.to_owned(), bytes.clone());
        Some(bytes)
    }

    /// The lowered graph look, or `None` when it cannot draw as a graph.
    fn get(&mut self, graph: &AppearanceGraph, values: &BTreeMap<String, Constant>) -> Option<Arc<Lowered>> {
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
                    log::warn!("appearance graph {:?} draws as its look's colour: {e}", graph.name);
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
/// the appearance stream, blended; the standard path draws them as invisible glass.
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
pub(crate) fn pbr_graph(color: [u8; 3], pbr: &solidcraft_engine::doc::pbr_appearance::PbrParameters) -> Option<AppearanceGraph> {
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
    crate::realistic_view::enabled(app)
}

/// The graph faces of body `b` into `sc.graphs`: each face `drawn(face)` keeps whose own look,
/// else the body's, names a graph, from the world-space display `mesh`. Object coordinates follow the definition placement.
/// Returns the faces it added.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_body<E>(
    sc: &mut GpuScene,
    looks: &mut GraphLooks,
    doc: &solidcraft_engine::doc::Document,
    b: &solidcraft_engine::doc::ModelBody,
    face_looks: &HashMap<usize, solidcraft_engine::doc::appearance::Look>,
    mesh: impl FnOnce() -> Arc<solidcraft_engine::geom::Mesh>,
    drawn: impl Fn(usize) -> bool,
    mut admit_triangle: impl FnMut(usize) -> Result<(), E>,
) -> Result<Vec<usize>, E> {
    let graphs = &doc.appearances.tables.graphs;
    let body_look = doc.model_body_look(b);
    let mut by_look: HashMap<u64, Option<(solidcraft_viewport_gpu::appearance::GraphAppearance, usize)>> = HashMap::new();
    let mut appearance = |look: &solidcraft_engine::doc::appearance::Look| {
        let pbr;
        let graph = match (look.graph.as_ref(), look.pbr.as_ref()) {
            (Some(id), _) => graphs.get(id)?,
            (None, Some(p)) => {
                pbr = pbr_graph(look.color, p)?;
                &pbr
            }
            (None, None) => return None,
        };
        let key = content_key(graph, &look.values);
        *by_look.entry(key).or_insert_with(|| {
            let l = looks.get(graph, &look.values)?;
            let set = texture_set(graph, &l.ir, &look.values);
            for k in set.iter().flatten() {
                if !sc.graphs.texture_sources().contains_key(k)
                    && let Some(bytes) = looks.texture_bytes(doc, &k.sha256)
                {
                    sc.graphs.texture_source(k.clone(), bytes);
                }
            }
            let appearance = sc.graphs.textured_appearance(&l.ir, &l.words, &l.base, &set).ok()?;
            Some((appearance, solidcraft_viewport_gpu::appearance::GraphScene::stride(l.ir.object_space)))
        })
    };
    // A body with one finish might have thousands of faces. Lower its inherited look
    // once and only request a display mesh if at least one face needs a graph.
    let inherited = std::cell::OnceCell::new();
    let appearances: Vec<_> = (0..b.body.face_count())
        .map(|face| {
            if !drawn(face) {
                return None;
            }
            match face_looks.get(&face) {
                Some(look) => appearance(look),
                None => *inherited.get_or_init(|| body_look.as_ref().and_then(&mut appearance)),
            }
        })
        .collect();
    let added: Vec<_> = appearances.iter().enumerate().filter_map(|(face, look)| look.is_some().then_some(face)).collect();
    if added.is_empty() {
        return Ok(added);
    }
    let mesh = mesh();
    let to_object =
        (b.source_transform != solidcraft_engine::doc::IDENTITY).then(|| solidcraft_engine::doc::mat_inverse(&b.source_transform)).flatten();
    for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
        let Some((m, stride)) = usize::try_from(*f).ok().and_then(|f| appearances.get(f)).copied().flatten() else { continue };
        let corners = t.map(|index| mesh.positions.get(index as usize).zip(mesh.normals.get(index as usize)));
        if let [Some(a), Some(b), Some(c)] = corners {
            admit_triangle(3 * stride)?;
            for (&position, normal) in [a, b, c] {
                let object = to_object.as_ref().map_or(position, |matrix| solidcraft_engine::doc::apply_point(matrix, position));
                sc.graphs.vertex(m, position.to_f32(), normal.to_f32(), object.to_f32());
            }
        }
    }
    Ok(added)
}
