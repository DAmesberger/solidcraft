//! Material graphs captured for one GPU path job (docs/material-graph.md §5.7): the path tracer's
//! half of "one IR, two GPU back ends". A job carries each graph look's lowered [`Ir`], its uniform
//! words (with the look's values and any runtime overrides already applied) and its decoded
//! textures, and binds them to bodies or faces of the job's [`ProductScene`].
//!
//! Nothing here shades: the path tracer generates WGSL from the IR (`solidcraft-gpu`), and the
//! scene's own `surface` of a bound body or face stays its base response, which every other
//! renderer draws. The path tracer has no lobe for some inputs; [`GraphMaterial::approximations`]
//! names each one it approximates, and the job's request reports them (decision O10).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use super::{CancelToken, ErrorCode, JobError, MAX_GPU_PATH_BODIES, Result};
use crate::ProductScene;
use crate::material_graph::ir::{Ir, Operand, ShapeKey};
use crate::material_graph::{Constant, limits, uniform_words};
use crate::texture_input::{MapRole, TextureSource, role_of};
use sha2::Digest as _;

/// Graph materials (a graph with one look's values) one job may hold.
pub const MAX_GRAPH_MATERIALS: usize = 4096;
/// Distinct shapes one job compiles into its path pipeline.
pub const MAX_GRAPH_SHAPES: usize = 32;
/// Body or face bindings one job may hold.
pub const MAX_GRAPH_BINDINGS: usize = 65_536;
/// Decoded linear pixel bytes of a job's unique graph textures (RGBA `f32`).
pub const MAX_GRAPH_PIXEL_BYTES: u64 = 256 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> JobError {
    JobError::new(ErrorCode::InvalidScene, message)
}

/// A texture an `image` node reads, admitted as the viewport admits it (`texture_input`, the file's
/// safety bounds and the role its use implies) and decoded at the path tracer's resident size.
#[derive(Clone)]
pub struct GraphTexture {
    source: Arc<TextureSource>,
    srgb: bool,
    digest: [u8; 32],
}

impl std::fmt::Debug for GraphTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphTexture").field("info", self.source.info()).field("srgb", &self.srgb).finish()
    }
}

/// The largest resident edge of a graph texture in any path job: the path tracer's resident clamp
/// (`material::textures::ResidentPolicy::path`) at its largest budget. A device with a smaller
/// storage binding drops more.
pub fn resident_edge() -> u32 {
    u32::try_from((super::textures::MAX_TEXTURE_PIXELS_BYTES / 16).isqrt()).unwrap_or(1)
}

impl GraphTexture {
    /// An encoded image (PNG or JPEG) for an `image` node in colour space `srgb` (decoded per
    /// texel before filtering), linear data, or 16-bit grey height (`texture_input::role_of`).
    pub fn new(bytes: Vec<u8>, srgb: bool) -> Result<Self> {
        let role = role_of(&bytes, srgb);
        let digest = sha2::Sha256::digest(&bytes).into();
        let source = TextureSource::new(bytes, role).map_err(|e| invalid(e.to_string()))?;
        Ok(Self { source: Arc::new(source), srgb, digest })
    }
    pub fn source(&self) -> &TextureSource {
        &self.source
    }
    pub fn srgb(&self) -> bool {
        self.srgb
    }
    /// Content plus interpretation: equal identities share one decode in a job's texture buffer,
    /// with runtime maps as well (`TextureMap::identity`'s role numbering).
    pub fn identity(&self) -> ([u8; 32], u8) {
        let role = match self.source.info().role {
            MapRole::BaseColor => 0,
            MapRole::NormalOpenGl => 1,
            MapRole::Roughness => 2,
            MapRole::Metallic => 3,
            MapRole::AmbientOcclusion => 4,
            MapRole::Height => 5,
        };
        (self.digest, role)
    }
    /// The linear RGBA `f32` bytes resident at most: halved until it fits [`resident_edge`].
    fn pixel_bytes(&self) -> u64 {
        let info = self.source.info();
        let (mut w, mut h) = (info.width.max(1), info.height.max(1));
        while w > resident_edge() || h > resident_edge() {
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }
        u64::from(w) * u64::from(h) * 16
    }
}

