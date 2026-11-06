//! The path tracer's back end of the material graph (docs/material-graph.md §5.7): the same IR
//! and the same code generator as the viewport, with a second target.
//!
//! - **Table.** A job's graph materials live in the texture buffer after its pixels, so `image`
//!   nodes sample the same `texture_words` with the same identity de-duplication as runtime maps.
//!   One record per (material, body): the shape, the body's object frame, the generator's record
//!   (its base-response head, then the material's uniform slots) and the image references.
//! - **Triangles** name their record in `interior.z` as −(record + 1); zero and the stage flag 1
//!   keep their meanings, so a triangle without a graph never enters `graph_surface`.
//! - **Code.** One evaluator per shape in the job, generated from the IR, and a `graph_surface`
//!   that switches on the record's shape and resolves the hit's triangle (`graph.wgsl`). A job's
//!   shader is compiled once per distinct shape set (jobs are immutable snapshots).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use solidcore_record::Vec3;
use solidcraft_render::ProductScene;
use solidcraft_render::job::graphs::{GraphMaterials, GraphTexture};
use solidcraft_render::surface::srgb8_to_linear;
use solidcraft_render::texture_input::{MapRole, PixelLayout};

use crate::material::textures::{PATH_BYTES_PER_PIXEL, ResidentPolicy};
use solidcraft_render::job::textures::RuntimeTextures;
use solidcraft_render::job::{CancelToken, ErrorCode, JobError, Result};
use solidcraft_render::material_graph::ir::{Ir, ShapeKey};

use crate::material::codegen::{self, ImageParams, Target};

/// vec4s of a record's path head (the shape, then world-to-object and object-to-world rows);
/// `GRAPH_HEAD` in `graph.wgsl`.
pub(super) const HEAD_VEC4: u32 = 7;
/// The largest vec4 index a record may start at: `interior.z` holds it exactly as an `f32`.
const MAX_RECORD_VEC4: u32 = 1 << 24;

fn invalid(message: impl Into<String>) -> JobError {
    JobError::new(ErrorCode::InvalidScene, message)
}

/// The path target of the shared generator: reads the record from the texture buffer and samples
/// images through `graph_image`.
struct PathTarget<'a> {
    name: String,
    ir: &'a Ir,
}

impl Target for PathTarget<'_> {
    fn name(&self) -> &str {
        &self.name
    }
    fn vec4(&self, index: &str) -> String {
        format!("graph_vec4({index})")
    }
    fn image(&self, binding: u8, tc: &str, default: &str, params: ImageParams) -> String {
        let slot = codegen::BASE_VEC4 + slot_vec4s(self.ir) + u32::from(binding);
        format!("graph_image(c.record + {slot}u, {tc}, {default}, {}u, {}u)", params.wrap, params.channel)
    }
    fn image_slope(&self, binding: u8, tc: &str, params: ImageParams) -> String {
        let slot = codegen::BASE_VEC4 + slot_vec4s(self.ir) + u32::from(binding);
        format!("graph_image_slope(c.record + {slot}u, {tc}, {}u, {}u)", params.wrap, params.channel)
    }
}

/// vec4s of a material's uniform slots.
fn slot_vec4s(ir: &Ir) -> u32 {
    ir.uniform_floats.div_ceil(4)
}

/// vec4s of one record.
fn record_vec4s(ir: &Ir) -> u32 {
    HEAD_VEC4 + codegen::BASE_VEC4 + slot_vec4s(ir) + u32::try_from(ir.textures.len()).unwrap_or(u32::MAX)
}

/// The WGSL a graph job appends to the path shader: one evaluator per shape (in
/// [`GraphMaterials::shapes`] order) and `graph_surface`, which switches on the record's shape.
pub(super) fn source(graphs: &GraphMaterials) -> Result<String> {
    let mut out = String::new();
    let mut cases = String::new();
    for (index, shape) in graphs.shapes().iter().enumerate() {
        let ir = graphs.materials().iter().map(|m| m.ir()).find(|ir| ir.shape == *shape).ok_or_else(|| invalid("graph shape without a material"))?;
        let target = PathTarget { name: format!("graph_eval_{index}"), ir };
        let eval = codegen::generate(ir, &target).map_err(|e| invalid(format!("material graph shape cannot run in the path tracer: {e}")))?;
        out.push_str(&eval);
        out.push('\n');
        let _ = writeln!(cases, "        case {index}u: {{ return graph_resolve(t, graph_eval_{index}(c)); }}");
    }
    let _ = write!(
        out,
        "fn graph_surface(t: Triangle, p: vec3<f32>, n: vec3<f32>, v: vec3<f32>) -> Triangle {{\n    \
         let r = graph_record(t);\n    if r == 0u {{ return t; }}\n    let c = graph_context(r - 1u, p, n, v);\n    \
         switch graph_words(r - 1u).x {{\n{cases}        default: {{ return t; }}\n    }}\n}}\n"
    );
    Ok(out)
}

