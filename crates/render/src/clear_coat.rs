//! Original single-scattering GGX dielectric coat over the opaque surface oracle.
//! The substrate is attenuated at entry and exit; internal inter-layer scattering
//! and refraction of substrate directions are omitted. This is a bounded preview
//! model, not a full layered random walk. See docs/render-clear-coat.md.
use crate::surface::{MIN_ROUGHNESS, OpaqueSurface, SurfaceError, unit_direction};
use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearCoat {
    weight: f64,
    roughness: f64,
    ior: f64,
}

impl ClearCoat {
    pub fn new(weight: f64, roughness: f64, ior: f64) -> Result<Self, SurfaceError> {
        if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
            return Err(SurfaceError::CoatWeight);
        }
        if !roughness.is_finite() || !(MIN_ROUGHNESS..=1.0).contains(&roughness) {
            return Err(SurfaceError::Roughness);
        }
        if !ior.is_finite() || !(1.0..=3.0).contains(&ior) {
            return Err(SurfaceError::DielectricIor);
        }
        Ok(Self { weight, roughness, ior })
    }
    pub fn weight(self) -> f64 {
        self.weight
    }
    pub fn roughness(self) -> f64 {
        self.roughness
    }
    pub fn ior(self) -> f64 {
        self.ior
    }
    fn fresnel(self, c: f64) -> f64 {
        if self.ior == 1.0 {
            return 0.0;
        }
        let ratio = (self.ior - 1.0) / (self.ior + 1.0);
        let f0 = ratio * ratio;
        f0 + (1.0 - f0) * (1.0 - c).powi(5)
    }
    /// Upper bound on coat directional reflectance: over the upper hemisphere,
    /// v.h >= nv/sqrt(2*(1+sqrt(1-nv^2))). Schlick decreases with v.h and
    /// the unit-Fresnel single-scattering Smith GGX lobe reflects at most one.
    fn energy_bound(self, cosine: f64) -> f64 {
        self.fresnel(cosine / (2.0 * (1.0 + (1.0 - cosine * cosine).max(0.0).sqrt())).sqrt())
    }
    pub(crate) fn layer(self, base: Vec3, nv: f64, nl: f64, nh: f64, vh: f64) -> Vec3 {
        if self.ior == 1.0 {
            return base;
        }
        let a2 = self.roughness.powi(4);
        let denom = (1.0 - nh * nh) + nh * nh * a2;
        let d = a2 / (PI * denom * denom);
        let visibility = |c: f64| 2.0 / (c + (a2 + (1.0 - a2) * c * c).sqrt());
        let coat = self.weight * self.fresnel(vh) * d * 0.25 * visibility(nv) * visibility(nl);
        let attenuation = (1.0 - self.weight * self.energy_bound(nv)) * (1.0 - self.weight * self.energy_bound(nl));
        base * attenuation + Vec3::new(coat, coat, coat)
    }
}

/// Test oracle for the viewport's coat colour (`sc_coat_layer` in `solidcraft-gpu`, docs/material-graph.md
/// §5.5): [`tinted_brdf`] with the default specular.
#[cfg(any(test, feature = "test-reference"))]
pub fn tinted_coat_brdf(surface: OpaqueSurface, coat_color: Vec3, normal: Vec3, view: Vec3, light: Vec3) -> Result<Vec3, SurfaceError> {
    tinted_brdf(surface, 1.0, Vec3::new(1.0, 1.0, 1.0), coat_color, normal, view, light)
}