/// An input of the graph's surface the path tracer has no lobe for, and what it does instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Approximation {
    /// The `standard_surface` input (OpenPBR name).
    pub input: &'static str,
    /// What the path tracer does: `isotropic`, `untinted`, `ignored`, `base_normal`, `not_emitted`,
    /// `full_glass` (a constant transmission weight below 1), `captured_transmission` (a driven
    /// weight: the scene's own transmission applies) or `opaque`.
    pub treatment: &'static str,
}

/// When an input is active, and what the path tracer does with it instead.
struct Lacking {
    input: &'static str,
    /// Its neutral constant: an input at this value needs no approximation.
    neutral: &'static [f32],
    treatment: &'static str,
}

const LACKING: &[Lacking] = &[
    Lacking { input: "specular_roughness_anisotropy", neutral: &[0.0], treatment: "isotropic" },
    Lacking { input: "coat_color", neutral: &[1.0, 1.0, 1.0], treatment: "untinted" },
    Lacking { input: "specular_weight", neutral: &[1.0], treatment: "ignored" },
    Lacking { input: "specular_color", neutral: &[1.0, 1.0, 1.0], treatment: "ignored" },
    Lacking { input: "geometry_coat_normal", neutral: &[], treatment: "base_normal" },
    Lacking { input: "emission_luminance", neutral: &[0.0], treatment: "not_emitted" },
    // A constant weight above 0 is glass (`GraphMaterial::see_through`), all or nothing; a driven
    // weight keeps the scene's transmission.
    Lacking { input: "transmission_weight", neutral: &[0.0], treatment: "captured_transmission" },
    Lacking { input: "geometry_opacity", neutral: &[1.0], treatment: "opaque" },
];

/// One graph with one look's values: what a bound face is shaded with.
#[derive(Clone, Debug)]
pub struct GraphMaterial {
    ir: Arc<Ir>,
    words: Vec<u32>,
    textures: Vec<Option<GraphTexture>>,
    approximations: Vec<Approximation>,
}

impl GraphMaterial {
    /// A lowered graph with a look's interface `values` (and runtime overrides) and, per
    /// [`Ir::textures`] entry, its texture (`None`: the `image` node's `default` colour).
    pub fn new(ir: Arc<Ir>, values: &BTreeMap<String, Constant>, textures: Vec<Option<GraphTexture>>) -> Result<Self> {
        let words = uniform_words(&ir, values).map_err(|e| invalid(format!("graph material values: {e}")))?;
        if words.len() > limits::UNIFORM_FLOATS || ir.textures.len() > limits::TEXTURES_PER_GRAPH {
            return Err(invalid("graph material exceeds the uniform or texture limits"));
        }
        if textures.len() != ir.textures.len() {
            return Err(invalid("graph material needs one texture entry per image binding"));
        }
        if textures.iter().zip(&ir.textures).any(|(t, b)| t.as_ref().is_some_and(|t| t.srgb != b.srgb)) {
            return Err(invalid("graph texture colour space does not match its image node"));
        }
        let approximations = approximations(&ir, &words);
        Ok(Self { ir, words, textures, approximations })
    }
    pub fn ir(&self) -> &Ir {
        &self.ir
    }
    /// The material's uniform words, `ir().uniform_floats` of them (`f32` bits; seeds as `u32`).
    pub fn words(&self) -> &[u32] {
        &self.words
    }
    pub fn textures(&self) -> &[Option<GraphTexture>] {
        &self.textures
    }
    /// The surface inputs this material drives or sets away from neutral that the path tracer
    /// has no lobe for (decision O10). Empty when the path tracer evaluates it exactly.
    pub fn approximations(&self) -> &[Approximation] {
        &self.approximations
    }
    /// The glass a see-through material gives its body or face in the path tracer, whose
    /// transmission is all or nothing: for a constant `transmission_weight` above 0, closed
    /// glass whose absorption gives `transmission_color` after `transmission_depth_mm` (Beer's
    /// law). `None` for an opaque material, and for a driven weight, which keeps the scene's own
    /// transmission (reported as `captured_transmission`).
    pub fn see_through(&self) -> Option<crate::transmission::Transmission> {
        let find = |name: &str| self.ir.surface.iter().find(|(n, _)| *n == name).map(|(_, o)| *o).unwrap_or(Operand::Absent);
        let read = |name: &str, default: &[f64]| -> Option<Vec<f64>> {
            match find(name) {
                Operand::Absent => Some(default.to_vec()),
                Operand::Uniform { offset, width } => {
                    let start = usize::try_from(offset).ok()?;
                    (0..usize::from(width)).map(|k| self.words.get(start + k).map(|w| f64::from(f32::from_bits(*w)))).collect()
                }
                _ => None,
            }
        };
        let weight = read("transmission_weight", &[0.0])?.first().copied()?;
        if !(weight > 0.0) {
            return None;
        }
        let color = read("transmission_color", &[1.0, 1.0, 1.0])?;
        let depth = read("transmission_depth_mm", &[1.0])?.first().copied()?.max(1e-3);
        let a = |k: usize| -color.get(k).copied().unwrap_or(1.0).clamp(1e-4, 1.0).ln() / depth;
        crate::transmission::Transmission::new(solidcraft_geom::Vec3::new(a(0), a(1), a(2)), None).ok()
    }
}

