//! The material graph's raster back end (docs/material-graph.md §5.1–5.4): the viewport's half of
//! "one IR, two GPU back ends". It turns a graph's IR (`solidcraft_render::material_graph`) into a
//! WGSL module per **shape**, caches their pipelines, and draws every material of one shape on one
//! uniform page with one draw.
//!
//! - [`codegen`]: IR → `material_eval`, straight-line WGSL over the shared library
//!   (`crate::shading`); no user string reaches WGSL.
//! - [`scene`]: uniform pages (16 KiB, WebGL2's block limit), each material's record (its base
//!   response, then its slots) and the per-draw vertices, which carry the record's vec4 offset.
//! - [`textures`]: the decode worker, the resident size, GPU mips by render passes and the
//!   sampling the raster target's `image` calls.
//! - [`cache`]: pipelines by shape key (at most [`CACHE_SHAPES`], LRU), compiled within a per-frame
//!   budget; a shape not compiled yet draws its base response through the always-built fallback.
//!
//! Group 3 of these pipelines is "studio + material" (`view::ibl::material_layout`), so the four
//! bind groups WebGL2 allows stay enough.

pub(crate) mod cache;
pub mod codegen;
pub mod scene;
pub mod textures;

pub use cache::MaterialStats;
pub use codegen::{BASE_VEC4, CodegenError, ImageParams, Raster, Target, generate, material_eval};
pub use scene::{GraphDraw, GraphMaterial, GraphScene, GraphSceneError, TEXTURE_SLOTS, TextureSet, base_record};
pub use textures::{PreparedTexture, ResidentPolicy, TextureCaps, TextureDecoder, TextureKey, TextureStats, texture_keys};

use solidcraft_render::material_graph::ir::Ir;

/// A graph-drawn vertex: position, normal (three floats each) and the material record's vec4
/// offset in its page (`u32`).
pub const VERTEX_SIZE: usize = 28;
/// The same with the object-space position, for shapes that read object space.
pub const OBJECT_VERTEX_SIZE: usize = 40;
/// vec4s in one uniform page.
pub const PAGE_VEC4: u32 = 1024;
/// Bytes in one uniform page: WebGL2's uniform-block limit.
pub const PAGE_BYTES: u64 = PAGE_VEC4 as u64 * 16;
/// Shapes whose pipelines a renderer keeps (least recently drawn first out).
pub const CACHE_SHAPES: usize = 64;
/// New shape pipelines a frame may create (target, §5.2).
pub const COMPILES_PER_FRAME: usize = 2;
/// A frame that has already taken this long (ms) before drawing creates no new pipeline.
pub const FRAME_BUDGET_MS: f64 = 8.0;

/// The vertex streams and their AO entries, part of the viewport module.
pub const STREAM: &str = include_str!("stream.wgsl");
/// The raster back end's part of every shape module (page, context, helpers, `fs_mat`).
pub const PRELUDE: &str = include_str!("prelude.wgsl");

fn module(eval: &str) -> String {
    // `pbr::SHADE` starts with the shared library.
    format!("{}\n{}\n{STREAM}\n{PRELUDE}\n{}\n{eval}", crate::view::SHADER, crate::pbr::SHADE, textures::sampling_source())
}

/// The WGSL module of a graph's shape: the shared library, the viewport's shading, the stream,
/// the prelude and the generated `material_eval`.
pub fn shape_source(ir: &Ir) -> Result<String, CodegenError> {
    Ok(module(&material_eval(ir)?))
}

/// The module of the constant-graph pipeline: every material's base response.
pub fn fallback_source() -> String {
    module(codegen::FALLBACK_EVAL)
}

/// Parse and validate a module with naga, as the device would, so a bad shape is an error value
/// before any pipeline is made.
pub fn validate(source: &str) -> Result<(naga::Module, naga::valid::ModuleInfo), CodegenError> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| CodegenError(e.emit_to_string(source)))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .map_err(|e| CodegenError(format!("{e:?}")))?;
    Ok((module, info))
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod shade_tests;
