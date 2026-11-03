//! The opt-in *product look* of the real-time PBR preview (`pbr::SOURCE`): a procedural studio
//! (softboxes, a gradient cyclorama and a dark floor) baked on the CPU into a small prefiltered
//! cube map and nine spherical-harmonic irradiance terms, split-sum specular with an analytic
//! environment-BRDF fit and multiple-scattering compensation, Khronos PBR Neutral tone mapping
//! with exposure, and a ±1 LSB triangular dither.
//!
//! Nothing changes unless a frame names a [`ProductLook`] (`Frame::product_look`): the fixed
//! studio and the per-channel Reinhard preview stay the default (docs/render-opaque-pbr.md). The
//! bake needs no compute shaders, so it runs on WebGL2 too; it never runs inside the renderer
//! ([`StudioBake`]).
//!
//! Sources (mathematics only; no code or data copied): split-sum prefiltering (Karis 2013, "Real
//! Shading in Unreal Engine 4"), filtered importance sampling (Křivánek and Colbert 2008, GPU Gems
//! 3 ch. 20), irradiance from nine spherical-harmonic terms (Ramamoorthi and Hanrahan 2001),
//! single-to-multiple scattering compensation (Fdez-Agüera 2019; Filament documentation). The
//! environment-BRDF polynomial is SolidCraft's own least-squares fit to the numerically integrated
//! split-sum terms of the viewport's own BRDF ([`env_brdf`] and its test).

use std::f64::consts::PI;

use wgpu::util::DeviceExt;

/// Side of the prefiltered cube map's sharpest level, in texels.
pub(crate) const ENV_SIZE: u32 = 128;
/// Levels of the prefiltered cube map: level `m` holds perceptual roughness `m / (ENV_MIPS - 1)`.
pub(crate) const ENV_MIPS: u32 = 6;
/// GGX samples per texel of each rough level (filtered importance sampling keeps them smooth).
const SPECULAR_SAMPLES: u32 = 64;
/// The source level the irradiance terms are projected from (32 × 32 texels a face).
const SH_LEVEL: usize = 2;
/// The environment uniform: nine irradiance terms and (largest level, 0, 0, 0).
const ENV_UNIFORM_SIZE: u64 = 10 * 16;

/// A procedural studio the product look lights with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StudioPreset {
    /// Four softboxes (key, fill, an overhead strip and a rim strip) over a light grey cyclorama.
    #[default]
    Product,
    /// Two large softboxes and a bright cyclorama: low contrast, soft highlights.
    Soft,
    /// Two tall strip lights and an overhead panel in a dark studio: crisp highlights on metal.
    Contrast,
}

impl StudioPreset {
    pub const ALL: [StudioPreset; 3] = [StudioPreset::Product, StudioPreset::Soft, StudioPreset::Contrast];

    /// The number a [`ProductLook`] record carries.
    pub fn id(self) -> u32 {
        match self {
            StudioPreset::Product => 0,
            StudioPreset::Soft => 1,
            StudioPreset::Contrast => 2,
        }
    }

    pub fn from_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
}

/// The opt-in product look of a frame (`Frame::product_look`). The frame's product lights, if any,
/// still light the scene, and the look's studio lights it from all around (once installed:
/// `Renderer::set_studio_environment`); the fixed three-light studio of the default preview does
/// not. Without a look a frame keeps the default preview, unchanged.
///
/// With the look, PBR surfaces also show ambient occlusion (from a frame's `ao_radius`) on the
/// studio's and the ambient lights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProductLook {
    /// Scale of the scene-referred radiance (product lights and studio) before tone mapping.
    pub exposure: f32,
    /// The studio reflected and lighting from all around.
    pub studio: StudioPreset,
    /// Turn of the studio about world +Z, radians (counter-clockwise seen from above).
    pub rotation: f32,
    /// Scale of the studio's radiance (0: only the product lights).
    pub intensity: f32,
}

impl Default for ProductLook {
    fn default() -> Self {
        ProductLook { exposure: 1.0, studio: StudioPreset::Product, rotation: 0.0, intensity: 1.0 }
    }
}

impl ProductLook {
    /// Largest exposure and intensity a record keeps.
    pub const MAX_SCALE: f32 = 1024.0;

    /// The uniform the shaders read: (exposure, studio intensity, studio rotation, 1), of
    /// [`ProductLook::sanitized`].
    pub(crate) fn uniform(&self) -> [f32; 4] {
        let look = self.sanitized();
        [look.exposure, look.intensity, look.rotation, 1.0]
    }