/// Admission of a job's graphs: bindings name the scene's bodies, glass stays coat- and
/// metal-free, a body takes runtime maps or a graph but not both, and every shape generates.
pub(super) fn admit(graphs: &GraphMaterials, scene: &ProductScene, textures: Option<&RuntimeTextures>) -> Result<()> {
    graphs.validate_scene(scene, &CancelToken::default())?;
    if let Some(textures) = textures
        && textures.bindings().iter().any(|t| graphs.bindings().iter().any(|g| g.body_index == t.body_index))
    {
        return Err(invalid("a body takes runtime texture maps or a material graph, not both"));
    }
    source(graphs).map(|_| ())
}

/// The job's shape set: what its compiled path pipeline depends on.
pub(super) fn shape_set(graphs: &GraphMaterials) -> Vec<ShapeKey> {
    graphs.shapes().to_vec()
}

/// Where each binding's record starts, for `interior.z`.
#[derive(Default)]
pub(super) struct Records {
    /// (body, face or every face) → record vec4 index + 1.
    by_binding: BTreeMap<(usize, Option<u32>), u32>,
}

impl Records {
    /// `interior.z` of a triangle of `face` on body `body`: −(record + 1), or 0 without a graph.
    pub(super) fn interior(&self, body: usize, face: u32) -> f32 {
        let r = self.by_binding.get(&(body, Some(face))).or_else(|| self.by_binding.get(&(body, None))).copied().unwrap_or(0);
        -(r as f32)
    }
}

fn push_vec4(bytes: &mut Vec<u8>, words: [u32; 4]) {
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
}

fn push_f32(bytes: &mut Vec<u8>, v: [f64; 4]) {
    push_vec4(bytes, v.map(|x| (x as f32).to_bits()));
}

