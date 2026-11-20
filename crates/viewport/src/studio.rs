//! Studios as data: a lighting setup a renderer lights a product with, kept in a studio library
//! (built-in entries ship as data, `assets/render/catalog/studios.json`; users duplicate, edit,
//! import and export their own). A studio has up to two parts:
//!
//! - a **rig** ([`StudioRig`]): rectangular area lights placed in multiples of the subject's
//!   extent B about the studio's centre, over a cyclorama in a finite room. The Realistic view
//!   bakes it into its studio cube, and the GPU path tracer lights a job with the same lights and
//!   cyclorama ([`StudioRig::placed_lights`], [`StudioRig::cyclorama_at`]);
//! - a **composition** ([`StudioComposition`]): an HDR environment, an optional starter rig, a
//!   ground and an exposure, which `render.preset.*` fits into a saved path-traced setup.
//!
//! The rig's mathematics lives here, once, so the view, the path tracer and the tests read the
//! same studio. A studio's light positions and gains come from upstream's starter rigs (`STU-*`,
//! `CAL-LIGHT-001`); the cyclorama is original.
//!
//! Sources (mathematics only; no code or data copied): box-projected reflections (Bjorke 2004, GPU
//! Gems ch. 19; Lagarde and Zanuttini 2012), the irradiance of a uniform polygonal luminaire
//! (Lambert 1760; Arvo 1995).

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

type Rgb = [f64; 3];

/// Most lights a rig binds (the Realistic view's uniform slots; a path job's area lights).
pub const MAX_LIGHTS: usize = 4;
/// Most bytes of a studio's id, name or source; of its description.
pub const MAX_NAME: usize = 128;
pub const MAX_DESCRIPTION: usize = 2048;
/// Largest light radiance a studio may give (relative linear units).
pub const MAX_RADIANCE: f64 = 1e6;

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

fn add(acc: &mut Rgb, c: Rgb, w: f64) {
    for (a, c) in acc.iter_mut().zip(c) {
        *a += c * w;
    }
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Why a studio record was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StudioError(pub String);

impl std::fmt::Display for StudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StudioError {}

fn invalid(message: impl Into<String>) -> StudioError {
    StudioError(message.into())
}

fn finite(values: impl IntoIterator<Item = f64>) -> bool {
    values.into_iter().all(f64::is_finite)
}

/// Whether `id` is a studio identity: 1 to 128 ASCII letters, digits, hyphens or underscores.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_NAME && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One studio of the library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Studio {
    /// The library identity (`NATIVE-STUDIO-026`, or a user studio's own).
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What the studio was made from (an upstream starter preset such as `STU-002`, or the id of
    /// the studio a user duplicated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The rig the Realistic view and the path tracer's studio light with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realistic: Option<StudioRig>,
    /// The composition `render.preset.*` fits into a saved path-traced setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<StudioComposition>,
}

impl Studio {
    /// Check every value: identity and text bounded, a rig's numbers finite and in range.
    pub fn validate(&self) -> Result<(), StudioError> {
        if !valid_id(&self.id) {
            return Err(invalid("a studio id is 1 to 128 ASCII letters, digits, hyphens or underscores"));
        }
        if self.name.trim().is_empty() || self.name.len() > MAX_NAME || self.name.chars().any(char::is_control) {
            return Err(invalid("a studio name is 1 to 128 bytes without control characters"));
        }
        if self.description.len() > MAX_DESCRIPTION || self.source.as_ref().is_some_and(|s| s.len() > MAX_NAME || s.chars().any(char::is_control)) {
            return Err(invalid("a studio description is at most 2048 bytes and its source at most 128"));
        }
        if self.realistic.is_none() && self.path.is_none() {
            return Err(invalid("a studio needs a rig (`realistic`), a composition (`path`) or both"));
        }
        if let Some(rig) = &self.realistic {
            rig.validate()?;
        }
        if let Some(path) = &self.path {
            path.validate()?;
        }
        Ok(())
    }
}

impl StudioRig {
    /// A key for the rig's content (what a renderer bakes from it), the same in every run: FNV-1a
    /// over every number's bits.
    pub fn content_key(&self) -> u64 {
        let mut words: Vec<u64> = vec![self.lights.len() as u64, self.rig_lights as u64];
        for l in &self.lights {
            let numbers = l.centre.into_iter().chain(l.width_axis).chain(l.height_axis).chain(l.radiance);
            words.extend(numbers.chain([l.half_width, l.half_height, l.grade]).map(f64::to_bits));
        }
        let c = &self.cyclorama;
        words.extend(c.floor.into_iter().chain(c.wall).chain(c.band).chain(c.ceiling).chain([c.spill]).map(f64::to_bits));
        words.extend([self.room.half, self.room.floor, self.room.ceiling, self.white_under_key].map(f64::to_bits));
        words.iter().flat_map(|w| w.to_le_bytes()).fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
    }
}

/// One rectangular area light of a rig, in subject extents B about the studio's centre (+Z up,
/// −Y front, +X right): its centre, unit width and height axes (their cross product is the
/// emitting side's normal), half sizes, and calibrated mean radiance (relative linear Rec. 709).
/// `grade` is the share of each half size over which the radiance falls off toward the edge (0: a
/// hard rectangle); the mean over the rectangle stays `radiance`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioLight {
    pub id: String,
    pub centre: Rgb,
    pub width_axis: Rgb,
    pub height_axis: Rgb,
    pub half_width: f64,
    pub half_height: f64,
    pub radiance: Rgb,
    #[serde(default)]
    pub grade: f64,
}