    /// The look the renderer draws: out-of-range values clamped, non-finite ones replaced by the
    /// defaults, the rotation within [0, 2π).
    pub fn sanitized(self) -> Self {
        let scale = |x: f32| if x.is_finite() { x.clamp(0.0, Self::MAX_SCALE) } else { 1.0 };
        let rotation = if self.rotation.is_finite() { self.rotation.rem_euclid(std::f32::consts::TAU) } else { 0.0 };
        ProductLook { exposure: scale(self.exposure), studio: self.studio, rotation, intensity: scale(self.intensity) }
    }
}

type Rgb = [f64; 3];

fn dot(a: Rgb, b: Rgb) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Rgb, b: Rgb) -> Rgb {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(a: Rgb) -> Rgb {
    let l = dot(a, a).sqrt();
    if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, 1.0] }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `acc += c × w`.
fn add(acc: &mut Rgb, c: Rgb, w: f64) {
    for (a, c) in acc.iter_mut().zip(c) {
        *a += c * w;
    }
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// A rectangular light seen from the studio's centre: its centre direction (degrees: azimuth
/// from +X toward +Y, elevation above the horizon), its angular half extents across and along the
/// elevation (degrees), and its relative radiance (linear RGB).
struct Softbox {
    azimuth: f64,
    elevation: f64,
    half_width: f64,
    half_height: f64,
    radiance: Rgb,
}

/// A procedural studio: up to four softboxes over a cyclorama that runs from `horizon` to
/// `zenith` overhead and sweeps down into a dark `floor` below the horizon. Radiance is relative
/// (no photometric calibration); [`ProductLook::intensity`] scales it.
struct Studio {
    boxes: &'static [Softbox],
    zenith: Rgb,
    horizon: Rgb,
    floor: Rgb,
}

const WARM: Rgb = [1.0, 0.97, 0.93];
const NEUTRAL: Rgb = [1.0, 1.0, 1.0];
const COOL: Rgb = [0.93, 0.97, 1.0];

const fn scaled(c: Rgb, s: f64) -> Rgb {
    [c[0] * s, c[1] * s, c[2] * s]
}

const PRODUCT: Studio = Studio {
    boxes: &[
        Softbox { azimuth: 135.0, elevation: 40.0, half_width: 22.0, half_height: 16.0, radiance: scaled(WARM, 4.0) },
        Softbox { azimuth: 20.0, elevation: 25.0, half_width: 18.0, half_height: 14.0, radiance: scaled(NEUTRAL, 1.4) },
        Softbox { azimuth: 270.0, elevation: 72.0, half_width: 34.0, half_height: 5.0, radiance: scaled(NEUTRAL, 3.0) },
        Softbox { azimuth: 300.0, elevation: 18.0, half_width: 5.0, half_height: 20.0, radiance: scaled(COOL, 3.0) },
    ],
    zenith: [0.05, 0.05, 0.055],
    horizon: [0.26, 0.26, 0.27],
    floor: [0.02, 0.02, 0.02],
};

const SOFT: Studio = Studio {
    boxes: &[
        Softbox { azimuth: 120.0, elevation: 45.0, half_width: 35.0, half_height: 25.0, radiance: scaled(WARM, 2.0) },
        Softbox { azimuth: 300.0, elevation: 35.0, half_width: 35.0, half_height: 25.0, radiance: scaled(NEUTRAL, 1.3) },
    ],
    zenith: [0.22, 0.22, 0.23],
    horizon: [0.45, 0.45, 0.46],
    floor: [0.06, 0.06, 0.06],
};

const CONTRAST: Studio = Studio {
    boxes: &[
        Softbox { azimuth: 150.0, elevation: 30.0, half_width: 6.0, half_height: 30.0, radiance: scaled(NEUTRAL, 7.0) },
        Softbox { azimuth: 330.0, elevation: 30.0, half_width: 6.0, half_height: 30.0, radiance: scaled(COOL, 5.0) },
        Softbox { azimuth: 0.0, elevation: 80.0, half_width: 20.0, half_height: 10.0, radiance: scaled(WARM, 2.0) },
    ],
    zenith: [0.01, 0.01, 0.012],
    horizon: [0.04, 0.04, 0.045],
    floor: [0.005, 0.005, 0.005],
};

fn studio(preset: StudioPreset) -> &'static Studio {
    match preset {
        StudioPreset::Product => &PRODUCT,
        StudioPreset::Soft => &SOFT,
        StudioPreset::Contrast => &CONTRAST,
    }
}

