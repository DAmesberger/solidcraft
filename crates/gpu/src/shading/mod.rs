//! The shared shading library (docs/material-graph.md §5.7): pure WGSL functions that both GPU
//! targets of the material system use, the viewport (`viewport.wgsl` + `pbr::SOURCE`, native and
//! WebGL2) and the path tracer (`path/path.wgsl`, native). One copy of the BSDF terms, noise,
//! patterns and projections, instead of today's two.
//!
//! Rules every module keeps, so it translates to WebGL2's GLSL ES 3.00 inside the viewport:
//! - only pure functions over values: no bindings, no storage, no pointers to storage, no
//!   derivatives (the viewport passes the pixel footprint in) and no entry points;
//! - every name starts with `sc_`, so nothing collides with either shader;
//! - modules are joined by plain concatenation (decision O11), in [`LIBRARY`] order, which is
//!   also their dependency order.
//!
//! Contract status: the modules and their order are fixed; track S fills `bsdf` and `noise` from
//! `path.wgsl`'s exact expressions and ports both shaders onto them, track C1 fills `patterns`
//! and `projection`, track C2 adds the viewport's anisotropic GGX and coat colour to `bsdf`.

/// Safe normalise, Schlick Fresnel with the IOR-1 rule, GGX D, separable Smith, the base BRDF,
/// the coat lobe and its attenuation bound; anisotropic GGX for the viewport.
pub const BSDF: &str = include_str!("bsdf.wgsl");
/// Lattice hashes; value, gradient and cellular noise in 2D and 3D; fBm.
pub const NOISE: &str = include_str!("noise.wgsl");
/// Brushed, weave, checker, grid and gradient patterns with their footprint fades.
pub const PATTERNS: &str = include_str!("patterns.wgsl");
/// Planar, triplanar and cylindrical projections.
pub const PROJECTION: &str = include_str!("projection.wgsl");

/// The modules in dependency order.
pub const LIBRARY: &[(&str, &str)] = &[("bsdf", BSDF), ("noise", NOISE), ("patterns", PATTERNS), ("projection", PROJECTION)];

/// The whole library, joined, to put in front of a shader's own source.
pub fn library() -> String {
    LIBRARY.iter().map(|(_, source)| *source).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests;