/// Test oracle for the viewport's specular tint and coat colour (`sc_layered_brdf` in
/// `solidcraft-gpu`, docs/material-graph.md §3.1 and §5.5): the coated BRDF of `surface` with
/// OpenPBR's `specular_weight` and `specular_color` on the base and its coat tinted by linear
/// `coat_color`.
///
/// - Specular: the dielectric's normal-incidence reflectance is scaled by the colour, the
///   metal's grazing reflectance is the colour (its edge tint); both are scaled by the weight,
///   and Schlick's Fresnel runs from the one to the other. The diffuse layer takes what the
///   tinted interface passes. Weight 1 and a white colour are [`OpaqueSurface::brdf`].
/// - Coat colour: a thin absorbing layer tints the substrate once on entry and once on exit; at
///   coverage w each pass keeps `1 - w * (1 - coat_color)`, on top of the coat's Fresnel
///   attenuation. The coat's own reflection is untinted. A white coat colour is the clear coat;
///   an index-matched coat (IOR 1) only tints.
///
/// No CPU renderer uses this: it exists for the GPU tests (feature `test-reference`).
#[cfg(any(test, feature = "test-reference"))]
pub fn tinted_brdf(
    surface: OpaqueSurface,
    specular_weight: f64,
    specular_color: Vec3,
    coat_color: Vec3,
    normal: Vec3,
    view: Vec3,
    light: Vec3,
) -> Result<Vec3, SurfaceError> {
    for c in [specular_weight, specular_color.x, specular_color.y, specular_color.z, coat_color.x, coat_color.y, coat_color.z] {
        if !c.is_finite() || !(0.0..=1.0).contains(&c) {
            return Err(SurfaceError::BaseColor);
        }
    }
    let n = unit_direction(normal)?;
    let v = unit_direction(view)?;
    let l = unit_direction(light)?;
    let nv = n.dot(v).clamp(-1.0, 1.0);
    let nl = n.dot(l).clamp(-1.0, 1.0);
    let base = OpaqueSurface::new(surface.base_color(), surface.metallic(), surface.roughness(), surface.dielectric_ior())?;
    let under = if specular_weight == 1.0 && specular_color == Vec3::new(1.0, 1.0, 1.0) {
        base.brdf(n, v, l)?
    } else {
        tinted_base(base, specular_weight, specular_color, n, v, l)?
    };
    let Some(coat) = surface.clear_coat() else { return Ok(under) };
    if nv <= 0.0 || nl <= 0.0 {
        return Ok(Vec3::ZERO);
    }
    let w = coat.weight();
    if w == 0.0 {
        return Ok(under);
    }
    let tint = |c: f64| (1.0 - w * (1.0 - c)).powi(2);
    let tint = Vec3::new(tint(coat_color.x), tint(coat_color.y), tint(coat_color.z));
    let h = unit_direction(v + l)?;
    let nh = n.dot(h).clamp(0.0, 1.0);
    let vh = v.dot(h).clamp(0.0, 1.0);
    // The white layer, then the tint on the attenuated substrate alone.
    let white = coat.layer(under, nv, nl, nh, vh);
    let attenuated = if coat.ior() == 1.0 { under } else { under * ((1.0 - w * coat.energy_bound(nv)) * (1.0 - w * coat.energy_bound(nl))) };
    let lobe = white - attenuated;
    Ok(Vec3::new(attenuated.x * tint.x, attenuated.y * tint.y, attenuated.z * tint.z) + lobe)
}

/// The isotropic base BRDF with a specular tint ([`tinted_brdf`]).
#[cfg(any(test, feature = "test-reference"))]
fn tinted_base(base: OpaqueSurface, weight: f64, color: Vec3, n: Vec3, v: Vec3, l: Vec3) -> Result<Vec3, SurfaceError> {
    let nv = n.dot(v).clamp(-1.0, 1.0);
    let nl = n.dot(l).clamp(-1.0, 1.0);
    if nv <= 0.0 || nl <= 0.0 {
        return Ok(Vec3::ZERO);
    }
    let h = unit_direction(v + l)?;
    let nh = n.dot(h).clamp(0.0, 1.0);
    let vh = v.dot(h).clamp(0.0, 1.0);
    let a2 = base.roughness().powi(4);
    let denom = (1.0 - nh * nh) + nh * nh * a2;
    let visibility = |c: f64| 2.0 / (c + (a2 + (1.0 - a2) * c * c).sqrt());
    let lobe = a2 / (PI * denom * denom) * 0.25 * visibility(nv) * visibility(nl);
    let m = base.metallic();
    let ior = base.dielectric_ior();
    let ratio = ((ior - 1.0) / (ior + 1.0)).powi(2);
    let grazing = if ior == 1.0 { 0.0 } else { 1.0 };
    let mix = |d: f64, metal: f64| (d + (metal - d) * m) * weight;
    let b = base.base_color();
    let f0 = Vec3::new(mix(ratio * color.x, b.x), mix(ratio * color.y, b.y), mix(ratio * color.z, b.z));
    let f90 = Vec3::new(mix(grazing, color.x), mix(grazing, color.y), mix(grazing, color.z));
    let fresnel = |c: f64| f0 + (f90 - f0) * (1.0 - c.clamp(0.0, 1.0)).powi(5);
    let (f, fv, fl) = (fresnel(vh), fresnel(nv), fresnel(nl));
    let diffuse = b * ((1.0 - m) / PI);
    Ok(Vec3::new(
        f.x * lobe + (1.0 - fv.x) * (1.0 - fl.x) * diffuse.x,
        f.y * lobe + (1.0 - fv.y) * (1.0 - fl.y) * diffuse.y,
        f.z * lobe + (1.0 - fv.z) * (1.0 - fl.z) * diffuse.z,
    ))
}