/// Softbox edges fade over this angle (radians), about a texel of the sharpest level.
const EDGE: f64 = 0.75 * PI / 180.0;

/// The studio's radiance arriving from unit direction `d` (Z up, unrotated).
fn studio_radiance(studio: &Studio, d: Rgb) -> Rgb {
    let z = d[2].clamp(-1.0, 1.0);
    let mut c = if z >= 0.0 {
        mix(studio.horizon, studio.zenith, smoothstep(0.0, 1.0, z.sqrt()))
    } else {
        // The cyclorama's sweep: from the wall's colour at the horizon into the floor.
        mix(studio.horizon, studio.floor, smoothstep(0.0, 0.2, -z))
    };
    for b in studio.boxes {
        let (sa, ca) = b.azimuth.to_radians().sin_cos();
        let (se, ce) = b.elevation.to_radians().sin_cos();
        let centre = [ca * ce, sa * ce, se];
        let across = unit(cross([0.0, 0.0, 1.0], centre));
        let along = cross(centre, across);
        let forward = dot(d, centre);
        if forward <= 0.0 {
            continue;
        }
        let x = dot(d, across).atan2(forward);
        let y = dot(d, along).atan2(forward);
        let (w, h) = (b.half_width.to_radians(), b.half_height.to_radians());
        let inside = (1.0 - smoothstep(w - EDGE, w + EDGE, x.abs())) * (1.0 - smoothstep(h - EDGE, h + EDGE, y.abs()));
        if inside <= 0.0 {
            continue;
        }
        // A diffuser is a little brighter at its centre.
        let (u, v) = ((x / w).clamp(-1.0, 1.0), (y / h).clamp(-1.0, 1.0));
        let profile = 0.85 + 0.15 * (1.0 - u * u) * (1.0 - v * v);
        add(&mut c, b.radiance, inside * profile);
    }
    c
}

/// The unit direction through cube face `face` (+X, −X, +Y, −Y, +Z, −Z) at face coordinates
/// `s`, `t` in [−1, 1] (`t` down the rows): the cube-map convention every wgpu backend samples.
fn face_direction(face: usize, s: f64, t: f64) -> Rgb {
    unit(match face {
        0 => [1.0, -t, -s],
        1 => [-1.0, -t, s],
        2 => [s, 1.0, t],
        3 => [s, -1.0, -t],
        4 => [s, -t, 1.0],
        _ => [-s, -t, -1.0],
    })
}

/// The face and face coordinates (`s`, `t` in [−1, 1]) a direction meets.
fn direction_face(d: Rgb) -> (usize, f64, f64) {
    let [x, y, z] = d;
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    let (face, sc, tc, ma) = if ax >= ay && ax >= az {
        if x >= 0.0 { (0, -z, -y, ax) } else { (1, z, -y, ax) }
    } else if ay >= az {
        if y >= 0.0 { (2, x, z, ay) } else { (3, x, -z, ay) }
    } else if z >= 0.0 {
        (4, x, -y, az)
    } else {
        (5, -x, -y, az)
    };
    let ma = ma.max(1e-12);
    (face, (sc / ma).clamp(-1.0, 1.0), (tc / ma).clamp(-1.0, 1.0))
}

/// One cube level: six faces of `size` × `size` linear RGB texels, rows from the top.
#[derive(Clone)]
struct Level {
    size: usize,
    texels: Vec<[f32; 3]>,
}

impl Level {
    fn texel(&self, face: usize, x: usize, y: usize) -> Rgb {
        let i = (face * self.size + y.min(self.size - 1)) * self.size + x.min(self.size - 1);
        self.texels.get(i).map_or([0.0; 3], |c| c.map(f64::from))
    }

    /// Bilinear within the face the direction meets (clamped at its edges).
    fn bilinear(&self, d: Rgb) -> Rgb {
        let (face, s, t) = direction_face(d);
        let n = self.size as f64;
        let x = ((s + 1.0) * 0.5 * n - 0.5).clamp(0.0, n - 1.0);
        let y = ((t + 1.0) * 0.5 * n - 0.5).clamp(0.0, n - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let a = mix(self.texel(face, x0, y0), self.texel(face, x0 + 1, y0), fx);
        let b = mix(self.texel(face, x0, y0 + 1), self.texel(face, x0 + 1, y0 + 1), fx);
        mix(a, b, fy)
    }

    /// The next level down: each texel the mean of the 2 × 2 under it.
    fn halved(&self) -> Level {
        let size = (self.size / 2).max(1);
        let mut texels = Vec::with_capacity(6 * size * size);
        for face in 0..6 {
            for y in 0..size {
                for x in 0..size {
                    let mut c = [0.0; 3];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let t = self.texel(face, 2 * x + dx, 2 * y + dy);
                        add(&mut c, t, 0.25);
                    }
                    texels.push(c.map(|v| v as f32));
                }
            }
        }
        Level { size, texels }
    }
}