impl StudioLight {
    fn validate(&self) -> Result<(), StudioError> {
        if self.id.len() > MAX_NAME || self.id.chars().any(char::is_control) {
            return Err(invalid("a light id is at most 128 bytes"));
        }
        let numbers = self.centre.into_iter().chain(self.width_axis).chain(self.height_axis).chain(self.radiance);
        if !finite(numbers.chain([self.half_width, self.half_height, self.grade])) {
            return Err(invalid("a studio light's values must be finite"));
        }
        if self.centre.iter().any(|c| c.abs() > 100.0) || !(1e-4..=100.0).contains(&self.half_width) || !(1e-4..=100.0).contains(&self.half_height) {
            return Err(invalid("a studio light stands within 100 B of the centre, its half sizes 0.0001 to 100 B"));
        }
        let (u, v) = (self.width_axis, self.height_axis);
        if (dot(u, u).sqrt() - 1.0).abs() > 1e-6 || (dot(v, v).sqrt() - 1.0).abs() > 1e-6 || dot(u, v).abs() > 1e-6 {
            return Err(invalid("a studio light's width and height axes are perpendicular unit vectors"));
        }
        if self.radiance.iter().any(|c| !(0.0..=MAX_RADIANCE).contains(c)) || !(0.0..=1.0).contains(&self.grade) {
            return Err(invalid("a studio light's radiance is 0 to 1000000 and its grade 0 to 1"));
        }
        Ok(())
    }

    /// The emitting side's unit normal.
    pub fn normal(&self) -> Rgb {
        unit(cross(self.width_axis, self.height_axis))
    }

    /// Corners in order around the rectangle.
    pub fn corners(&self) -> [Rgb; 4] {
        let (u, v) = (self.width_axis, self.height_axis);
        let at = |a: f64, b: f64| -> Rgb { std::array::from_fn(|k| self.centre[k] + u[k] * a * self.half_width + v[k] * b * self.half_height) };
        [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)]
    }

    /// The relative radiance profile at local coordinates `x`, `y` in [−1, 1]: 1 in the middle,
    /// falling smoothly to 0 at the edges over `grade`, scaled so that its mean is 1.
    pub fn profile(&self, x: f64, y: f64) -> f64 {
        graded_profile(self.grade, x, y)
    }

    /// The radiance seen from the studio's centre along unit `d`, and how far along `d` it is
    /// (`None` when `d` misses the light or meets its back).
    fn seen_from_centre(&self, d: Rgb) -> Option<(f64, Rgb)> {
        let n = self.normal();
        let facing = dot(d, n);
        if facing >= 0.0 {
            return None;
        }
        let t = dot(self.centre, n) / facing;
        if t <= 0.0 {
            return None;
        }
        let hit = d.map(|c| c * t);
        let offset: Rgb = std::array::from_fn(|k| hit[k] - self.centre[k]);
        let (x, y) = (dot(offset, self.width_axis) / self.half_width, dot(offset, self.height_axis) / self.half_height);
        if x.abs() > 1.0 || y.abs() > 1.0 {
            return None;
        }
        let p = self.profile(x, y);
        Some((t, self.radiance.map(|c| c * p)))
    }
}

