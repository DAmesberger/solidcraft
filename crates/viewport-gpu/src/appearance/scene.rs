//! The CPU side of appearance-graph drawing (docs/appearance-graph.md §5.2): uniform pages, each
//! material's record in one, and the vertices of every (shape, page), which one draw
//! covers. Data preparation only.

use std::collections::HashMap;
use std::sync::Arc;

use solidcraft_viewport::appearance_graph::ir::{Ir, ShapeKey};
use solidcraft_viewport::appearance_graph::{SurfaceSample, limits};

use super::{BASE_VEC4, OBJECT_VERTEX_SIZE, PAGE_VEC4, VERTEX_SIZE};

/// Pages one scene may hold: 16 MiB of uniforms, far above a design's 4096 graphs.
pub const MAX_PAGES: usize = 1024;
/// A material in a [`GraphScene`]: where its record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GraphAppearance(u32);

/// Where a material lives, for a draw of placed meshes ([`GraphScene::placed_appearance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlacedAppearance {
    pub shape: ShapeKey,
    pub page: u32,
    pub object: bool,
    pub record: u32,
}

/// Why a material cannot join a scene.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphSceneError {
    #[error("the material's uniform words ({given}) do not match its graph ({expected})")]
    Words { given: usize, expected: usize },
    #[error("the scene holds at most {MAX_PAGES} uniform pages of materials")]
    Pages,
}

/// One uniform page: `PAGE_VEC4` vec4s, as `u32` words (`f32` bits).
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub words: Vec<u32>,
    /// vec4s in use from the start.
    used: u32,
}

/// The vertices one draw covers: every material of one shape on one page.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphDraw {
    pub shape: ShapeKey,
    pub page: u32,
    /// [`OBJECT_VERTEX_SIZE`] vertices (the shape reads object space), else [`VERTEX_SIZE`].
    pub object: bool,
    pub vertices: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Entry {
    draw: usize,
    record: u32,
    object: bool,
}

/// The appearance-graph surfaces of a scene.
#[derive(Clone, Debug, Default)]
pub struct GraphScene {
    shapes: HashMap<ShapeKey, Arc<Ir>>,
    pages: Vec<Page>,
    draws: Vec<GraphDraw>,
    draw_index: HashMap<(ShapeKey, u32), usize>,
    materials: Vec<Entry>,
    /// Identical records (same shape and words) share one.
    dedup: HashMap<(ShapeKey, Vec<u32>), GraphAppearance>,
}

/// The base response as the head of a record ([`BASE_VEC4`] vec4s, `mat_base_response`):
/// (base colour, metalness), (roughness, IOR, coat weight, coat roughness), (coat IOR,
/// specular weight, anisotropy, rotation), (specular colour, emission luminance), (coat colour,
/// transmission weight), (emission colour, transmission depth), (transmission colour, opacity).
pub fn base_record(s: &SurfaceSample) -> [f32; 4 * BASE_VEC4 as usize] {
    let o = s.opaque;
    let (cw, cr, ci) = o.clear_coat().map_or((0.0, 1.0, 1.0), |c| (c.weight(), c.roughness(), c.ior()));
    let b = o.base_color();
    let f = |x: f64| x as f32;
    let v = |c: solidcore_record::Vec3| [f(c.x), f(c.y), f(c.z)];
    let [sr, sg, sb] = v(s.specular_color);
    let [cr_, cg, cb] = v(s.coat_color);
    let [er, eg, eb] = v(s.emission_color);
    let [tr, tg, tb] = v(s.transmission_color);
    [
        f(b.x),
        f(b.y),
        f(b.z),
        f(o.metallic()),
        f(o.roughness()),
        f(o.dielectric_ior()),
        f(cw),
        f(cr),
        f(ci),
        f(s.specular_weight),
        f(s.anisotropy),
        f(s.rotation),
        sr,
        sg,
        sb,
        f(s.emission_luminance),
        cr_,
        cg,
        cb,
        f(s.transmission_weight),
        er,
        eg,
        eb,
        f(s.transmission_depth_mm),
        tr,
        tg,
        tb,
        f(s.opacity),
    ]
}