/// Trilinear lookup in a pyramid of levels.
fn sample(levels: &[Level], d: Rgb, lod: f64) -> Rgb {
    let top = levels.len().saturating_sub(1) as f64;
    let lod = lod.clamp(0.0, top);
    let l0 = lod.floor() as usize;
    let l1 = (l0 + 1).min(levels.len().saturating_sub(1));
    let (Some(a), Some(b)) = (levels.get(l0), levels.get(l1)) else { return [0.0; 3] };
    mix(a.bilinear(d), b.bilinear(d), lod - l0 as f64)
}

/// The `i`-th of `n` points of the Hammersley set.
fn hammersley(i: u32, n: u32) -> (f64, f64) {
    ((f64::from(i) + 0.5) / f64::from(n), f64::from(i.reverse_bits()) / 4_294_967_296.0)
}

/// A studio baked for the product look: the prefiltered cube levels (level `m` the radiance
/// reflected at perceptual roughness `m / (levels - 1)`, the sharpest at 0) and the irradiance
/// polynomial. Plain data, so it can be baked on any thread ([`StudioEnvironment::bake`]) and
/// handed to [`crate::view::Renderer::set_studio_environment`].
#[derive(Clone)]
pub struct StudioEnvironment {
    preset: StudioPreset,
    levels: Vec<Level>,
    /// Irradiance E(n) = Σ k_i p_i(n) over p = (1, x, y, z, xy, yz, xz, z², x² − y²).
    irradiance: [Rgb; 9],
}

/// Where a bake's radiance comes from.
#[derive(Clone, Copy)]
enum Radiance {
    Studio(&'static Studio),
    /// The same radiance everywhere (the white furnace of the tests).
    #[cfg(test)]
    Constant(f64),
}

impl Radiance {
    fn at(self, d: Rgb) -> Rgb {
        match self {
            Radiance::Studio(s) => studio_radiance(s, d),
            #[cfg(test)]
            Radiance::Constant(c) => [c; 3],
        }
    }
}

/// A studio bake in bounded steps ([`StudioBake::step`]), for a client that cannot bake on another
/// thread (the web has no `std::thread`) or wants to spread the work over frames. The renderer
/// never bakes: a client bakes (all at once off the window thread with
/// [`StudioEnvironment::bake`], or step by step here) and installs the result with
/// `Renderer::set_studio_environment`.
pub struct StudioBake {
    preset: StudioPreset,
    radiance: Radiance,
    /// The sharpest level's texels so far, while it is being drawn.
    sharp: Vec<[f32; 3]>,
    /// The source pyramid (the sharpest level halved down), once the sharpest level is drawn.
    source: Vec<Level>,
    irradiance: [Rgb; 9],
    /// The prefiltered levels done so far, and the texels of the one in progress.
    levels: Vec<Level>,
    current: Vec<[f32; 3]>,
    /// The GGX samples (cos θ, φ, source lod) of the level in progress.
    samples: Vec<(f64, f64, f64)>,
    /// The next row of the level in progress, counted over its six faces.
    row: usize,
}

impl StudioBake {
    /// Work units per row: the sharpest level evaluates the radiance 4 times a texel, a rough
    /// level takes [`SPECULAR_SAMPLES`] samples a texel.
    const SHARP_SAMPLES: usize = 4;
    /// A step size that keeps a frame's share of the bake short: at most 1.8 ms in an optimised
    /// native build (measured, docs/render-opaque-pbr.md; a debug or wasm build is slower), 152
    /// steps a studio.
    pub const FRAME_WORK: usize = 16_384;

    pub fn new(preset: StudioPreset) -> Self {
        Self::with_radiance(preset, Radiance::Studio(studio(preset)))
    }

    fn with_radiance(preset: StudioPreset, radiance: Radiance) -> Self {
        let size = ENV_SIZE as usize;
        StudioBake {
            preset,
            radiance,
            sharp: Vec::with_capacity(6 * size * size),
            source: Vec::new(),
            irradiance: [[0.0; 3]; 9],
            levels: Vec::new(),
            current: Vec::new(),
            samples: Vec::new(),
            row: 0,
        }
    }