/// The relative radiance profile of a rectangular light graded over `grade` (0 uniform, 1 graded
/// from the middle) at local coordinates `x`, `y` in [−1, 1]: 1 in the middle, falling smoothly
/// to 0 at the edges over `grade` of the half size, scaled so that its mean over the rectangle is
/// 1. Every renderer that draws a studio's graded lights uses this one definition.
pub fn graded_profile(grade: f64, x: f64, y: f64) -> f64 {
    let g = if grade.is_finite() { grade.clamp(0.0, 1.0) } else { 0.0 };
    if g <= 0.0 {
        return 1.0;
    }
    let edge = |t: f64| 1.0 - smoothstep(1.0 - g, 1.0, t.abs());
    // The mean of `edge` over [−1, 1] is 1 − g/2 (smoothstep's mean is a half).
    let mean = (1.0 - 0.5 * g) * (1.0 - 0.5 * g);
    edge(x) * edge(y) / mean
}

/// The finite room reflections are projected against, in subject extents B about the studio's
/// centre: |x|, |y| ≤ `half`, from `floor` (the ground) up to `ceiling`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    pub half: f64,
    pub floor: f64,
    pub ceiling: f64,
}

impl Default for Room {
    /// 4 B across (about four bounding radii), the floor at the ground half an extent below the
    /// centre.
    fn default() -> Self {
        Room { half: 2.0, floor: -0.5, ceiling: 2.5 }
    }
}

impl Room {
    fn validate(&self) -> Result<(), StudioError> {
        if !finite([self.half, self.floor, self.ceiling])
            || !(0.5..=100.0).contains(&self.half)
            || !(-100.0..0.0).contains(&self.floor)
            || !(0.0..=100.0).contains(&self.ceiling)
            || self.ceiling <= 0.0
        {
            return Err(invalid("a studio room is 0.5 to 100 B in half size, its floor below the centre and its ceiling above, within 100 B"));
        }
        Ok(())
    }

    /// Whether `p` lies in the room.
    pub fn contains(&self, p: Rgb) -> bool {
        let e = 1e-9;
        p[0].abs() <= self.half + e && p[1].abs() <= self.half + e && p[2] >= self.floor - e && p[2] <= self.ceiling + e
    }

    /// Where the ray from `p` (inside) along `r` leaves the room (box projection).
    pub fn exit(&self, p: Rgb, r: Rgb) -> Rgb {
        let lo = [-self.half, -self.half, self.floor];
        let hi = [self.half, self.half, self.ceiling];
        let p: Rgb = std::array::from_fn(|k| p[k].clamp(lo[k], hi[k]));
        let mut t = f64::INFINITY;
        for k in 0..3 {
            let s = if r[k].abs() < 1e-9 { 1e-9 } else { r[k] };
            let far = if s > 0.0 { hi[k] } else { lo[k] };
            t = t.min((far - p[k]) / s);
        }
        let t = t.max(0.0);
        std::array::from_fn(|k| p[k] + t * r[k])
    }

    /// The point where the ray from the centre along unit `d` meets the room's walls.
    fn wall(&self, d: Rgb) -> Rgb {
        self.exit([0.0; 3], d)
    }
}

/// A cyclorama by the elevation of the direction from the studio's centre: the floor below,
/// sweeping up into the wall colour toward the horizon, a brighter band just above it, and a soft
/// ceiling overhead. Like a real white cyclorama it is lit by the rig: its radiances are shares of
/// a white diffuse card's under the key alone, and the floor, walls and band are brighter on the
/// key's side (`spill`: the share they gain toward the key's azimuth and lose opposite it).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cyclorama {
    pub floor: Rgb,
    pub wall: Rgb,
    pub band: Rgb,
    pub ceiling: Rgb,
    pub spill: f64,
}

impl Cyclorama {
    fn validate(&self) -> Result<(), StudioError> {
        let shares = self.floor.into_iter().chain(self.wall).chain(self.band).chain(self.ceiling);
        if !finite(shares.clone().chain([self.spill]))
            || shares.into_iter().any(|c| !(0.0..=100.0).contains(&c))
            || !(0.0..=1.0).contains(&self.spill)
        {
            return Err(invalid("a cyclorama's shares are 0 to 100 and its spill 0 to 1"));
        }
        Ok(())
    }

