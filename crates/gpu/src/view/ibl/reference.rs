//! CPU references for the product look's tests: the split-sum terms integrated numerically, and
//! the shader's environment term (`pbr_ibl`) evaluated on a baked studio.

use std::f64::consts::PI;

use super::{Rgb, StudioEnvironment, dot, env_brdf, sample};

/// The split-sum scale and bias (A, B) of the viewport's BRDF (GGX, separable Smith, Schlick
/// Fresnel; `pbr_brdf`) at `nv` and perceptual roughness `r`, by GGX importance sampling with
/// `n` stratified samples: the white-furnace albedo is F0·A + F90·B.
pub(crate) fn dfg(nv: f64, r: f64, n: u32) -> (f64, f64) {
    let a2 = r.powi(4);
    let v = [(1.0 - nv * nv).max(0.0).sqrt(), 0.0, nv];
    let g1 = |c: f64| 2.0 * c / (c + (a2 + (1.0 - a2) * c * c).sqrt());
    let (mut sa, mut sb) = (0.0, 0.0);
    for i in 0..n {
        let (u1, u2) = super::hammersley(i, n);
        let phi = 2.0 * PI * u2;
        let ct = ((1.0 - u1) / (1.0 + (a2 - 1.0) * u1)).sqrt();
        let st = (1.0 - ct * ct).max(0.0).sqrt();
        let h = [st * phi.cos(), st * phi.sin(), ct];
        let vh = dot(v, h);
        let l = [2.0 * vh * h[0] - v[0], 2.0 * vh * h[1] - v[1], 2.0 * vh * h[2] - v[2]];
        if l[2] <= 0.0 || vh <= 0.0 {
            continue;
        }
        let w = g1(nv) * g1(l[2]) * vh / (nv * ct);
        let fc = (1.0 - vh).powi(5);
        sa += w * (1.0 - fc);
        sb += w * fc;
    }
    (sa / f64::from(n), sb / f64::from(n))
}

impl StudioEnvironment {
    /// The baked irradiance at unit normal `n` (unrotated).
    pub(crate) fn irradiance_at(&self, n: Rgb) -> Rgb {
        let [x, y, z] = n;
        let p = [1.0, x, y, z, x * y, y * z, x * z, z * z, x * x - y * y];
        let mut e = [0.0; 3];
        for (term, pi) in self.irradiance.iter().zip(p) {
            super::add(&mut e, *term, pi);
        }
        e
    }

    /// The prefiltered radiance along `d` at level `lod`.
    pub(crate) fn prefiltered(&self, d: Rgb, lod: f64) -> Rgb {
        sample(&self.levels, super::unit(d), lod)
    }

    /// A studio of constant radiance `c` everywhere (the white furnace).
    pub(crate) fn furnace(c: f64) -> Self {
        Self::constant(c)
    }

    /// The studio's own (unbaked) radiance along `d`.
    pub(crate) fn source(&self, d: Rgb) -> Rgb {
        super::studio_radiance(super::studio(self.preset), super::unit(d))
    }

    /// The levels the shader samples.
    pub(crate) fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Mean radiance of a level over the sphere (solid-angle weighted).
    pub(crate) fn level_mean(&self, level: usize) -> Rgb {
        let Some(l) = self.levels.get(level) else { return [0.0; 3] };
        let n = l.size as f64;
        let (mut sum, mut total) = ([0.0; 3], 0.0);
        for face in 0..6 {
            for y in 0..l.size {
                for x in 0..l.size {
                    let s = 2.0 * (x as f64 + 0.5) / n - 1.0;
                    let t = 2.0 * (y as f64 + 0.5) / n - 1.0;
                    let w = 1.0 / (1.0 + s * s + t * t).powf(1.5);
                    let c = l.texel(face, x, y);
                    super::add(&mut sum, c, w);
                    total += w;
                }
            }
        }
        sum.map(|v| v / total)
    }
}

/// Rotate `d` by `-rotation` about +Z: where to look in a studio turned by `rotation`.
pub(crate) fn unturn(d: Rgb, rotation: f64) -> Rgb {
    let (s, c) = rotation.sin_cos();
    [c * d[0] + s * d[1], -s * d[0] + c * d[1], d[2]]
}

/// The shader's environment term (`pbr_ibl`) on the CPU: outgoing radiance toward `v` from a
/// surface (`base` linear reflectance, metallic, perceptual roughness, dielectric IOR) with unit
/// normal `n`, lit by `env` turned by `rotation`.
pub(crate) fn shade(env: &StudioEnvironment, base: Rgb, metallic: f64, roughness: f64, ior: f64, n: Rgb, v: Rgb, rotation: f64) -> Rgb {
    let [specular, coat, diffuse] = shade_parts(env, base, metallic, roughness, ior, [0.0, 1.0, 1.0], n, v, rotation);
    std::array::from_fn(|k| specular[k] + coat[k] + diffuse[k])
}

/// `pbr_ibl_parts` on the CPU: the base's specular, the clear coat's (weight, perceptual roughness,
/// IOR) specular and the diffuse layer. A coat reflects its single-scattering albedo
/// w·(F0·A + B) of the studio at its own roughness and passes the rest to the base.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shade_parts(
    env: &StudioEnvironment,
    base: Rgb,
    metallic: f64,
    roughness: f64,
    ior: f64,
    coat: Rgb,
    n: Rgb,
    v: Rgb,
    rotation: f64,
) -> [Rgb; 3] {
    let nv = dot(n, v).max(1e-4);
    let ratio = (ior - 1.0) / (ior + 1.0);
    let f0: Rgb = std::array::from_fn(|k| ratio * ratio + (base[k] - ratio * ratio) * metallic);
    let f90 = if ior == 1.0 { metallic } else { 1.0 };
    let (a, b) = env_brdf(nv as f32, roughness as f32);
    let (a, b) = (f64::from(a), f64::from(b));
    let ess = (a + b).max(1e-4);
    let r = [2.0 * nv * n[0] - v[0], 2.0 * nv * n[1] - v[1], 2.0 * nv * n[2] - v[2]];
    let top = env.level_count().saturating_sub(1) as f64;
    let prefiltered = env.prefiltered(unturn(r, rotation), roughness * top);
    let irradiance = env.irradiance_at(unturn(n, rotation)).map(|e| e.max(0.0));
    let mut specular = [0.0; 3];
    let mut diffuse = [0.0; 3];
    for k in 0..3 {
        let s = (f0[k] * a + f90 * b) * (1.0 + f0[k] * (1.0 / ess - 1.0));
        specular[k] = prefiltered[k] * s;
        diffuse[k] = base[k] * (1.0 - metallic) / PI * (1.0 - s).max(0.0) * irradiance[k];
    }
    let [weight, coat_roughness, coat_ior] = coat;
    if weight <= 0.0 || coat_ior == 1.0 {
        return [specular, [0.0; 3], diffuse];
    }
    let coat_ratio = (coat_ior - 1.0) / (coat_ior + 1.0);
    let (ca, cb) = env_brdf(nv as f32, coat_roughness as f32);
    let albedo = weight * (coat_ratio * coat_ratio * f64::from(ca) + f64::from(cb));
    let seen = env.prefiltered(unturn(r, rotation), coat_roughness * top);
    [specular.map(|s| s * (1.0 - albedo)), seen.map(|c| c * albedo), diffuse.map(|d| d * (1.0 - albedo))]
}