    /// The studio being baked.
    pub fn preset(&self) -> StudioPreset {
        self.preset
    }

    /// Whether every level is baked.
    pub fn is_done(&self) -> bool {
        !self.source.is_empty() && self.levels.len() >= self.source.len()
    }

    /// Bake whole rows until about `work` units are done (at least one row; a unit is one
    /// radiance or GGX sample). Returns whether the bake is done.
    pub fn step(&mut self, work: usize) -> bool {
        let mut done = 0;
        while !self.is_done() {
            done += self.row_step();
            if done >= work {
                break;
            }
        }
        self.is_done()
    }

    /// The baked studio, once [`StudioBake::is_done`] (`None` before).
    pub fn finish(self) -> Option<StudioEnvironment> {
        self.is_done().then_some(StudioEnvironment { preset: self.preset, levels: self.levels, irradiance: self.irradiance })
    }

    /// One row of the level in progress; its work units.
    fn row_step(&mut self) -> usize {
        if self.source.is_empty() {
            return self.sharp_row();
        }
        let m = self.levels.len();
        let Some(size) = self.source.get(m).map(|l| l.size) else { return 1 };
        if self.row == 0 {
            self.samples = specular_samples(m);
            self.current = Vec::with_capacity(6 * size * size);
        }
        let (face, y) = (self.row / size, self.row % size);
        for x in 0..size {
            let c = prefilter(&self.source, &self.samples, face, x, y, size);
            self.current.push(c);
        }
        self.row += 1;
        if self.row >= 6 * size {
            self.levels.push(Level { size, texels: std::mem::take(&mut self.current) });
            self.row = 0;
        }
        size * SPECULAR_SAMPLES as usize
    }

    /// One row of the sharpest level: the radiance at 2 × 2 points in each texel. After its last
    /// row, the source pyramid and the irradiance terms.
    fn sharp_row(&mut self) -> usize {
        let size = ENV_SIZE as usize;
        let n = size as f64;
        let (face, y) = (self.row / size, self.row % size);
        for x in 0..size {
            let mut c = [0.0; 3];
            for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let d = face_direction(face, 2.0 * (x as f64 + ox) / n - 1.0, 2.0 * (y as f64 + oy) / n - 1.0);
                add(&mut c, self.radiance.at(d), 0.25);
            }
            self.sharp.push(c.map(|v| v as f32));
        }
        self.row += 1;
        if self.row >= 6 * size {
            let mut source = vec![Level { size, texels: std::mem::take(&mut self.sharp) }];
            while source.len() < ENV_MIPS as usize {
                let Some(last) = source.last() else { break };
                let next = last.halved();
                source.push(next);
            }
            self.irradiance = StudioEnvironment::project(source.get(SH_LEVEL).or(source.last()));
            if let Some(sharp) = source.first() {
                self.levels.push(sharp.clone());
            }
            self.source = source;
            self.row = 0;
        }
        size * Self::SHARP_SAMPLES
    }
}

/// The GGX samples (cos θ, φ, source lod) prefiltering level `m` takes, with the source level
/// each reads (filtered importance sampling).
fn specular_samples(m: usize) -> Vec<(f64, f64, f64)> {
    let top = (ENV_MIPS - 1) as f64;
    let n = ENV_SIZE as f64;
    // One texel's solid angle at the sharpest level.
    let texel_angle = 4.0 * PI / (6.0 * n * n);
    let roughness = m as f64 / top;
    let a2 = roughness.powi(4);
    (0..SPECULAR_SAMPLES)
        .map(|i| {
            let (u1, u2) = hammersley(i, SPECULAR_SAMPLES);
            let cos_t = ((1.0 - u1) / (1.0 + (a2 - 1.0) * u1)).sqrt();
            let denominator = cos_t * cos_t * (a2 - 1.0) + 1.0;
            let pdf = a2 / (PI * denominator * denominator) / 4.0;
            let lod = 0.5 * (1.0 / (f64::from(SPECULAR_SAMPLES) * pdf.max(1e-12) * texel_angle)).log2() + 1.0;
            (cos_t, 2.0 * PI * u2, lod)
        })
        .collect()
}

