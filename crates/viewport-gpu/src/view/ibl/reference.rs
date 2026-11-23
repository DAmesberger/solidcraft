//! CPU references for the product look's tests: the split-sum terms integrated numerically, and
//! the shader's environment term (`pbr_ibl`) evaluated on a baked studio.

use std::f64::consts::PI;

use super::{BaseLevel, Radiance, Rgb, StudioBake, StudioEnvironment, StudioPreset, dot, mix, unit};
use crate::environment::{DFG_SAMPLES, DFG_SIZE, SPECULAR_LEVELS, dfg_at};

/// Evaluate the same four entries the shader interpolates in the shared BRDF lookup table.
pub(super) fn brdf(nv: f64, r: f64) -> [f64; 2] {
    let size = DFG_SIZE as f64;
    let texel = |t: f64| (t * size - 0.5).clamp(0.0, size - 1.0);
    let (x, y) = (texel(nv), texel(r));
    let at = |x: f64, y: f64| {
        let uv = |t: f64| ((t.min(size - 1.0) + 0.5) / size) as f32;
        dfg_at(uv(x), uv(y), DFG_SAMPLES).map(f64::from)
    };
    let (x0, y0) = (x.floor(), y.floor());
    let [a, b, c, d] = [at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0)];
    let (u, v) = (x.fract(), y.fract());
    let ab: [f64; 2] = std::array::from_fn(|i| (1.0 - v) * (a[i] * (1.0 - u) + b[i] * u) + v * (c[i] * (1.0 - u) + d[i] * u));
    ab.map(|c| c / (ab[0] + ab[1]).max(1.0))
}

impl StudioEnvironment {
    /// Physical irradiance. The shared map stores irradiance divided by π.
    pub(crate) fn irradiance_at(&self, n: Rgb) -> Rgb {
        self.diffuse.sample_world(unit(n).map(|c| c as f32)).map(|v| f64::from(v) * PI)
    }

    pub(crate) fn prefiltered(&self, d: Rgb, lod: f64) -> Rgb {
        let top = self.levels.len() - 1;
        let lod = lod.clamp(0.0, top as f64);
        let d = unit(d).map(|c| c as f32);
        let sample = |level: usize| self.levels[level.min(top)].sample_world(d).map(f64::from);
        mix(sample(lod.floor() as usize), sample(lod.ceil() as usize), lod.fract())
    }

    pub(super) fn level_at(&self, roughness: f64) -> f64 {
        let top = f64::from(SPECULAR_LEVELS - 1);
        let base = f64::from(self.base.roughness());
        if roughness >= 1.0 / top { roughness * top } else { ((roughness - base) / (1.0 / top - base)).max(0.0) }
    }

    /// A studio of constant radiance in every direction.
    pub(crate) fn furnace(c: f64) -> Self {
        let mut bake = StudioBake::with_base(StudioPreset::NONE, BaseLevel::Preblurred);
        bake.radiance = Radiance::Constant(c);
        bake.bake()
    }

    /// The studio's own (unbaked) radiance along `d`, at its calibrated scale.
    pub(crate) fn source(&self, d: Rgb) -> Rgb {
        super::studio(self.preset).map_or([0.0; 3], |r| r.radiance_in(super::unit(d), r.key_white()).map(|c| c * self.calibration))
    }

    /// The cyclorama's own radiance along `d` (without the rig's lights), at the calibrated scale.
    pub(crate) fn ambient_source(&self, d: Rgb) -> Rgb {
        super::studio(self.preset).map_or([0.0; 3], |r| r.ambient_in(super::unit(d), r.key_white()).map(|c| c * self.calibration))
    }

    /// The levels the shader samples.
    pub(crate) fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Mean radiance of a level over the sphere (solid-angle weighted).
    pub(crate) fn level_mean(&self, level: usize) -> Rgb {
        let Some(l) = self.levels.get(level) else { return [0.0; 3] };
        let height = l.height() as f64;
        let (mut sum, mut total) = ([0.0; 3], 0.0);
        for (y, row) in l.rgb.chunks_exact(l.width).enumerate() {
            let weight = ((y as f64 + 0.5) / height * PI).sin();
            for pixel in row {
                super::add(&mut sum, pixel.map(f64::from), weight);
                total += weight;
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
    let [a, b] = brdf(nv, roughness);
    let ess = (a + b).max(1e-4);
    let r = [2.0 * nv * n[0] - v[0], 2.0 * nv * n[1] - v[1], 2.0 * nv * n[2] - v[2]];
    let prefiltered = env.prefiltered(unturn(r, rotation), env.level_at(roughness));
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
    let [ca, cb] = brdf(nv, coat_roughness);
    let albedo = weight * (coat_ratio * coat_ratio * ca + cb);
    let seen = env.prefiltered(unturn(r, rotation), env.level_at(coat_roughness));
    [specular.map(|s| s * (1.0 - albedo)), seen.map(|c| c * albedo), diffuse.map(|d| d * (1.0 - albedo))]
}