/// Whether `op` makes its input differ from `neutral`: a wire always may, a constant when its
/// words differ, an unset input never (its default is neutral).
fn active(op: &Operand, neutral: &[f32], words: &[u32]) -> bool {
    match op {
        Operand::Absent => false,
        Operand::Uniform { offset, width } => {
            let start = usize::try_from(*offset).unwrap_or(usize::MAX);
            (0..usize::from(*width)).any(|k| {
                let word = start.checked_add(k).and_then(|i| words.get(i)).copied().unwrap_or(0);
                let want = neutral.get(k).or(neutral.first()).copied().unwrap_or(0.0);
                f32::from_bits(word) != want
            })
        }
        _ => true,
    }
}

/// A scalar operand's constant value: its uniform word, or none for a wire or an unset input.
fn constant(op: &Operand, words: &[u32]) -> Option<f32> {
    match op {
        Operand::Uniform { offset, width: 1 } => usize::try_from(*offset).ok().and_then(|i| words.get(i)).map(|w| f32::from_bits(*w)),
        _ => None,
    }
}

fn approximations(ir: &Ir, words: &[u32]) -> Vec<Approximation> {
    let layers = std::iter::once(ir.surface.as_slice()).chain(ir.mix_layers.iter().map(|l| l.fg.as_slice()));
    let mut out: Vec<Approximation> = Vec::new();
    for surface in layers {
        let find = |name: &str| surface.iter().find(|(n, _)| *n == name).map(|(_, o)| *o).unwrap_or(Operand::Absent);
        for lacking in LACKING {
            let op = find(lacking.input);
            let mut treatment = lacking.treatment;
            let on = if lacking.input == "geometry_coat_normal" {
                // One shading normal: a coat normal only differs when it is not the base's.
                op != Operand::Absent && op != find("geometry_normal")
            } else if lacking.input == "transmission_weight" {
                // A constant weight is glass (`see_through`): exact at 1, full glass below it.
                match constant(&op, words) {
                    Some(w) => {
                        treatment = "full_glass";
                        w > 0.0 && w < 1.0
                    }
                    None => active(&op, lacking.neutral, words),
                }
            } else {
                active(&op, lacking.neutral, words)
            };
            if on && !out.iter().any(|a| a.input == lacking.input) {
                out.push(Approximation { input: lacking.input, treatment });
            }
        }
    }
    out.sort_by_key(|a| LACKING.iter().position(|l| l.input == a.input));
    out
}