/// One prefiltered texel (`x`, `y` of `face` at a level of `size`) from the source pyramid.
fn prefilter(source: &[Level], samples: &[(f64, f64, f64)], face: usize, x: usize, y: usize, size: usize) -> [f32; 3] {
    let s = 2.0 * (x as f64 + 0.5) / size as f64 - 1.0;
    let t = 2.0 * (y as f64 + 0.5) / size as f64 - 1.0;
    let normal = face_direction(face, s, t);
    let helper = if normal[2].abs() < 0.999 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let tx = unit(cross(helper, normal));
    let ty = cross(normal, tx);
    let (mut sum, mut weight) = ([0.0; 3], 0.0);
    for &(cos_t, phi, lod) in samples {
        let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
        let (sp, cp) = phi.sin_cos();
        let h = [
            tx[0] * sin_t * cp + ty[0] * sin_t * sp + normal[0] * cos_t,
            tx[1] * sin_t * cp + ty[1] * sin_t * sp + normal[1] * cos_t,
            tx[2] * sin_t * cp + ty[2] * sin_t * sp + normal[2] * cos_t,
        ];
        let nh = dot(normal, h);
        let l = [2.0 * nh * h[0] - normal[0], 2.0 * nh * h[1] - normal[1], 2.0 * nh * h[2] - normal[2]];
        let nl = dot(normal, l);
        if nl <= 0.0 {
            continue;
        }
        let c = sample(source, unit(l), lod);
        add(&mut sum, c, nl);
        weight += nl;
    }
    let w = if weight > 0.0 { 1.0 / weight } else { 0.0 };
    sum.map(|v| (v * w) as f32)
}

impl StudioEnvironment {
    /// Bake `preset` all at once (about 1 MB of texels; tens of milliseconds in an optimised
    /// build). Pure CPU work with no GPU or I/O: run it off the window thread, or bake in steps
    /// with [`StudioBake`] where there are no threads.
    pub fn bake(preset: StudioPreset) -> Self {
        Self::run(StudioBake::new(preset))
    }

    fn run(mut bake: StudioBake) -> Self {
        let preset = bake.preset;
        while !bake.step(usize::MAX) {}
        bake.finish().unwrap_or(StudioEnvironment { preset, levels: Vec::new(), irradiance: [[0.0; 3]; 9] })
    }

    /// The studio this was baked from.
    pub fn preset(&self) -> StudioPreset {
        self.preset
    }

    /// A studio of constant radiance `c` everywhere.
    #[cfg(test)]
    fn constant(c: f64) -> Self {
        Self::run(StudioBake::with_radiance(StudioPreset::Product, Radiance::Constant(c)))
    }

    /// The irradiance polynomial of a level's radiance: its nine spherical-harmonic terms, each
    /// convolved with the clamped cosine (Ramamoorthi and Hanrahan 2001, equations 12–13).
    fn project(level: Option<&Level>) -> [Rgb; 9] {
        let Some(level) = level else { return [[0.0; 3]; 9] };
        let n = level.size as f64;
        let mut l = [[0.0; 3]; 9];
        let mut total = 0.0;
        for face in 0..6 {
            for y in 0..level.size {
                for x in 0..level.size {
                    let s = 2.0 * (x as f64 + 0.5) / n - 1.0;
                    let t = 2.0 * (y as f64 + 0.5) / n - 1.0;
                    let d = face_direction(face, s, t);
                    let angle = (2.0 / n) * (2.0 / n) / (1.0 + s * s + t * t).powf(1.5);
                    total += angle;
                    let [dx, dy, dz] = d;
                    let basis = [
                        0.282095,
                        0.488603 * dy,
                        0.488603 * dz,
                        0.488603 * dx,
                        1.092548 * dx * dy,
                        1.092548 * dy * dz,
                        0.315392 * (3.0 * dz * dz - 1.0),
                        1.092548 * dx * dz,
                        0.546274 * (dx * dx - dy * dy),
                    ];
                    let c = level.texel(face, x, y);
                    for (term, b) in l.iter_mut().zip(basis) {
                        add(term, c, b * angle);
                    }
                }
            }
        }
        // The texels' solid angles sum to a little off 4π: rescale so a constant is exact.
        let fix = if total > 0.0 { 4.0 * PI / total } else { 0.0 };
        let l = l.map(|t| t.map(|v| v * fix));
        let (c1, c2, c3, c4, c5) = (0.429043, 0.511664, 0.743125, 0.886227, 0.247708);
        let [l00, l1m1, l10, l11, l2m2, l2m1, l20, l21, l22] = l;
        let term = |f: &dyn Fn(usize) -> f64| [f(0), f(1), f(2)];
        [
            term(&|k| c4 * l00[k] - c5 * l20[k]),
            term(&|k| 2.0 * c2 * l11[k]),
            term(&|k| 2.0 * c2 * l1m1[k]),
            term(&|k| 2.0 * c2 * l10[k]),
            term(&|k| 2.0 * c1 * l2m2[k]),
            term(&|k| 2.0 * c1 * l2m1[k]),
            term(&|k| 2.0 * c1 * l21[k]),
            term(&|k| c3 * l20[k]),
            term(&|k| c1 * l22[k]),
        ]
    }