/// The inverse of a 3×3 matrix given by rows, or `None` when singular.
fn inverse(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let [a, b, c] = m;
    let cross = |x: [f64; 3], y: [f64; 3]| [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let (r0, r1, r2) = (cross(b, c), cross(c, a), cross(a, b));
    let det = a[0] * r0[0] + a[1] * r0[1] + a[2] * r0[2];
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    // The inverse's columns are the cofactor rows over det.
    Some([[r0[0] / det, r1[0] / det, r2[0] / det], [r0[1] / det, r1[1] / det, r2[1] / det], [r0[2] / det, r1[2] / det, r2[2] / det]])
}

/// Decode a graph texture at the path tracer's resident size (`ResidentPolicy::path`, the same
/// power-of-two filter as the viewport's mips) and append its linear RGBA `f32` pixels, row 0 at
/// the top: sRGB decoded per texel, data and 16-bit height as stored (height in every colour
/// channel, alpha 1). Shared by identity with runtime maps and other graphs.
fn pixels(
    bytes: &mut Vec<u8>,
    texture: &GraphTexture,
    policy: &ResidentPolicy,
    cancel: &CancelToken,
    runtime: &BTreeMap<([u8; 32], u8), (u32, [u32; 2])>,
    shared: &mut BTreeMap<([u8; 32], u8), (u32, [u32; 2])>,
) -> Result<(u32, [u32; 2])> {
    cancel.check()?;
    if let Some(record) = shared.get(&texture.identity()) {
        return Ok(*record);
    }
    // A runtime map's record is its mip descriptor: the top level's pixels start where its first
    // level entry says (`textures::pixels`).
    if let Some((descriptor, size)) = runtime.get(&texture.identity()) {
        let word = (*descriptor as usize + 4) * 4;
        let offset = bytes.get(word..word + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes);
        let offset = offset.ok_or_else(|| JobError::new(ErrorCode::Internal, "runtime map descriptor outside the texture buffer"))?;
        shared.insert(texture.identity(), (offset, *size));
        return Ok((offset, *size));
    }
    let failed = |e: solidcraft_render::texture_input::TextureError| {
        if cancel.is_cancelled() { JobError::new(ErrorCode::Cancelled, "graph texture decoding cancelled") } else { invalid(e.to_string()) }
    };
    let info = texture.source().info();
    let mut decoded = texture.source().decode(|| cancel.is_cancelled()).map_err(failed)?;
    for _ in 0..policy.levels_to_drop(info.width, info.height, PATH_BYTES_PER_PIXEL, false) {
        match decoded.halve(|| cancel.is_cancelled()).map_err(failed)? {
            Some(next) => decoded = next,
            None => break,
        }
    }
    let (decoded, _) = decoded.fit_within(policy.max_edge, || cancel.is_cancelled()).map_err(failed)?;
    let (w, h) = (decoded.info().width, decoded.info().height);
    let count = usize::try_from(u64::from(w) * u64::from(h)).map_err(|_| JobError::new(ErrorCode::LimitExceeded, "graph texture too large"))?;
    bytes.try_reserve(count.saturating_mul(16)).map_err(|_| JobError::new(ErrorCode::AllocationFailed, "graph texture allocation failed"))?;
    let offset = u32::try_from(bytes.len() / 4).map_err(|_| JobError::new(ErrorCode::LimitExceeded, "texture buffer offset overflow"))?;
    let srgb = decoded.info().role == MapRole::BaseColor;
    for (index, px) in (0..count).enumerate() {
        if index % 4096 == 0 {
            cancel.check()?;
        }
        let rgba: [f32; 4] = match decoded.info().layout {
            PixelLayout::Gray16 => {
                let v = decoded.pixels().get(2 * px..2 * px + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
                let v = f32::from(v) / 65535.0;
                [v, v, v, 1.0]
            }
            PixelLayout::Rgba8 => {
                let c = decoded.rgba().get(4 * px..4 * px + 4).unwrap_or(&[0; 4]);
                let channel = |k: usize| {
                    let x = c.get(k).copied().unwrap_or(0);
                    if srgb && k < 3 { srgb8_to_linear(x) as f32 } else { f32::from(x) / 255.0 }
                };
                [channel(0), channel(1), channel(2), channel(3)]
            }
        };
        for x in rgba {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
    }
    shared.insert(texture.identity(), (offset, [w, h]));
    Ok((offset, [w, h]))
}

/// Append the job's graph table to the texture buffer: first every image's pixels (shared with
/// runtime maps by identity), then the table, contiguous and vec4-aligned. Writes the table's
/// first word to `texture_words[1]`. `origin` is the camera target the geometry is rebased on.
pub(super) fn append(
    bytes: &mut Vec<u8>,
    graphs: &GraphMaterials,
    origin: Vec3,
    cancel: &CancelToken,
    runtime: &BTreeMap<([u8; 32], u8), (u32, [u32; 2])>,
    policy: &ResidentPolicy,
) -> Result<Records> {
    let mut shared = BTreeMap::new();
    let mut references = Vec::with_capacity(graphs.materials().len());
    for material in graphs.materials() {
        let mut refs = Vec::with_capacity(material.textures().len());
        for texture in material.textures() {
            let (offset, size) = match texture {
                Some(texture) => pixels(bytes, texture, policy, cancel, runtime, &mut shared)?,
                None => (0, [0; 2]),
            };
            refs.push([offset, size[0], size[1], 0]);
        }
        references.push(refs);
    }
    while !bytes.len().is_multiple_of(16) {
        bytes.push(0);
    }
    let start = u32::try_from(bytes.len() / 4).map_err(|_| JobError::new(ErrorCode::LimitExceeded, "graph table offset overflow"))?;
    bytes.get_mut(4..8).ok_or_else(|| JobError::new(ErrorCode::Internal, "texture header missing"))?.copy_from_slice(&start.to_le_bytes());
    push_f32(bytes, [origin.x, origin.y, origin.z, 0.0]);
    let mut next = 1u32;
    let mut made = BTreeMap::<(usize, usize), u32>::new();
    let mut records = Records::default();
    for (index, binding) in graphs.bindings().iter().enumerate() {
        if index % 1024 == 0 {
            cancel.check()?;
        }
        let key = (binding.material, binding.body_index);
        if let Some(record) = made.get(&key) {
            records.by_binding.insert((binding.body_index, binding.face), record + 1);
            continue;
        }
        let material = graphs.materials().get(binding.material).ok_or_else(|| invalid("graph binding material out of range"))?;
        let ir = material.ir();
        let shape = graphs.shapes().binary_search(&ir.shape).map_err(|_| invalid("graph material shape missing"))?;
        let record = next;
        next = next
            .checked_add(record_vec4s(ir))
            .filter(|n| *n < MAX_RECORD_VEC4)
            .ok_or_else(|| JobError::new(ErrorCode::LimitExceeded, "graph material table exceeds 16 Mi vec4"))?;
        let placement = binding.object_to_world.unwrap_or([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
        let m = placement.map(|r| [r[0], r[1], r[2]]);
        let t = placement.map(|r| r[3]);
        let inv = inverse(m).ok_or_else(|| invalid("graph binding placement is singular"))?;
        // object = M⁻¹·(p + origin − t) for the rebased point p.
        let d = [origin.x - t[0], origin.y - t[1], origin.z - t[2]];
        push_vec4(bytes, [u32::try_from(shape).unwrap_or(u32::MAX), 0, 0, 0]);
        for row in inv {
            push_f32(bytes, [row[0], row[1], row[2], row[0] * d[0] + row[1] * d[1] + row[2] * d[2]]);
        }
        for row in m {
            push_f32(bytes, [row[0], row[1], row[2], 0.0]);
        }
        // The generator's base-response head: the path tracer's fallback is the captured surface,
        // so the head stays zero.
        for _ in 0..codegen::BASE_VEC4 {
            push_vec4(bytes, [0; 4]);
        }
        let words = material.words();
        for k in 0..slot_vec4s(ir) as usize {
            let at = |i: usize| words.get(4 * k + i).copied().unwrap_or(0);
            push_vec4(bytes, [at(0), at(1), at(2), at(3)]);
        }
        for r in references.get(binding.material).map(Vec::as_slice).unwrap_or(&[]) {
            push_vec4(bytes, *r);
        }
        made.insert(key, record);
        records.by_binding.insert((binding.body_index, binding.face), record + 1);
    }
    Ok(records)
}

#[cfg(test)]
#[path = "material_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "golden_tests.rs"]
mod golden_tests;
