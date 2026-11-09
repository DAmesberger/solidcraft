//! The Realistic look's default studios: upstream's starter lighting rigs (`STU-*`,
//! `assets/render/starter/lighting/exports/rigs.json`) bound in real time.
//!
//! A rig's rectangular area lights are placed in multiples of the subject's extent B about the
//! subject (the rig's own convention, `source_subject_scale_policy`), inside a finite room sized
//! to the scene: x and y within ±[`Room::half`] B, the floor at the ground and the ceiling
//! [`Room::ceiling`] B above the studio's centre. The studio's centre sits half an extent above
//! the ground, over the subject's centre. Around the lights runs a cyclorama: a mid-grey floor
//! that sweeps up into a brighter horizon band and a soft ceiling, never darker than about a
//! tenth of the studio's mean radiance.
//!
//! The lights are both baked into the studio cube (what reflections see, box-projected against
//! the room) and evaluated as direct lights (what diffuse surfaces receive, by the exact
//! irradiance of a polygon, clipped to the surface's horizon). Each emitter has one energy
//! owner per lobe: specular reflections read the cube, diffuse shading reads the direct lights
//! plus the cyclorama's own irradiance, so no light is counted twice.
//!
//! Sources (mathematics only; no code or data copied): box-projected cube-map reflections
//! (Bjorke 2004, GPU Gems ch. 19; Lagarde and Zanuttini 2012, "Local image-based lighting with
//! parallax-corrected cubemaps"), the irradiance of a uniform polygonal luminaire (Lambert 1760;
//! Arvo 1995, "Applications of irradiance tensors to the simulation of non-Lambertian
//! phenomena"). The rigs' positions, sizes and calibrated gains are upstream's own
//! (`CAL-LIGHT-001`, MIT OR Apache-2.0); the test `the_bound_rigs_are_upstreams` checks them
//! against the export. A studio may add its own lights after the exported ones ([`Rig::exported`]),
//! each documented where it is defined.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

use super::{Rgb, add, cross, dot, mix, smoothstep, unit};

/// Most lights a rig binds (the uniform's slots; `StudioLighting` allows four area lights).
pub(crate) const MAX_LIGHTS: usize = 4;

/// One rectangular area light of a rig, in subject extents B about the subject's centre (+Z up,
/// −Y front, +X right): its centre, unit width and height axes (their cross product is the
/// emitting side's normal), half sizes, and calibrated mean radiance (relative linear sRGB).
/// `grade` is the share of each half size over which the radiance falls off toward the edge
/// (0: a hard rectangle); the mean over the rectangle stays the calibrated radiance.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RigLight {
    /// The export's light id (the tests match lights by it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) id: &'static str,
    pub(crate) centre: Rgb,
    pub(crate) width_axis: Rgb,
    pub(crate) height_axis: Rgb,
    pub(crate) half_width: f64,
    pub(crate) half_height: f64,
    pub(crate) radiance: Rgb,
    pub(crate) grade: f64,
}

impl RigLight {
    pub(crate) fn normal(&self) -> Rgb {
        unit(cross(self.width_axis, self.height_axis))
    }

    /// Corners in order around the rectangle.
    pub(crate) fn corners(&self) -> [Rgb; 4] {
        let (u, v) = (self.width_axis, self.height_axis);
        let at = |a: f64, b: f64| -> Rgb { std::array::from_fn(|k| self.centre[k] + u[k] * a * self.half_width + v[k] * b * self.half_height) };
        [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)]
    }

    /// The relative radiance profile at local coordinates `x`, `y` in [−1, 1]: 1 in the middle,
    /// falling smoothly to 0 at the edges over `grade`, scaled so that its mean is 1.
    pub(crate) fn profile(&self, x: f64, y: f64) -> f64 {
        let g = self.grade.clamp(0.0, 1.0);
        let edge = |t: f64| 1.0 - smoothstep(1.0 - g, 1.0, t.abs());
        // The mean of `edge` over [−1, 1] is 1 − g/2 (smoothstep's mean is a half).
        let mean = (1.0 - 0.5 * g) * (1.0 - 0.5 * g);
        edge(x) * edge(y) / mean
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

/// The finite room reflections are projected against, in subject extents B about the studio's
/// centre: |x|, |y| ≤ `half`, from `floor` (the ground) up to `ceiling`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Room {
    pub(crate) half: f64,
    pub(crate) floor: f64,
    pub(crate) ceiling: f64,
}