    /// The uniform the shader reads: the nine irradiance terms, then (largest level, 0, 0, 0).
    fn uniform_bytes(&self) -> Vec<u8> {
        let mut v: Vec<f32> = self.irradiance.iter().flat_map(|t| [t[0] as f32, t[1] as f32, t[2] as f32, 0.0]).collect();
        v.extend([self.levels.len().saturating_sub(1) as f32, 0.0, 0.0, 0.0]);
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
}

/// The nearest half-precision float to a finite, non-negative `x` (larger values: the largest
/// half, 65504; NaN and negatives: 0).
fn half(x: f32) -> u16 {
    if !(x > 0.0) {
        return 0;
    }
    if x >= 65504.0 {
        return 0x7bff;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        // Subnormal: the implicit one shifted in, rounded to nearest.
        if exponent < -10 {
            return 0;
        }
        let m = mantissa | 0x80_0000;
        let shift = (14 - exponent) as u32;
        let rounded = (m + (1 << (shift - 1))) >> shift;
        return rounded as u16;
    }
    let rounded = (((exponent as u32) << 10) | (mantissa >> 13)) + ((mantissa >> 12) & 1);
    rounded.min(0x7bff) as u16
}

/// The environment's bind group (group 3 of the PBR pipeline): the prefiltered cube, its sampler
/// and the irradiance uniform.
pub(crate) fn layout(device: &wgpu::Device, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sc_studio"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    })
}

pub(crate) fn sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sc_studio"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

/// A studio on the GPU.
pub(crate) struct EnvGpu {
    pub(crate) bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
    _uniform: wgpu::Buffer,
}

fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, texture: wgpu::Texture, uniform: wgpu::Buffer) -> EnvGpu {
    let view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sc_studio"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
        ],
    });
    EnvGpu { bind, _texture: texture, _uniform: uniform }
}

fn cube(device: &wgpu::Device, size: u32, mips: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_studio"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// The stand-in bound while no studio is (an all-black 1 × 1 cube): the pipeline always has its
/// group, and a frame without the look never reads it.
pub(crate) fn placeholder(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler) -> EnvGpu {
    let texture = cube(device, 1, 1);
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sc_studio"),
        contents: &[0; ENV_UNIFORM_SIZE as usize],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    bind(device, layout, sampler, texture, uniform)
}

/// Upload a baked studio.
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    env: &StudioEnvironment,
) -> EnvGpu {
    let mips = env.levels.len().max(1) as u32;
    let size = env.levels.first().map_or(1, |l| l.size.max(1)) as u32;
    let texture = cube(device, size, mips);
    for (mip, level) in env.levels.iter().enumerate() {
        let side = level.size as u32;
        let face_texels = level.size * level.size;
        for face in 0..6 {
            let bytes: Vec<u8> = level
                .texels
                .iter()
                .skip(face * face_texels)
                .take(face_texels)
                .flat_map(|c| [half(c[0]), half(c[1]), half(c[2]), 0x3c00])
                .flat_map(u16::to_le_bytes)
                .collect();
            if bytes.len() != face_texels * 8 {
                continue;
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: face as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(side * 8), rows_per_image: Some(side) },
                wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            );
        }
    }
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sc_studio"),
        contents: &env.uniform_bytes(),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    bind(device, layout, sampler, texture, uniform)
}

/// The renderer's studios: the stand-in and each preset uploaded so far (at most one per preset).
pub(crate) struct Studios {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    placeholder: EnvGpu,
    baked: Vec<(StudioPreset, EnvGpu)>,
}