    /// The relative radiance along unit `d`, `key` the key light's direction from the centre.
    pub fn at(&self, d: Rgb, key: Rgb) -> Rgb {
        let e = d[2].clamp(-1.0, 1.0).asin().to_degrees();
        // The floor holds a pool of the key's light around the subject: brighter straight down,
        // fading toward the sweep into the walls.
        let pool = 1.0 + 0.6 * (1.0 - smoothstep(-42.0, -10.0, e));
        let floor = self.floor.map(|v| v * pool);
        let mut c = if e < 0.0 { mix(floor, self.wall, smoothstep(-30.0, -6.0, e)) } else { mix(self.wall, self.ceiling, smoothstep(0.0, 75.0, e)) };
        let band = (-((e - 8.0) / 12.0).powi(2)).exp();
        add(&mut c, self.band, band);
        // Linear in the direction's horizontal part: full toward the horizon, a smooth gradient
        // across the ceiling; the floor, lit from above, takes a third of it.
        let k = (key[0] * key[0] + key[1] * key[1]).sqrt();
        let toward = if k > 1e-9 { (d[0] * key[0] + d[1] * key[1]) / k } else { 0.0 };
        let spill = self.spill * (1.0 - 0.67 * (1.0 - smoothstep(-25.0, 0.0, e)));
        c.map(|v| v * (1.0 + spill * toward))
    }
}

/// How the Realistic view's frames of a studio are judged (docs/material-graph.md §5.9): a
/// dielectric whose base colour's linear luminance is under `dark_albedo` is judged against its
/// own albedo (a highlight `highlight`/255 above its median, faces `distinct`/255 apart), and
/// planar metal faces narrower than `narrow_face` B are not judged for a gradient.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Acceptance {
    pub dark_albedo: f64,
    pub highlight: f64,
    pub distinct: f64,
    pub narrow_face: f64,
}

impl Default for Acceptance {
    fn default() -> Self {
        Acceptance { dark_albedo: 0.05, highlight: 24.0, distinct: 6.0, narrow_face: 0.2 }
    }
}

impl Acceptance {
    fn validate(&self) -> Result<(), StudioError> {
        if !finite([self.dark_albedo, self.highlight, self.distinct, self.narrow_face])
            || !(0.0..=1.0).contains(&self.dark_albedo)
            || !(0.0..=255.0).contains(&self.highlight)
            || !(0.0..=255.0).contains(&self.distinct)
            || !(0.0..=100.0).contains(&self.narrow_face)
        {
            return Err(invalid("acceptance: dark albedo 0 to 1, highlight and distinct 0 to 255, narrow face 0 to 100 B"));
        }
        Ok(())
    }
}

/// The scene-referred value a white diffuse face under the key maps to by default: PBR Neutral
/// leaves it below its shoulder (0.76) and takes 0.04 off, giving 0.6038 linear, 0.80 display.
pub const WHITE_UNDER_KEY: f64 = 0.6438;

fn default_white() -> f64 {
    WHITE_UNDER_KEY
}

/// A studio's rig: lights in a room over a cyclorama, calibrated so that a white Lambertian face
/// at the studio's centre turned to the key (the first light) reaches `white_under_key` before
/// tone mapping. Every emitter has one energy owner per lobe: specular reflections read the
/// studio's radiance ([`StudioRig::radiance_in`]), diffuse shading the lights directly plus the
/// cyclorama's own irradiance, so no light is counted twice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioRig {
    /// The key first.
    pub lights: Vec<StudioLight>,
    /// How many of `lights` come from the starter rig the studio was made from (the rest are the
    /// studio's own additions, such as an overhead scrim).
    #[serde(default)]
    pub rig_lights: usize,
    pub cyclorama: Cyclorama,
    #[serde(default)]
    pub room: Room,
    #[serde(default = "default_white")]
    pub white_under_key: f64,
    #[serde(default)]
    pub acceptance: Acceptance,
}

/// One of a rig's lights placed in the world ([`StudioRig::placed_lights`]): a rectangle about
/// `centre` spanned by the half axes `u` and `v` (world units; it emits toward `u × v`), of mean
/// linear Rec. 709 `radiance`, graded toward its edges over `grade` ([`graded_profile`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedStudioLight {
    pub centre: Rgb,
    pub u: Rgb,
    pub v: Rgb,
    pub radiance: Rgb,
    pub grade: f64,
}