/// Every studio's room: 4 B across (about four bounding radii), the floor at the ground half an
/// extent below the centre.
pub(crate) const ROOM: Room = Room { half: 2.0, floor: -0.5, ceiling: 2.5 };

impl Room {
    #[cfg(test)]
    pub(crate) fn contains(&self, p: Rgb) -> bool {
        let e = 1e-9;
        p[0].abs() <= self.half + e && p[1].abs() <= self.half + e && p[2] >= self.floor - e && p[2] <= self.ceiling + e
    }

    /// Where the ray from `p` (inside) along `r` leaves the room (box projection).
    pub(crate) fn exit(&self, p: Rgb, r: Rgb) -> Rgb {
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
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cyclorama {
    pub(crate) floor: Rgb,
    pub(crate) wall: Rgb,
    pub(crate) band: Rgb,
    pub(crate) ceiling: Rgb,
    pub(crate) spill: f64,
}

impl Cyclorama {
    /// The relative radiance along unit `d`, `key` the key light's direction from the centre.
    pub(crate) fn at(&self, d: Rgb, key: Rgb) -> Rgb {
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

/// A studio: a starter rig's lights in the room, over a cyclorama.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rig {
    /// The upstream preset it binds (`STU-*`).
    pub(crate) preset: &'static str,
    /// The exported rig's lights, the key first, then the studio's own additions.
    pub(crate) lights: &'static [RigLight],
    /// How many of `lights` are the export's; the rest are the studio's own.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) exported: usize,
    pub(crate) cyclorama: Cyclorama,
}

const fn grey(v: f64) -> Rgb {
    [v, v, v]
}

const fn tint(c: Rgb, s: f64) -> Rgb {
    [c[0] * s, c[1] * s, c[2] * s]
}

/// The cyclorama's slight warmth toward the floor and coolness toward the ceiling.
const WARM: Rgb = [1.0, 0.985, 0.96];
const COOL: Rgb = [0.96, 0.98, 1.0];

/// `STU-002`, "Three-point product studio": a graded key above front-left, a fill card
/// front-right (EV −2 of the key) and a strip rim behind (EV −0.5), in a mid-bright room.
pub(crate) const PRODUCT: Rig = Rig {
    preset: "STU-002",
    lights: &[
        RigLight {
            id: "key",
            centre: [-1.2, -1.2, 1.0],
            width_axis: [-FRAC_1_SQRT_2, FRAC_1_SQRT_2, 0.0],
            height_axis: [0.358979079309, 0.358979079309, 0.861549790341],
            half_width: 0.4,
            half_height: 0.5,
            radiance: grey(357.289685146),
            grade: 0.5,
        },
        RigLight {
            id: "fill",
            centre: [1.2, -1.0, 0.6],
            width_axis: [-0.640184399664, -0.768221279597, -2.77555756156e-17],
            height_axis: [-0.275460015502, 0.229550012919, 0.933503385869],
            half_width: 0.6,
            half_height: 0.5,
            radiance: grey(45.1401839419),
            grade: 0.35,
        },
        RigLight {
            id: "rim",
            centre: [0.7, 1.1, 1.0],
            width_axis: [0.843661487732, -0.536875492193, 2.77555756156e-17],
            height_axis: [-0.326732019607, -0.51343603081, 0.793492047616],
            half_width: 0.125,
            half_height: 0.6,
            radiance: grey(475.065381914),
            grade: 0.3,
        },
    ],
    exported: 3,
    cyclorama: Cyclorama { floor: tint(WARM, 0.9), wall: grey(1.05), band: grey(0.8), ceiling: tint(COOL, 0.6), spill: 0.4 },
};

/// `STU-004`, "Black housing with broad side reflections": two broad side panels (the right at
/// EV −0.5) over a bright cyclorama. Low contrast, soft highlights.
pub(crate) const SOFT: Rig = Rig {
    preset: "STU-004",
    lights: &[
        RigLight {
            id: "key",
            centre: [-1.5, -0.4, 0.7],
            width_axis: [-0.25766265056, 0.966234939601, -1.38777878078e-17],
            height_axis: [0.397174885335, 0.105913302756, 0.911610927292],
            half_width: 0.75,
            half_height: 0.75,
            radiance: grey(106.904574216),
            grade: 0.5,
        },
        RigLight {
            id: "fill",
            centre: [1.5, -0.4, 0.7],
            width_axis: [-0.25766265056, -0.966234939601, 1.38777878078e-17],
            height_axis: [-0.397174885335, 0.105913302756, 0.911610927292],
            half_width: 0.75,
            half_height: 0.75,
            radiance: grey(75.5929493679),
            grade: 0.5,
        },
    ],
    exported: 2,
    cyclorama: Cyclorama { floor: tint(WARM, 1.35), wall: grey(1.9), band: grey(0.6), ceiling: tint(COOL, 1.4), spill: 0.15 },
};

/// `STU-005`, "Mirror-metal vertical strips": two tall strips either side (the right at EV −1)
/// and a low front fill (EV −3) in a darker room. Crisp highlights on metal. The Realistic default.
///
/// The studio's own addition is a broad, soft overhead scrim ([`TOP_SCRIM`]): the strips sit
/// about 10° high, so without it a flat metal face seen from above, or from the default
/// three-quarter view, mirrors only the dim ceiling or wall and reads dull (gold reads olive).
/// The scrim fills the room's top, and its long graded edge falls off between about 60° and
/// 34° of elevation, so top faces seen from three-quarter views mirror a gradient, while the
/// walls below stay dark and the room moody.
pub(crate) const CONTRAST: Rig = Rig {
    preset: "STU-005",
    lights: &[
        RigLight {
            id: "left_strip",
            centre: [-1.1, -0.3, 0.2],
            width_axis: [-0.263117405792, 0.964763821238, 0.0],
            height_axis: [0.166685800662, 0.0454597638169, 0.984961549367],
            half_width: 0.06,
            half_height: 0.8,
            radiance: grey(592.738980244),
            grade: 0.3,
        },
        RigLight {
            id: "right_strip",
            centre: [1.1, -0.3, 0.2],
            width_axis: [-0.263117405792, -0.964763821238, 0.0],
            height_axis: [-0.166685800662, 0.0454597638169, 0.984961549367],
            half_width: 0.06,
            half_height: 0.8,
            radiance: grey(296.369490122),
            grade: 0.3,
        },
        RigLight {
            id: "front_fill",
            centre: [0.0, -1.8, 0.8],
            width_axis: [-1.0, 0.0, 0.0],
            height_axis: [0.0, 0.406138466053, 0.91381154862],
            half_width: 0.55,
            half_height: 0.4,
            radiance: grey(40.6801806776),
            grade: 0.4,
        },
        TOP_SCRIM,
    ],
    exported: 3,
    cyclorama: Cyclorama { floor: tint(WARM, 0.9), wall: grey(0.9), band: grey(0.75), ceiling: tint(COOL, 0.6), spill: 0.2 },
};

/// A soft overhead scrim, 3.9 B square (nearly the room's width), 1.3 B above the studio's
/// centre, facing down, graded over 70 % of its half size. Its radiance is set so that a neutral
/// polished metal seen from straight above reads light (about 0.63 scene-referred, against 0.24
/// under the ceiling alone) without reaching the tone curve's shoulder; along the default view's
/// mirror direction (about 35° high) it gives about 0.3, falling off downward.
const TOP_SCRIM: RigLight = RigLight {
    id: "top_scrim",
    centre: [0.0, 0.0, 1.3],
    width_axis: [1.0, 0.0, 0.0],
    height_axis: [0.0, -1.0, 0.0],
    half_width: 1.95,
    half_height: 1.95,
    radiance: grey(14.0),
    grade: 0.7,
};

impl Rig {
    /// The radiance of a white diffuse card at the centre turned to the key, lit by the key alone:
    /// the unit of the cyclorama's radiances.
    pub(crate) fn key_white(&self) -> f64 {
        let Some(key) = self.lights.first() else { return 1.0 };
        let [r, g, b] = key.radiance.map(|c| c * polygon_irradiance([0.0; 3], unit(key.centre), &key.corners()) / PI);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The cyclorama's radiance along unit `d` from the studio's centre (what diffuse shading
    /// takes from the room apart from the direct lights).
    #[cfg(test)]
    pub(crate) fn ambient(&self, d: Rgb) -> Rgb {
        self.ambient_in(d, self.key_white())
    }

    /// [`Rig::ambient`] given [`Rig::key_white`] (worked out once per bake).
    pub(crate) fn ambient_in(&self, d: Rgb, key_white: f64) -> Rgb {
        let key = self.lights.first().map_or([0.0, 0.0, 1.0], |k| unit(k.centre));
        self.cyclorama.at(d, key).map(|c| c * key_white)
    }

    /// Everything seen from the studio's centre along unit `d`: the cyclorama on the room's
    /// walls, plus every light the ray meets inside the room (a diffuser glows over what is
    /// behind it, so a graded edge fades into the wall, never to black).
    #[cfg(test)]
    pub(crate) fn radiance(&self, d: Rgb) -> Rgb {
        self.radiance_in(d, self.key_white())
    }

    /// [`Rig::radiance`] given [`Rig::key_white`].
    pub(crate) fn radiance_in(&self, d: Rgb, key_white: f64) -> Rgb {
        let wall = ROOM.wall(d);
        let far = dot(wall, wall).sqrt();
        let mut c = self.ambient_in(d, key_white);
        for (_, light) in self.lights.iter().filter_map(|light| light.seen_from_centre(d)).filter(|(t, _)| *t <= far + 1e-9) {
            add(&mut c, light, 1.0);
        }
        c
    }

    /// The irradiance the rig's lights give a surface at `p` (studio coordinates) with unit
    /// normal `n`: each light's exact polygon irradiance at its mean radiance, clipped to the
    /// surface's horizon, from its emitting side only.
    pub(crate) fn direct_irradiance(&self, p: Rgb, n: Rgb) -> Rgb {
        let mut e = [0.0; 3];
        for light in self.lights {
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
    pub(crate) fn ambient_irradiance(&self, n: Rgb) -> Rgb {
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

    /// The per-preset exposure calibration: the scale that maps a white Lambertian face at the
    /// studio's centre, turned to the key (the first light), to [`WHITE_UNDER_KEY`] before tone
    /// mapping. The studio's radiance is baked at this scale, so a look's exposure 1 is the
    /// calibrated one and stays fixed while the view orbits.
    pub(crate) fn calibration(&self) -> f64 {
        let Some(key) = self.lights.first() else { return 1.0 };
        let n = unit(key.centre);
        let [r, g, b] = {
            let mut e = self.direct_irradiance([0.0; 3], n);
            add(&mut e, self.ambient_irradiance(n), 1.0);
            e
        };
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if luminance > 0.0 { WHITE_UNDER_KEY * PI / luminance } else { 1.0 }
    }
}

/// The scene-referred value a white diffuse face under the key maps to: PBR Neutral leaves it
/// below its shoulder (0.76) and takes 0.04 off, giving 0.6038 linear, 0.80 display (sRGB).
pub(crate) const WHITE_UNDER_KEY: f64 = 0.6438;

/// The irradiance (over unit radiance) a uniform polygon with corners `corners` gives a surface at
/// `p` with unit normal `n`: the polygon clipped to the surface's upper half-space, then
/// ½ Σ θᵢ (n · γᵢ) over its edges (Lambert's formula).
pub(crate) fn polygon_irradiance(p: Rgb, n: Rgb, corners: &[Rgb; 4]) -> f64 {
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