impl GraphScene {
    /// Add a material of graph `ir` with its uniform `words` (`uniform_words`) and base response
    /// `base` (`base_sample`). Equal materials share a record.
    pub fn appearance(&mut self, ir: &Arc<Ir>, words: &[u32], base: &SurfaceSample) -> Result<GraphAppearance, GraphSceneError> {
        let expected = usize::try_from(ir.uniform_floats).unwrap_or(usize::MAX).min(limits::UNIFORM_FLOATS);
        if words.len() != expected {
            return Err(GraphSceneError::Words { given: words.len(), expected });
        }
        let mut record: Vec<u32> = base_record(base).iter().map(|x| x.to_bits()).collect();
        record.extend_from_slice(words);
        record.resize(record.len().div_ceil(4) * 4, 0);
        let key = (ir.shape, record);
        if let Some(m) = self.dedup.get(&key) {
            return Ok(*m);
        }
        let record = key.1.clone();
        let vec4s = u32::try_from(record.len() / 4).unwrap_or(u32::MAX);
        let page = match self.pages.iter().position(|p| p.used + vec4s <= PAGE_VEC4) {
            Some(p) => p,
            None => {
                if self.pages.len() >= MAX_PAGES {
                    return Err(GraphSceneError::Pages);
                }
                self.pages.push(Page { words: vec![0; PAGE_VEC4 as usize * 4], used: 0 });
                self.pages.len() - 1
            }
        };
        let Some(p) = self.pages.get_mut(page) else { return Err(GraphSceneError::Pages) };
        let start = p.used;
        for (i, w) in record.iter().enumerate() {
            if let Some(slot) = p.words.get_mut(start as usize * 4 + i) {
                *slot = *w;
            }
        }
        p.used += vec4s;
        self.shapes.entry(ir.shape).or_insert_with(|| ir.clone());
        let page = u32::try_from(page).unwrap_or(u32::MAX);
        let draw = *self.draw_index.entry((ir.shape, page)).or_insert_with(|| {
            self.draws.push(GraphDraw { shape: ir.shape, page, object: ir.object_space, vertices: Vec::new() });
            self.draws.len() - 1
        });
        let m = GraphAppearance(u32::try_from(self.materials.len()).unwrap_or(u32::MAX));
        self.materials.push(Entry { draw, record: start, object: ir.object_space });
        self.dedup.insert(key, m);
        Ok(m)
    }

    /// One vertex of material `m`: world position and normal, and its object-space position
    /// (read only by shapes that use object space). Triangles are three consecutive vertices of
    /// one material.
    pub fn vertex(&mut self, m: GraphAppearance, p: [f32; 3], n: [f32; 3], object: [f32; 3]) {
        let Some(entry) = usize::try_from(m.0).ok().and_then(|i| self.materials.get(i)) else { return };
        let (record, object_space) = (entry.record, entry.object);
        let Some(draw) = self.draws.get_mut(entry.draw) else { return };
        let v = &mut draw.vertices;
        for x in p.into_iter().chain(n) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&record.to_le_bytes());
        if object_space {
            for x in object {
                v.extend_from_slice(&x.to_le_bytes());
            }
        }
    }

    /// Where material `m` lives.
    pub(crate) fn placed_appearance(&self, m: GraphAppearance) -> Option<PlacedAppearance> {
        let entry = usize::try_from(m.0).ok().and_then(|i| self.materials.get(i))?;
        let draw = self.draws.get(entry.draw)?;
        Some(PlacedAppearance { shape: draw.shape, page: draw.page, object: draw.object, record: entry.record })
    }

    pub fn is_empty(&self) -> bool {
        self.draws.iter().all(|d| d.vertices.is_empty())
    }

    pub fn pages(&self) -> &[Page] {
        &self.pages
    }

    pub fn draws(&self) -> &[GraphDraw] {
        &self.draws
    }

    /// The IR of shape `key`.
    pub fn shape(&self, key: &ShapeKey) -> Option<&Arc<Ir>> {
        self.shapes.get(key)
    }

    /// Bytes per vertex of a draw.
    pub fn stride(object: bool) -> usize {
        if object { OBJECT_VERTEX_SIZE } else { VERTEX_SIZE }
    }
}