impl Studios {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layout = layout(device, wgpu::ShaderStages::FRAGMENT);
        let sampler = sampler(device);
        let placeholder = placeholder(device, &layout, &sampler);
        Studios { layout, sampler, placeholder, baked: Vec::new() }
    }

    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Whether `preset` is on the GPU.
    pub(crate) fn has(&self, preset: StudioPreset) -> bool {
        self.baked.iter().any(|(p, _)| *p == preset)
    }

    /// Upload `env`, replacing the same preset's studio.
    pub(crate) fn install(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, env: &StudioEnvironment) {
        let gpu = upload(device, queue, &self.layout, &self.sampler, env);
        self.baked.retain(|(p, _)| *p != env.preset());
        self.baked.push((env.preset(), gpu));
    }

    /// The bind group for a frame's look (the stand-in without one, or before its studio is up).
    pub(crate) fn bind(&self, preset: Option<StudioPreset>) -> &wgpu::BindGroup {
        preset.and_then(|preset| self.baked.iter().find(|(p, _)| *p == preset)).map_or(&self.placeholder.bind, |(_, gpu)| &gpu.bind)
    }
}

/// The environment-BRDF fit the shader evaluates (`pbr_env_brdf` in `pbr::SOURCE`): the split-sum
/// scale `A` and bias `B` (E = F0·A + F90·B) of the viewport's GGX/Smith BRDF, as polynomials in
/// perceptual roughness `r` and `t = 1 − n·v`: P(r, t) + (1 − G1(n·v)) Q(r, t). Rows are r⁰, r¹,
/// r²; columns t⁰ … t³. SolidCraft's own least-squares fit to [`reference::dfg`] over n·v in
/// [0.02, 1] and r in [0.045, 1]: largest error 0.046 (A) and 0.023 (B), RMS 0.006 and 0.004.
#[cfg(test)]
pub(crate) const ENV_BRDF_A: [[[f32; 4]; 3]; 2] = [
    [[1.00560, -0.56397, 2.50123, -2.93657], [0.32333, -0.09745, -2.44274, 2.89048], [-1.07421, 4.87385, -9.53132, 5.20517]],
    [[-27.45309, 62.21089, -24.05595, -10.77288], [10.98918, 41.63909, -157.76626, 106.99327], [9.57360, -85.17991, 167.93185, -93.69565]],
];
#[cfg(test)]
pub(crate) const ENV_BRDF_B: [[[f32; 4]; 3]; 2] = [
    [[-0.02603, 0.65203, -2.71929, 3.09764], [0.09148, -3.19509, 11.5273, -9.30966], [-0.07161, 5.26964, -16.16043, 8.05334]],
    [[-30.15892, 134.44678, -183.67455, 78.98073], [44.8516, -207.80166, 299.57468, -136.8757], [-20.09456, 90.78933, -127.08962, 59.86296]],
];

/// [`ENV_BRDF_A`] and [`ENV_BRDF_B`] at `n·v` and perceptual roughness `r`, each in [0, 1] and
/// their sum (the white-furnace albedo of F0 = 1) at most one.
#[cfg(test)]
pub(crate) fn env_brdf(nv: f32, r: f32) -> (f32, f32) {
    let nv = nv.clamp(1e-4, 1.0);
    let a2 = (r * r) * (r * r);
    let g = 2.0 * nv / (nv + (a2 + (1.0 - a2) * nv * nv).sqrt());
    let t = 1.0 - nv;
    let poly = |c: &[[f32; 4]; 3]| {
        let mut rj = 1.0;
        let mut sum = 0.0;
        for row in c {
            sum += rj * (row[0] + t * (row[1] + t * (row[2] + t * row[3])));
            rj *= r;
        }
        sum
    };
    let a = poly(&ENV_BRDF_A[0]) + (1.0 - g) * poly(&ENV_BRDF_A[1]);
    let b = poly(&ENV_BRDF_B[0]) + (1.0 - g) * poly(&ENV_BRDF_B[1]);
    let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
    // A surface reflects no more than it receives: the fit's albedo stays at or below one.
    let total = (a + b).max(1.0);
    (a / total, b / total)
}

/// Khronos PBR Neutral tone mapping (`look_tone` in the viewport shader): linear Rec. 709 in,
/// [0, 1] out. Adapted from the Khronos Group's PBR Neutral reference (Apache-2.0; see
/// ATTRIBUTION.md).
#[cfg(test)]
pub(crate) fn pbr_neutral(c: [f32; 3]) -> [f32; 3] {
    const START: f32 = 0.8 - 0.04;
    const DESATURATION: f32 = 0.15;
    let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    let x = c[0].min(c[1]).min(c[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let c = c.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c;
    }
    let d = 1.0 - START;
    let new_peak = 1.0 - d * d / (peak + d - START);
    let c = c.map(|v| v * new_peak / peak);
    let g = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    c.map(|v| v + (new_peak - v) * g)
}

#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;
