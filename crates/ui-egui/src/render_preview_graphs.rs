//! Graph and `pbr` looks in the Render workspace preview's Realistic look (docs/material-graph.md
//! §6): the same material stream as the modelling view's (`viewport_graphs`, `GpuScene::graphs`),
//! lit by the studio and the preview's own product lights.
//!
//! The preview's scene draws every face as before (its base response, `pbr` or colour), and the
//! stream shades the opaque graph and `pbr` faces over them, as in the modelling view. See-through
//! graph faces (a transmission or an opacity below 1) are drawn only by the stream, blended, so
//! the preview leaves them out of its own triangles. Faces the appearance-assign preview recolours
//! keep the preview's look: the stream leaves them alone. The Standard preview builds none of this.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use solidcraft_engine::render::ProductScene;

use crate::SolveApp;
use crate::gpu::GpuScene;

/// The stream's share of a preview scene: its material-graph surfaces, and per body the faces
/// only the stream draws.
#[derive(Default)]
pub(crate) struct PreviewGraphs {
    pub(crate) graphs: solidcraft_viewport_gpu::material::GraphScene,
    see_through: HashMap<String, Vec<bool>>,
    /// Per body, the faces the stream shades opaque over the preview's own triangles.
    shaded: HashMap<String, Vec<bool>>,
}

impl PreviewGraphs {
    /// Per face of body `body`, whether the stream alone draws it (a see-through graph face);
    /// `None` when it draws none of them.
    pub(crate) fn stream_only_faces(&self, body: &str) -> Option<&[bool]> {
        self.see_through.get(body).map(Vec::as_slice)
    }

    /// Per face of body `body`, whether the stream shades it opaque over the preview's own
    /// triangle; `None` when it shades none. The preview draws those faces before the stream, not
    /// as PBR triangles after it (which would cover it).
    pub(crate) fn shaded_faces(&self, body: &str) -> Option<&[bool]> {
        self.shaded.get(body).map(Vec::as_slice)
    }

    /// What the stream adds to the preview's scene key: its surfaces' records, vertices and
    /// textures, so a value edit that leaves the base response alone still draws a new picture.
    pub(crate) fn hash(&self, h: &mut impl Hasher) {
        for page in self.graphs.pages() {
            page.words.hash(h);
        }
        for draw in self.graphs.draws() {
            (draw.shape, draw.page, draw.textures, draw.object, draw.blended).hash(h);
            draw.vertices.hash(h);
        }
        self.graphs.textures().hash(h);
        for map in [&self.see_through, &self.shaded] {
            let mut bodies: Vec<_> = map.iter().collect();
            bodies.sort();
            bodies.hash(h);
        }
    }
}

/// Whether the Render preview draws graph looks: its own look is Realistic, with a GPU.
pub(crate) fn active(app: &SolveApp) -> bool {
    app.viewport.gpu.is_some() && crate::view_look::current(app, "render_preview") == crate::view_look::REALISTIC
}

/// The stream for `scene`, the preview's product scene, in the Realistic look; `None` in the
/// Standard look. `preview`'s bodies and faces (the appearance being tried on) are left to the
/// scene's own drawing.
pub(crate) fn build(app: &SolveApp, scene: &ProductScene, preview: Option<&crate::render_workspace::assign::Preview>) -> Option<PreviewGraphs> {
    if !active(app) {
        return None;
    }
    let doc = &app.session.doc;
    let world = crate::open_meshing::session_world(&app.session);
    let (tried_bodies, tried_faces): (HashSet<&str>, HashSet<(&str, usize)>) = preview.map_or_else(Default::default, |p| {
        let (bodies, faces) = p.targets();
        (bodies.iter().map(String::as_str).collect(), faces.iter().map(|(b, f)| (b.as_str(), *f)).collect())
    });
    let mut out = PreviewGraphs::default();
    let mut sc = GpuScene::default();
    let mut looks = app.viewport.graph_looks.lock().unwrap_or_else(|e| e.into_inner());
    for mesh in &scene.meshes {
        let Some(b) = world.body(&mesh.name) else { continue };
        let face_looks: HashMap<usize, solidcraft_engine::doc::appearance::Look> = doc.face_colors(b).into_iter().collect();
        let body_tried = tried_bodies.contains(mesh.name.as_str());
        let body_opacity = doc.model_body_opacity(b);
        let see_through = crate::viewport_graphs::see_through_faces(doc, b, &face_looks);
        let drawn = |f: usize| {
            if tried_faces.contains(&(mesh.name.as_str(), f)) || (body_tried && !face_looks.contains_key(&f)) {
                return false;
            }
            let opacity = face_looks.get(&f).map_or(body_opacity, |l| l.opacity);
            opacity >= 0.999 || see_through.get(f).copied().unwrap_or(false)
        };
        let only: Vec<bool> = (0..see_through.len()).map(|f| see_through.get(f).copied().unwrap_or(false) && drawn(f)).collect();
        let added = crate::viewport_graphs::add_body(&mut sc, &mut looks, doc, b, &face_looks, &mesh.mesh, drawn);
        let mut shaded = vec![false; see_through.len()];
        for f in added.into_iter().filter(|f| !see_through.get(*f).copied().unwrap_or(false)) {
            if let Some(s) = shaded.get_mut(f) {
                *s = true;
            }
        }
        if shaded.iter().any(|x| *x) {
            out.shaded.insert(mesh.name.clone(), shaded);
        }
        if only.iter().any(|x| *x) {
            out.see_through.insert(mesh.name.clone(), only);
        }
    }
    out.graphs = sc.graphs;
    Some(out)
}

#[cfg(test)]
#[path = "render_preview_graphs_tests.rs"]
mod tests;