/// A body (or one of its faces) shaded by a graph material.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GraphBinding {
    pub body_index: usize,
    /// The exact `render.scene` body name, as runtime textures bind.
    pub body_name: String,
    /// One B-rep face of the body, or `None` for every face without its own binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face: Option<u32>,
    /// Index into [`GraphMaterials::materials`].
    pub material: usize,
    /// The body's placement, rows of `world = M·object + t` (`[M | t]`, mm), so object-space
    /// projections stick to an occurrence. `None`: object space is world space.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_to_world: Option<[[f64; 4]; 3]>,
}

/// Graph materials and their bindings, captured for one job. Serialised as its summary only.
#[derive(Clone, Debug)]
pub struct GraphMaterials {
    materials: Vec<GraphMaterial>,
    bindings: Vec<GraphBinding>,
    shapes: Vec<ShapeKey>,
    pixel_bytes: u64,
}

/// What a job's graph materials are, for `render.preflight` and the job result.
#[derive(Clone, Debug, Serialize)]
pub struct GraphSummary {
    pub materials: usize,
    pub shapes: usize,
    pub bindings: usize,
    pub linear_pixel_bytes: u64,
    /// Per binding whose material the path tracer approximates: the inputs and treatments.
    pub approximations: Vec<BindingApproximations>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BindingApproximations {
    pub body_index: usize,
    pub body_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face: Option<u32>,
    pub inputs: Vec<Approximation>,
}

fn finite_placement(m: &[[f64; 4]; 3]) -> bool {
    if m.iter().flatten().any(|x| !x.is_finite() || x.abs() > 1e7) {
        return false;
    }
    let r = |i: usize| [m[i][0], m[i][1], m[i][2]];
    let (a, b, c) = (r(0), r(1), r(2));
    let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0]);
    let scale = [a, b, c].iter().flatten().fold(0.0f64, |s, x| s.max(x.abs()));
    det.is_finite() && det.abs() > 1e-12 * scale * scale * scale
}

