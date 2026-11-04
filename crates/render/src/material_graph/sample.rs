//! The surface node's output: [`OpaqueSurface`] (base colour, metallic, roughness, dielectric
//! IOR and optional [`ClearCoat`]) plus the inputs it lacks. `OpaqueSurface` is the constant
//! core: a `pbr` look is exactly a one-node constant graph whose sample has default extensions,
//! and the two convert losslessly (docs/material-graph.md §5.7).
//!
//! GPU back ends mirror this struct as WGSL `Surface`; tests compare against it.
//!
//! [`ClearCoat`]: crate::clear_coat::ClearCoat

use solidcore_record::Vec3;

use crate::surface::OpaqueSurface;

/// One evaluated surface point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceSample {
    pub opaque: OpaqueSurface,
    pub specular_weight: f64,
    /// Linear.
    pub specular_color: Vec3,
    /// 0: isotropic.
    pub anisotropy: f64,
    /// Radians about the normal.
    pub rotation: f64,
    /// Linear tint of light through the coat.
    pub coat_color: Vec3,
    /// Relative luminance, like the product lights; not calibrated.
    pub emission_luminance: f64,
    pub emission_color: Vec3,
    pub transmission_weight: f64,
    pub transmission_color: Vec3,
    pub transmission_depth_mm: f64,
    /// Cut-out coverage, not glass.
    pub opacity: f64,
}

impl From<OpaqueSurface> for SurfaceSample {
    fn from(opaque: OpaqueSurface) -> Self {
        let one = Vec3::new(1.0, 1.0, 1.0);
        SurfaceSample {
            opaque,
            specular_weight: 1.0,
            specular_color: one,
            anisotropy: 0.0,
            rotation: 0.0,
            coat_color: one,
            emission_luminance: 0.0,
            emission_color: one,
            transmission_weight: 0.0,
            transmission_color: one,
            transmission_depth_mm: 1.0,
            opacity: 1.0,
        }
    }
}

impl SurfaceSample {
    /// The `OpaqueSurface` this sample is exactly, when every extension has its default.
    pub fn as_opaque(&self) -> Option<OpaqueSurface> {
        (*self == SurfaceSample::from(self.opaque)).then_some(self.opaque)
    }
}
