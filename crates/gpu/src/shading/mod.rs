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
//! Both shaders are on the library: the viewport's `pbr::SOURCE` and `pbr::SHADE` start with it,
//! and the path tracer's shader starts with it (`library_source!`). `bsdf` holds the opaque
//! surface and its coat (extracted from `path.wgsl`'s expressions), `noise` the lattice noises
//! (the path tracer's aging masks' and the viewport's former pattern noise), `projection` the
//! planar, triplanar and cylindrical coordinates. Track C1 fills `patterns`. `bsdf` also holds
//! the viewport's anisotropic GGX (with its bent studio reflection), the coat colour and coat
//! normal (`sc_layered_brdf`) and the see-through fallback for transmission (`sc_see_through`).

/// Safe normalise, Schlick Fresnel with the IOR-1 rule, GGX D, separable Smith, the base BRDF,
/// the coat lobe and its attenuation bound; anisotropic GGX for the viewport.
pub const BSDF: &str = include_str!("bsdf.wgsl");
/// Lattice hashes; value, gradient and cellular noise in 2D and 3D; fBm.
pub const NOISE: &str = include_str!("noise.wgsl");
/// Brushed, weave, checker, grid and gradient patterns with their footprint fades.
pub const PATTERNS: &str = include_str!("patterns.wgsl");
/// Planar, triplanar and cylindrical projections.
pub const PROJECTION: &str = include_str!("projection.wgsl");

/// The whole library as one literal, joined as [`library`] joins it, for `concat!` into a
/// shader's constant source.
macro_rules! library_source {
    () => {
        concat!(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shading/bsdf.wgsl")),
            "\n",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shading/noise.wgsl")),
            "\n",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shading/patterns.wgsl")),
            "\n",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/shading/projection.wgsl"))
        )
    };
}
pub(crate) use library_source;

/// The modules in dependency order.
pub const LIBRARY: &[(&str, &str)] = &[("bsdf", BSDF), ("noise", NOISE), ("patterns", PATTERNS), ("projection", PROJECTION)];

/// The whole library, joined, to put in front of a shader's own source.
pub fn library() -> String {
    LIBRARY.iter().map(|(_, source)| *source).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod bsdf_tests;
#[cfg(test)]
mod tests;