impl GraphMaterials {
    pub fn new(materials: Vec<GraphMaterial>, bindings: Vec<GraphBinding>) -> Result<Self> {
        if materials.is_empty() || materials.len() > MAX_GRAPH_MATERIALS {
            return Err(invalid(format!("graph materials require 1..{MAX_GRAPH_MATERIALS} materials")));
        }
        if bindings.is_empty() || bindings.len() > MAX_GRAPH_BINDINGS {
            return Err(invalid(format!("graph materials require 1..{MAX_GRAPH_BINDINGS} bindings")));
        }
        let mut seen = std::collections::BTreeSet::new();
        for b in &bindings {
            if b.body_index >= MAX_GPU_PATH_BODIES
                || b.material >= materials.len()
                || b.body_name.is_empty()
                || b.body_name.len() > 1024
                || b.body_name.chars().any(char::is_control)
                || b.body_name.starts_with("__render_stage__/")
                || !seen.insert((b.body_index, b.face))
            {
                return Err(invalid("invalid or duplicate graph material binding"));
            }
            if b.object_to_world.as_ref().is_some_and(|m| !finite_placement(m)) {
                return Err(invalid("graph binding placement must be finite, within 1e7 mm and invertible"));
            }
        }
        let mut shapes: Vec<ShapeKey> = materials.iter().map(|m| m.ir.shape).collect();
        shapes.sort_unstable();
        shapes.dedup();
        if shapes.len() > MAX_GRAPH_SHAPES {
            return Err(invalid(format!("a job compiles at most {MAX_GRAPH_SHAPES} material graph shapes")));
        }
        let mut unique = std::collections::BTreeMap::new();
        for texture in materials.iter().flat_map(|m| m.textures.iter().flatten()) {
            unique.entry(texture.identity()).or_insert(texture.pixel_bytes());
        }
        let pixel_bytes = unique.values().try_fold(0u64, |s, b| s.checked_add(*b)).ok_or_else(|| invalid("graph texture accounting overflow"))?;
        if pixel_bytes > MAX_GRAPH_PIXEL_BYTES {
            return Err(invalid("graph textures exceed 256 MiB of linear pixels"));
        }
        Ok(Self { materials, bindings, shapes, pixel_bytes })
    }
    pub fn materials(&self) -> &[GraphMaterial] {
        &self.materials
    }
    pub fn bindings(&self) -> &[GraphBinding] {
        &self.bindings
    }
    /// The job's distinct shapes, sorted: the path pipeline has one evaluator per entry.
    pub fn shapes(&self) -> &[ShapeKey] {
        &self.shapes
    }
    /// The material bound to `face` of body `body_index`: its own binding, else the body's.
    pub fn binding_for(&self, body_index: usize, face: u32) -> Option<&GraphBinding> {
        let find = |face: Option<u32>| self.bindings.iter().find(|b| b.body_index == body_index && b.face == face);
        find(Some(face)).or_else(|| find(None))
    }
    /// Source snapshots, decoded pixels, staging and the device buffer, plus generated code.
    pub fn working_bytes(&self) -> u64 {
        let sources: u64 = self.materials.iter().flat_map(|m| m.textures.iter().flatten()).map(|t| t.source.info().encoded_bytes as u64).sum();
        let records = self.bindings.len() as u64 * (limits::UNIFORM_FLOATS as u64 * 4 + 256);
        sources.saturating_add(self.pixel_bytes.saturating_mul(3)).saturating_add(records).saturating_add(4 * 1024 * 1024)
    }
    pub fn summary(&self) -> GraphSummary {
        let approximations = self
            .bindings
            .iter()
            .filter_map(|b| {
                let inputs = self.materials.get(b.material)?.approximations.clone();
                (!inputs.is_empty()).then(|| BindingApproximations { body_index: b.body_index, body_name: b.body_name.clone(), face: b.face, inputs })
            })
            .collect();
        GraphSummary {
            materials: self.materials.len(),
            shapes: self.shapes.len(),
            bindings: self.bindings.len(),
            linear_pixel_bytes: self.pixel_bytes,
            approximations,
        }
    }
    /// Every binding names a body of `scene` by ordinal and exact name, and a bound transmissive
    /// surface keeps what the path tracer's glass allows (no coat, no metal).
    pub fn validate_scene(&self, scene: &ProductScene, cancel: &CancelToken) -> Result<()> {
        for (index, b) in self.bindings.iter().enumerate() {
            if index % 1024 == 0 {
                cancel.check()?;
            }
            let body = scene
                .meshes
                .get(b.body_index)
                .filter(|body| body.name == b.body_name)
                .ok_or_else(|| invalid("graph binding ordinal and exact body name must match render.scene"))?;
            if let Some(face) = b.face
                && !body.mesh.tri_face.contains(&face)
            {
                return Err(invalid("graph binding names a face the body does not have"));
            }
            let transmissive = match b.face {
                Some(face) => body.faces.iter().find(|f| f.face == face).map_or(body.transmission, |f| f.transmission).is_some(),
                None => body.transmission.is_some() || body.faces.iter().any(|f| f.transmission.is_some()),
            };
            let Some(material) = self.materials.get(b.material) else { return Err(invalid("graph binding material out of range")) };
            if transmissive && drives_coat_or_metal(&material.ir, &material.words) {
                return Err(invalid("clear coat and metal graphs cannot shade a transmissive surface"));
            }
        }
        Ok(())
    }
}

/// Whether a material can put a coat or metal on its surface (the path tracer's glass has neither).
fn drives_coat_or_metal(ir: &Ir, words: &[u32]) -> bool {
    std::iter::once(ir.surface.as_slice())
        .chain(ir.mix_layers.iter().map(|l| l.fg.as_slice()))
        .any(|surface| surface.iter().any(|(name, op)| matches!(*name, "coat_weight" | "base_metalness") && active(op, &[0.0], words)))
}

impl Serialize for GraphMaterials {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.summary().serialize(serializer)
    }
}

#[cfg(test)]
#[path = "graphs_tests.rs"]
mod tests;