impl StudioRig {
    fn validate(&self) -> Result<(), StudioError> {
        if self.lights.is_empty() || self.lights.len() > MAX_LIGHTS || self.rig_lights > self.lights.len() {
            return Err(invalid("a studio rig has 1 to 4 lights, the key first"));
        }
        for light in &self.lights {
            light.validate()?;
        }
        self.cyclorama.validate()?;
        self.room.validate()?;
        self.acceptance.validate()?;
        if !self.white_under_key.is_finite() || !(0.01..=10.0).contains(&self.white_under_key) {
            return Err(invalid("a studio's white under the key is 0.01 to 10"));
        }
        if !self.lights.iter().all(|l| self.room.contains(l.centre)) {
            return Err(invalid("a studio's lights stand inside its room"));
        }
        Ok(())
    }

    /// The radiance of a white diffuse card at the centre turned to the key, lit by the key alone:
    /// the unit of the cyclorama's radiances.
    pub fn key_white(&self) -> f64 {
        let Some(key) = self.lights.first() else { return 1.0 };
        let [r, g, b] = key.radiance.map(|c| c * polygon_irradiance([0.0; 3], unit(key.centre), &key.corners()) / PI);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The cyclorama's radiance along unit `d` from the studio's centre given [`Self::key_white`]
    /// (what diffuse shading takes from the room apart from the direct lights).
    pub fn ambient_in(&self, d: Rgb, key_white: f64) -> Rgb {
        let key = self.lights.first().map_or([0.0, 0.0, 1.0], |k| unit(k.centre));
        self.cyclorama.at(d, key).map(|c| c * key_white)
    }

    /// Everything seen from the studio's centre along unit `d` given [`Self::key_white`]: the
    /// cyclorama on the room's walls, plus every light the ray meets inside the room (a diffuser
    /// glows over what is behind it, so a graded edge fades into the wall, never to black).
    pub fn radiance_in(&self, d: Rgb, key_white: f64) -> Rgb {
        let wall = self.room.wall(d);
        let far = dot(wall, wall).sqrt();
        let mut c = self.ambient_in(d, key_white);
        for (_, light) in self.lights.iter().filter_map(|light| light.seen_from_centre(d)).filter(|(t, _)| *t <= far + 1e-9) {
            add(&mut c, light, 1.0);
        }
        c
    }

    /// The irradiance the rig's lights give a surface at `p` (studio coordinates) with unit normal
    /// `n`: each light's exact polygon irradiance at its mean radiance, clipped to the surface's
    /// horizon, from its emitting side only.
    pub fn direct_irradiance(&self, p: Rgb, n: Rgb) -> Rgb {
        let mut e = [0.0; 3];
        for light in &self.lights {
            let side: Rgb = std::array::from_fn(|k| p[k] - light.centre[k]);
            if dot(side, light.normal()) <= 0.0 {
                continue;
            }
            add(&mut e, light.radiance, polygon_irradiance(p, n, &light.corners()));
        }
        e
    }

    /// The cyclorama's irradiance on a surface at the studio's centre with unit normal `n`, by a
    /// direct cosine integral.
    pub fn ambient_irradiance(&self, n: Rgb) -> Rgb {
        let (steps_t, steps_p) = (96, 192);
        let key_white = self.key_white();
        let mut e = [0.0; 3];
        for i in 0..steps_t {
            let theta = PI * (f64::from(i) + 0.5) / f64::from(steps_t);
            for j in 0..steps_p {
                let phi = 2.0 * PI * (f64::from(j) + 0.5) / f64::from(steps_p);
                let d = [theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()];
                let cosine = dot(n, d);
                if cosine > 0.0 {
                    let w = theta.sin() * (PI / f64::from(steps_t)) * (2.0 * PI / f64::from(steps_p)) * cosine;
                    add(&mut e, self.ambient_in(d, key_white), w);
                }
            }
        }
        e
    }

    /// The exposure calibration: the scale that maps a white Lambertian face at the studio's
    /// centre, turned to the key, to [`StudioRig::white_under_key`] before tone mapping. The
    /// studio's radiance is used at this scale, so exposure 1 is the calibrated one.
    pub fn calibration(&self) -> f64 {
        let Some(key) = self.lights.first() else { return 1.0 };
        let n = unit(key.centre);
        let [r, g, b] = {
            let mut e = self.direct_irradiance([0.0; 3], n);
            add(&mut e, self.ambient_irradiance(n), 1.0);
            e
        };
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if luminance > 0.0 { self.white_under_key * PI / luminance } else { 1.0 }
    }

    /// The rig's lights for a renderer that lights a scene by the studio itself (a path tracer),
    /// around a subject of bounds `min`..`max` (world units), turned by `rotation` radians about
    /// +Z: each a rectangle at its mean radiance, graded as the view grades it, scaled by `calibration`
    /// ([`StudioRig::calibration`]), the key first. They stand where the view's do: in multiples of
    /// the subject's extent B about the studio's centre, half an extent above the ground over the
    /// subject's centre. `None` for bounds that are not finite or have no extent.
    pub fn placed_lights(&self, min: Rgb, max: Rgb, rotation: f64, calibration: f64) -> Option<Vec<PlacedStudioLight>> {
        if !finite(min.into_iter().chain(max).chain([rotation, calibration])) {
            return None;
        }
        let extent = (0..3).map(|k| max[k] - min[k]).fold(0.0, f64::max);
        if !(extent > 0.0) || !extent.is_finite() {
            return None;
        }
        let centre = [0.5 * (min[0] + max[0]), 0.5 * (min[1] + max[1]), min[2] + 0.5 * extent];
        let (sin, cos) = rotation.sin_cos();
        let turn = |v: Rgb| [cos * v[0] - sin * v[1], sin * v[0] + cos * v[1], v[2]];
        Some(
            self.lights
                .iter()
                .map(|l| {
                    let p = turn(l.centre);
                    PlacedStudioLight {
                        centre: std::array::from_fn(|k| centre[k] + extent * p[k]),
                        u: turn(l.width_axis).map(|c| c * l.half_width * extent),
                        v: turn(l.height_axis).map(|c| c * l.half_height * extent),
                        radiance: l.radiance.map(|c| c * calibration),
                        grade: l.grade,
                    }
                })
                .collect(),
        )
    }
}

/// The irradiance (over unit radiance) a uniform polygon with corners `corners` gives a surface at
/// `p` with unit normal `n`: the polygon clipped to the surface's upper half-space, then
/// ½ Σ θᵢ (n · γᵢ) over its edges (Lambert's formula).
pub fn polygon_irradiance(p: Rgb, n: Rgb, corners: &[Rgb; 4]) -> f64 {
    let v: Vec<Rgb> = corners.iter().map(|c| std::array::from_fn(|k| c[k] - p[k])).collect();
    let mut clipped: Vec<Rgb> = Vec::with_capacity(5);
    for i in 0..v.len() {
        let (a, b) = (v[i], v[(i + 1) % v.len()]);
        let (da, db) = (dot(a, n), dot(b, n));
        if da >= 0.0 {
            clipped.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            clipped.push(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t));
        }
    }
    if clipped.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..clipped.len() {
        let (a, b) = (clipped[i], clipped[(i + 1) % clipped.len()]);
        let (la, lb) = (dot(a, a).sqrt(), dot(b, b).sqrt());
        if la < 1e-12 || lb < 1e-12 {
            continue;
        }
        let (a, b) = (a.map(|c| c / la), b.map(|c| c / lb));
        let c = cross(a, b);
        let lc = dot(c, c).sqrt();
        if lc < 1e-12 {
            continue;
        }
        sum += dot(a, b).clamp(-1.0, 1.0).acos() * dot(n, c) / lc;
    }
    0.5 * sum.abs()
}

/// A studio's composition for a saved path-traced setup (`render.preset.*`): an HDR environment
/// from the built-in library, at `intensity`, an optional starter rig (`RIG-*`), a ground colour
/// and an exposure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioComposition {
    pub hdr: String,
    pub intensity: f64,
    pub exposure_ev: f64,
    pub ground: Rgb,
    #[serde(default)]
    pub rig: Option<String>,
}

impl StudioComposition {
    fn validate(&self) -> Result<(), StudioError> {
        if !valid_id(&self.hdr) || self.rig.as_deref().is_some_and(|r| !valid_id(r)) {
            return Err(invalid("a composition names its HDR and rig by library id"));
        }
        if !finite([self.intensity, self.exposure_ev].into_iter().chain(self.ground))
            || !(0.0..=100.0).contains(&self.intensity)
            || !(-32.0..=32.0).contains(&self.exposure_ev)
            || self.ground.iter().any(|c| !(0.0..=1.0).contains(c))
        {
            return Err(invalid("a composition's intensity is 0 to 100, its exposure -32 to 32 EV and its ground 0 to 1"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "studio_tests.rs"]
mod tests;
