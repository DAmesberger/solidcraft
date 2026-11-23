//! Pure WGSL shared by the viewport and the GPU path tracer.

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

/// Shared surface response, periodic noise and texture projections in dependency order.
pub const SOURCE: &str = library_source!();

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod bsdf_tests;