/// Conditional opaque-lobe PDF, including the coat and base mixture. Rejected
/// below-surface microfacet samples retain their mass; never renormalize them.
pub fn surface_pdf(surface: OpaqueSurface, normal: Vec3, view: Vec3, light: Vec3) -> Result<f64, SurfaceError> {
    let n = unit_direction(normal)?;
    let v = unit_direction(view)?;
    let l = unit_direction(light)?;
    let nl = n.dot(l);
    if nl <= 0.0 || n.dot(v) <= 0.0 {
        return Ok(0.0);
    }
    let h = unit_direction(v + l)?;
    let nh = n.dot(h).clamp(0.0, 1.0);
    let vh = v.dot(h).max(1e-20);
    let ggx = |roughness: f64| {
        let a2 = roughness.powi(4);
        let d = (1.0 - nh * nh) + nh * nh * a2;
        a2 / (PI * d * d) * nh / (4.0 * vh)
    };
    let base = 0.5 * nl / PI + 0.5 * ggx(surface.roughness());
    Ok(surface.clear_coat().map_or(base, |coat| {
        let p = coat.weight() * 0.5;
        (1.0 - p) * base + p * ggx(coat.roughness())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_matched_coat_does_not_attenuate_or_add_grazing_specular() {
        let base = OpaqueSurface::new(Vec3::new(0.2, 0.5, 0.8), 0.0, 0.3, 1.5).unwrap();
        for roughness in [0.045, 0.5, 1.0] {
            let coat = ClearCoat::new(1.0, roughness, 1.0).unwrap();
            let coated = base.with_clear_coat(coat);
            for nv in [1e-10_f64, 0.02, 0.5, 1.0] {
                let view = Vec3::new((1.0 - nv * nv).sqrt(), 0.0, nv);
                let light = Vec3::new(0.2, 0.3, 0.8);
                assert_eq!(base.brdf(Vec3::Z, view, light).unwrap(), coated.brdf(Vec3::Z, view, light).unwrap());
                assert_eq!(coat.fresnel(nv), 0.0);
                assert_eq!(coat.energy_bound(nv), 0.0);
            }
        }
    }
    #[test]
    fn a_white_coat_colour_is_the_clear_coat_and_a_tint_only_darkens_the_base() {
        let v = Vec3::new(0.5, 0.0, 0.8);
        let l = Vec3::new(-0.3, 0.2, 0.6);
        for (weight, ior) in [(1.0, 1.5), (0.4, 1.5), (1.0, 1.0)] {
            let base = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), 0.0, 0.4, 1.5).unwrap();
            let coated = base.with_clear_coat(ClearCoat::new(weight, 0.12, ior).unwrap());
            let white = tinted_coat_brdf(coated, Vec3::new(1.0, 1.0, 1.0), Vec3::Z, v, l).unwrap();
            assert!((white - coated.brdf(Vec3::Z, v, l).unwrap()).len() < 1e-15, "w={weight} ior={ior}");
            let green = tinted_coat_brdf(coated, Vec3::new(0.1, 0.5, 0.1), Vec3::Z, v, l).unwrap();
            assert!(green.x < white.x && green.y < white.y && green.z < white.z && green.x > 0.0);
            // A black coat absorbs the whole substrate under it: an index-matched one leaves
            // nothing, an interface its own reflection; a partial coat the uncoated share.
            let black = tinted_coat_brdf(coated, Vec3::ZERO, Vec3::Z, v, l).unwrap();
            assert!(black.x >= 0.0 && black.x < white.x, "{black:?}");
            if weight == 1.0 && ior == 1.0 {
                assert_eq!(black, Vec3::ZERO);
            }
            if weight == 1.0 && ior != 1.0 {
                assert!((black.x - black.y).abs() < 1e-15 && (black.y - black.z).abs() < 1e-15, "only the white lobe: {black:?}");
            }
        }
        // No coat: nothing to tint; out-of-range colours are refused.
        let plain = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), 0.0, 0.4, 1.5).unwrap();
        assert_eq!(tinted_coat_brdf(plain, Vec3::ZERO, Vec3::Z, v, l).unwrap(), plain.brdf(Vec3::Z, v, l).unwrap());
        assert!(tinted_coat_brdf(plain, Vec3::new(1.1, 0.0, 0.0), Vec3::Z, v, l).is_err());
        assert!(tinted_coat_brdf(plain, Vec3::new(f64::NAN, 0.0, 0.0), Vec3::Z, v, l).is_err());
    }
    #[test]
    fn the_specular_tint_reduces_to_the_opaque_surface_and_scales_its_fresnel() {
        let v = Vec3::new(0.5, 0.0, 0.8);
        let l = Vec3::new(-0.3, 0.2, 0.6);
        let white = Vec3::new(1.0, 1.0, 1.0);
        for (metallic, ior) in [(0.0, 1.5), (1.0, 1.5), (0.5, 1.0), (0.0, 2.4)] {
            let s = OpaqueSurface::new(Vec3::new(0.8, 0.5, 0.2), metallic, 0.4, ior).unwrap();
            let plain = s.brdf(Vec3::Z, v, l).unwrap();
            // The tinted formula itself at the default specular is the opaque surface's.
            let base = tinted_base(s, 1.0, white, Vec3::Z, v.normalized().unwrap(), l.normalized().unwrap()).unwrap();
            assert!((base - plain).len() < 1e-14, "m={metallic} ior={ior}: {base:?} vs {plain:?}");
            // Weight 0: no specular interface, the whole Lambert layer of the dielectric part.
            let none = tinted_brdf(s, 0.0, white, white, Vec3::Z, v, l).unwrap();
            let lambert = s.base_color() * ((1.0 - metallic) / PI);
            assert!((none - lambert).len() < 1e-14, "{none:?}");
            // A tint only lowers the specular part; it is reciprocal.
            let tinted = tinted_brdf(s, 0.7, Vec3::new(1.0, 0.5, 0.2), white, Vec3::Z, v, l).unwrap();
            let back = tinted_brdf(s, 0.7, Vec3::new(1.0, 0.5, 0.2), white, Vec3::Z, l, v).unwrap();
            assert!((tinted - back).len() < 1e-14);
            assert!(tinted.x.is_finite() && tinted.x >= 0.0);
        }
        assert!(tinted_brdf(OpaqueSurface::new(white, 0.0, 0.4, 1.5).unwrap(), 1.1, white, white, Vec3::Z, v, l).is_err());
    }
    #[test]
    fn admission_zero_coat_and_reciprocity() {
        for bad in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(ClearCoat::new(bad, 0.2, 1.5).is_err());
        }
        for bad in [0.0, 0.044, 1.1, f64::NAN] {
            assert!(ClearCoat::new(1.0, bad, 1.5).is_err());
        }
        for bad in [0.99, 3.01, f64::NAN] {
            assert!(ClearCoat::new(1.0, 0.2, bad).is_err());
        }
        let base = OpaqueSurface::new(Vec3::new(0.01, 0.02, 0.03), 0.0, 0.3, 1.5).unwrap();
        assert_eq!(base, base.with_clear_coat(ClearCoat::new(0.0, 0.1, 1.5).unwrap()));
        let coated = base.with_clear_coat(ClearCoat::new(1.0, 0.12, 1.5).unwrap());
        let v = Vec3::new(0.5, 0.0, 0.8);
        let l = Vec3::new(-0.3, 0.2, 0.6);
        assert!((coated.brdf(Vec3::Z, v, l).unwrap() - coated.brdf(Vec3::Z, l, v).unwrap()).len() < 1e-12);
        assert!(coated.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap().x > base.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap().x);
        // Normal incidence: D=1/(pi*roughness^4), G=1, Fresnel=.04.
        let expected = base.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap() * (1.0 - ClearCoat::new(1.0, 0.12, 1.5).unwrap().energy_bound(1.0)).powi(2)
            + Vec3::new(1.0, 1.0, 1.0) * (0.04 / (4.0 * PI * 0.12_f64.powi(4)));
        assert!((coated.brdf(Vec3::Z, Vec3::Z, Vec3::Z).unwrap() - expected).len() < 1e-10);
    }
    #[test]
    fn layered_white_furnace_and_pdf_quadrature() {
        // Deterministic quadrature resolves these lobes; it is not a proof for
        // arbitrary roughness. Narrow coat lobes have separate GPU oracle checks.
        for roughness in [0.25, 0.5, 1.0] {
            for metallic in [0.0, 1.0] {
                for cosine in [0.2_f64, 1.0] {
                    let s = OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), metallic, 0.5, 1.5)
                        .unwrap()
                        .with_clear_coat(ClearCoat::new(1.0, roughness, 1.5).unwrap());
                    let v = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, cosine);
                    let mut energy = 0.0;
                    let mut probability = 0.0;
                    for i in 0..256 {
                        let mu = (f64::from(i) + 0.5) / 256.0;
                        let r = (1.0 - mu * mu).sqrt();
                        for j in 0..256 {
                            let phi = 2.0 * PI * (f64::from(j) + 0.5) / 256.0;
                            let l = Vec3::new(r * phi.cos(), r * phi.sin(), mu);
                            energy += s.brdf(Vec3::Z, v, l).unwrap().x * mu;
                            probability += surface_pdf(s, Vec3::Z, v, l).unwrap();
                        }
                    }
                    energy *= 2.0 * PI / 65536.0;
                    probability *= 2.0 * PI / 65536.0;
                    assert!(energy <= 1.025, "roughness={roughness} metal={metallic} cosine={cosine} energy={energy}");
                    assert!((0.2..=1.005).contains(&probability), "{probability}");
                }
            }
        }
    }

    #[test]
    fn conservative_bound_and_layer_furnace_include_shiny_bases_and_grazing() {
        const COUNT: u32 = 4096;
        let mut maximum = 0.0_f64;
        for base_roughness in [0.045, 0.12, 0.5, 1.0] {
            for coat_roughness in [0.12, 0.5, 1.0] {
                for weight in [0.25, 1.0] {
                    for ior in [1.0, 1.5, 3.0] {
                        for cosine in [0.02_f64, 0.2, 1.0] {
                            for metallic in [0.0, 1.0] {
                                let coat = ClearCoat::new(weight, coat_roughness, ior).unwrap();
                                let surface =
                                    OpaqueSurface::new(Vec3::new(1.0, 1.0, 1.0), metallic, base_roughness, 1.5).unwrap().with_clear_coat(coat);
                                let v = Vec3::new((1.0 - cosine * cosine).sqrt(), 0.0, cosine);
                                let p = weight * 0.5;
                                let mut energy = 0.0;
                                // Integrate each proposal with its mixture probability.
                                // NDF importance quadrature resolves sharp lobes, unlike
                                // a uniform hemisphere grid at roughness 0.045.
                                for (component, probability) in [(0, (1.0 - p) * 0.5), (1, (1.0 - p) * 0.5), (2, p)] {
                                    for i in 0..COUNT {
                                        let x = (f64::from(i) + 0.5) / f64::from(COUNT);
                                        let phi = 2.0 * PI * f64::from(i.reverse_bits()) / 4294967296.0;
                                        let l = if component == 0 {
                                            Vec3::new(x.sqrt() * phi.cos(), x.sqrt() * phi.sin(), (1.0 - x).sqrt())
                                        } else {
                                            let r = if component == 1 { base_roughness } else { coat_roughness };
                                            let c = ((1.0 - x) / (1.0 + (r.powi(4) - 1.0) * x)).sqrt();
                                            let radius = (1.0 - c * c).sqrt();
                                            let h = Vec3::new(radius * phi.cos(), radius * phi.sin(), c);
                                            h * (2.0 * v.dot(h)) - v
                                        };
                                        if l.z <= 0.0 {
                                            continue;
                                        }
                                        let h = unit_direction(v + l).unwrap();
                                        assert!(coat.fresnel(v.dot(h).clamp(0.0, 1.0)) <= coat.energy_bound(cosine) + 1e-12);
                                        let pdf = surface_pdf(surface, Vec3::Z, v, l).unwrap();
                                        energy += probability * surface.brdf(Vec3::Z, v, l).unwrap().x * l.z / pdf / f64::from(COUNT);
                                    }
                                }
                                maximum = maximum.max(energy);
                                assert!(
                                    energy.is_finite() && energy <= 1.003,
                                    "base={base_roughness}, coat={coat_roughness}, w={weight}, ior={ior}, nv={cosine}, m={metallic}: {energy}"
                                );
                            }
                        }
                    }
                }
            }
        }
        eprintln!("maximum conservative layered furnace reflectance: {maximum}");
        for cosine in [0.0, 1e-300, 1e-20, 0.02, 1.0] {
            let bound = ClearCoat::new(1.0, 0.12, 1.5).unwrap().energy_bound(cosine);
            assert!(bound.is_finite() && (0.0..=1.0).contains(&bound));
        }
    }
}
