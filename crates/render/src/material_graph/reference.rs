//! The test-only reference: an f64 interpreter of the material-graph [`Ir`] (decision O12,
//! docs/material-graph.md §5.7, §10 track T). It is an **oracle**, never a renderer: product code
//! never evaluates a graph on the CPU. The module exists only under `cfg(test)` or the
//! `test-reference` feature, which only dev-dependencies enable; `cargo xtask layers` refuses a
//! normal or build dependency that enables it.
//!
//! Both GPU back ends (the viewport's raster back end, track C1, and the path tracer's, track PT)
//! read back their evaluation at fixed points and compare it with [`evaluate`] at the same
//! [`Point`]s. The reference reads the same uniform words the GPU reads ([`uniform_words`]), so
//! constants arrive with the same `f32` rounding.
//!
//! # Using it from a back end's tests
//!
//! ```ignore
//! // crates/gpu/Cargo.toml: [dev-dependencies] solidcraft-render = { workspace = true, features = ["test-reference"] }
//! use solidcraft_render::material_graph::reference::{self, Point, ReferenceSurface};
//! let (ir, words) = reference::prepare_json(fixture_text, &Default::default())?;
//! for point in reference::sample_points() {
//!     let want = reference::evaluate(&ir, &words, &[], &point)?.surface;
//!     let got = ReferenceSurface::from_floats(&gpu_readback_for(&point))?; // WGSL `Surface` order
//!     let (field, diff) = want.max_difference(&got);
//!     assert!(diff < 1e-4, "{field}: {diff}");
//! }
//! ```
//!
//! [`Evaluation::values`] holds every instruction's value, indexed by [`ValueId`], so a back end
//! can compare intermediate pattern values too (the path tracer's pattern values equal the
//! viewport's at the same points, §5.7).
//!
//! # What is compared, and where it can differ from a GPU
//!
//! - GPU arithmetic is `f32`; the reference is `f64`. Lattice noise and `floor`/`fract`/`step`
//!   jump at cell boundaries, so compare at points well inside cells ([`sample_points`] are).
//! - The viewport filters (mips, `fwidth` footprints); the path tracer does not. The reference's
//!   footprint is [`Point::footprint_mm`] (0: unfiltered, the path tracer's case) and it samples
//!   images at mip level 0, so compare filtered output at a small footprint.
//!
//! # The node formulas (the contract the back ends mirror)
//!
//! The noise and projection functions mirror the shared library (`crates/gpu/src/shading/`);
//! **re-check** them when track S's `noise.wgsl` and track C1's `patterns.wgsl` land.
//!
//! - **Gradients.** Every float carries its world-space gradient per mm. Positions carry their
//!   Jacobian (identity in world space, rows of the inverse object-to-world map in object space);
//!   normals, facing, constants and the tangent carry none. Maths nodes apply the chain rule.
//!   This is the "slope" of §3: `normal.from_height` reads the height's gradient.
//! - **Texcoords** hold one plane (planar, cylindrical) or three weighted planes (triplanar). A
//!   plane has uv, the world gradients of u and v, the footprint `fp = footprint_mm · max(|∇u|,
//!   |∇v|)` in uv units, and its tangent: ∇u made tangential to the world normal, normalised (the
//!   geometric tangent where that vanishes). Patterns and images reading a triplanar texcoord
//!   are evaluated per plane and blended by the weights (value, gradient, slope; tangents
//!   blended then normalised).
//! - `texcoord.planar`: uv = ((q − origin)·u/|u|², (q − origin)·v/|v|²), q the position in the
//!   node's space. `texcoord.triplanar`: r = (q − origin)/scale_mm; planes (r.y, r.z), (r.x, r.z),
//!   (r.x, r.y) with `sc_triplanar_weights` of the space's normal. `texcoord.cylindrical`:
//!   `sc_cylindrical_uv`. `texcoord.transform`: uv' = R(rotation)·(uv ⊙ scale) + offset.
//! - **Slopes of patterns** are forward differences with a step of 0.05 of the pattern's period
//!   along each axis (`pat_bumps`): slope = (f(u + h_u, v) − f(u, v))/h_u, likewise for v; the
//!   world gradient is slope.x·∇u + slope.y·∇v. A 3D noise differences along its position's axes.
//!   A 3D noise's `slope` output is zero.
//! - **Fades**: `pat_fade(fp, period) = 1 − smoothstep(0.2, 0.6, fp / max(period, 1e-6))` and
//!   the weave's `pat_weave_fade` (0.1, 0.3). A pattern fades toward 0.5 (`grid`: toward its
//!   covered fraction) by multiplying its deviation.
//! - `pattern.noise`: `sc_fbm(kind, 2, (uv/scale_mm, 0), seed, octaves, lacunarity, gain)`, or 3D at
//!   position/scale_mm, faded with period scale_mm. `pattern.brushed`, `pattern.weave`,
//!   `pattern.checker`, `pattern.grid`, `pattern.gradient`: see `reference/patterns.rs`, one
//!   function each, documented with its formula.
//! - `image`: bilinear at mip 0, v = 1 at the top row; sRGB decoded per texel before filtering;
//!   wrap `repeat`/`clamp`/`mirror` as the sampler address modes. `value` is the Rec.709
//!   luminance (`rgb`) or the channel. An unbound texture gives `default` with alpha 0. Slopes
//!   difference by one texel.
//! - **Maths**: component-wise with a float broadcast. `divide`, `remap` and `smoothstep` hold
//!   their denominator ≥ 1e-6 from zero, keeping its sign (zero counts as positive). `power` is
//!   `max(a, 0)^b`, `sqrt` is `sqrt(max(x, 0))`, `clamp` is `min(max(x, low), high)`,
//!   `step(edge, x)` is `x ≥ edge`, `normalize` is `sc_unit` (zero → +Z; a vector2 or float is
//!   padded with zeros, so zero stays zero). `hsv_adjust` (hex-cone HSV; saturation clamped to
//!   [0, 1]) carries no gradient.
//! - **Surface**: each input is clamped to its catalogue range, with NaN components mapped to the
//!   default; normals go through `sc_unit` (NaN: the geometric normal); the tangent is made
//!   tangential to the base normal and normalised (the geometric tangent if that vanishes), and
//!   is zero in the output while the anisotropy is 0. `base_color` is multiplied by
//!   `base_weight`. `surface.mix` blends each sanitised layer's fields linearly, normals and the
//!   tangent by normalised lerp; the factor is clamped to [0, 1] (NaN: 0.5).

mod dual;
mod noise;
mod patterns;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;

use super::catalogue::{InputDefault, PortType, node_spec};
use super::graph::{Constant, MaterialGraph};
use super::ir::{GeometryInput, Instr, Ir, Operand, ValueId, lower, uniform_words, validate};
pub use dual::D as Dual;
use dual::{D, ZERO3, cross, dot3, length3, mix3, tangential, unit};
pub use patterns::{LUMINANCE, SLOPE_STEP};

/// Why the reference could not evaluate a graph (a test-setup error, never a shading result).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceError(pub String);

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReferenceError {}

fn err<T>(message: impl Into<String>) -> Result<T, ReferenceError> {
    Err(ReferenceError(message.into()))
}

/// One shading point: what the geometry supplies to a material. Lengths in mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub object_position: [f64; 3],
    pub world_position: [f64; 3],
    /// Unit geometric normals.
    pub object_normal: [f64; 3],
    pub world_normal: [f64; 3],
    /// The geometric tangent (unit, perpendicular to `world_normal`): an unconnected
    /// `geometry_tangent`, and the fallback where a projection has none.
    pub tangent: [f64; 3],
    /// |n·v|.
    pub facing: f64,
    /// The pixel footprint in world mm; 0 is unfiltered (the path tracer).
    pub footprint_mm: f64,
    /// The linear part of the object-to-world map, as rows: world = M·object + t.
    pub object_to_world: [[f64; 3]; 3],
}

const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

impl Point {
    /// A point whose object and world spaces coincide, facing the viewer, unfiltered, with a
    /// tangent from the world X axis (or Y, on a face normal to X).
    pub fn new(position: [f64; 3], normal: [f64; 3]) -> Point {
        let n = unit(normal);
        let x = if n[0].abs() > 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        Point {
            object_position: position,
            world_position: position,
            object_normal: n,
            world_normal: n,
            tangent: unit(tangential(x, n)),
            facing: 1.0,
            footprint_mm: 0.0,
            object_to_world: IDENTITY,
        }
    }

    pub fn with_footprint(self, footprint_mm: f64) -> Point {
        Point { footprint_mm, ..self }
    }

    /// The same object point placed by `m` (rows) and `t`: world position, world normal
    /// (inverse transpose) and tangent follow.
    pub fn placed(self, m: [[f64; 3]; 3], t: [f64; 3]) -> Result<Point, ReferenceError> {
        let inv = inverse(m)?;
        let apply = |m: &[[f64; 3]; 3], v: [f64; 3]| [dot3(m[0], v), dot3(m[1], v), dot3(m[2], v)];
        let wp = dual::add3(apply(&m, self.object_position), t);
        let inv_t = transpose(inv);
        let n = unit(apply(&inv_t, self.object_normal));
        let tan = unit(tangential(apply(&m, self.tangent), n));
        Ok(Point { world_position: wp, world_normal: n, tangent: tan, object_to_world: m, ..self })
    }
}

fn transpose(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]]
}

fn inverse(m: [[f64; 3]; 3]) -> Result<[[f64; 3]; 3], ReferenceError> {
    let c0 = cross(m[1], m[2]);
    let det = dot3(m[0], c0);
    if det.abs() < 1e-12 || !det.is_finite() {
        return err("the object-to-world map is singular");
    }
    let c1 = cross(m[2], m[0]);
    let c2 = cross(m[0], m[1]);
    // The inverse's columns are the cofactor rows over det.
    Ok(transpose([c0, c1, c2].map(|r| r.map(|x| x / det))))
}

/// Fixed sample points both back ends use: inside noise cells and away from pattern edges, on
/// faces of several orientations, unfiltered.
pub fn sample_points() -> Vec<Point> {
    let s = 1.0 / 3f64.sqrt();
    vec![
        Point::new([0.317, 0.611, 0.0], [0.0, 0.0, 1.0]),
        Point::new([1.237, -2.419, 0.5], [0.0, 0.0, 1.0]),
        Point::new([-3.731, 1.113, 0.0], [0.0, 0.0, -1.0]),
        Point::new([2.0, 0.433, 0.271], [1.0, 0.0, 0.0]),
        Point::new([0.389, -2.0, 1.177], [0.0, -1.0, 0.0]),
        Point::new([0.613, 0.709, 0.523], [s, s, s]),
        Point::new([-1.071, 3.329, -0.417], [0.6, 0.0, 0.8]),
        Point::new([4.153, -0.271, 2.909], [0.0, 0.8, -0.6]),
    ]
}

/// A float, vector, colour or normal: `width` components, each with its world gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Num {
    pub width: u8,
    pub c: [D; 3],
}

impl Num {
    pub fn scalar(v: D) -> Num {
        Num { width: 1, c: [v, D::c(0.0), D::c(0.0)] }
    }
    pub fn vec(c: &[D]) -> Num {
        let mut out = [D::c(0.0); 3];
        for (o, v) in out.iter_mut().zip(c) {
            *o = *v;
        }
        Num { width: u8::try_from(c.len().min(3)).unwrap_or(3), c: out }
    }
    pub fn constant(v: &[f64]) -> Num {
        Num::vec(&v.iter().map(|x| D::c(*x)).collect::<Vec<_>>())
    }
    /// Component `k`, a float broadcast to every component; zero past a vector's width.
    pub fn at(&self, k: usize) -> D {
        if self.width == 1 {
            self.c[0]
        } else if k < usize::from(self.width) {
            self.c.get(k).copied().unwrap_or_default()
        } else {
            D::c(0.0)
        }
    }
    /// The component values.
    pub fn values(&self) -> Vec<f64> {
        self.c.iter().take(usize::from(self.width)).map(|d| d.v).collect()
    }
    fn v3(&self) -> [f64; 3] {
        [self.at(0).v, self.at(1).v, self.at(2).v]
    }
}

/// One plane of a texcoord.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub uv: [f64; 2],
    /// World gradients of u and v, per mm.
    pub gu: [f64; 3],
    pub gv: [f64; 3],
    /// The footprint in uv units.
    pub fp: f64,
    /// Unit world direction of increasing u along the surface.
    pub tangent: [f64; 3],
}

/// A projected coordinate: one plane, or three weighted planes for triplanar.
#[derive(Clone, Debug, PartialEq)]
pub struct Texcoord {
    pub planes: Vec<(Plane, f64)>,
}

impl Texcoord {
    /// The blended tangent, normalised.
    pub fn tangent(&self) -> [f64; 3] {
        unit(self.planes.iter().fold(ZERO3, |acc, (p, w)| dual::add3(acc, dual::scale(p.tangent, *w))))
    }
}

/// The value of one IR instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(Num),
    Texcoord(Texcoord),
}

impl Value {
    pub fn num(&self) -> Option<&Num> {
        match self {
            Value::Num(n) => Some(n),
            Value::Texcoord(_) => None,
        }
    }
    /// A float's value.
    pub fn scalar(&self) -> Option<f64> {
        self.num().filter(|n| n.width == 1).map(|n| n.c[0].v)
    }
}

/// A texture as the GPU holds it: RGBA texels in [0, 1], row 0 at the top, as stored (sRGB
/// encoded or linear; the `image` node's space says which).
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceTexture {
    pub width: u32,
    pub height: u32,
    pub texels: Vec<[f64; 4]>,
}

impl ReferenceTexture {
    /// From 8-bit RGBA rows, top row first.
    pub fn from_rgba8(width: u32, height: u32, bytes: &[u8]) -> Result<ReferenceTexture, ReferenceError> {
        let n = usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| ReferenceError("texture too large".into()))?;
        if bytes.len() != n.saturating_mul(4) {
            return err(format!("{width}×{height} RGBA needs {} bytes, got {}", n.saturating_mul(4), bytes.len()));
        }
        let texels = bytes.as_chunks::<4>().0.iter().map(|c| c.map(|b| f64::from(b) / 255.0)).collect();
        Ok(ReferenceTexture { width, height, texels })
    }
}

/// The WGSL `Surface` the back ends' `material_eval` returns (crates/gpu/src/pbr.rs), field for
/// field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceSurface {
    pub base_color: [f64; 3],
    pub metalness: f64,
    pub roughness: f64,
    pub ior: f64,
    pub coat_weight: f64,
    pub coat_roughness: f64,
    pub coat_ior: f64,
    pub normal: [f64; 3],
    pub coat_normal: [f64; 3],
    pub specular_weight: f64,
    pub specular_color: [f64; 3],
    pub anisotropy: f64,
    pub rotation: f64,
    /// Unit; zero while isotropic.
    pub tangent: [f64; 3],
    pub coat_color: [f64; 3],
    pub emission_luminance: f64,
    pub emission_color: [f64; 3],
    pub transmission_weight: f64,
    pub transmission_color: [f64; 3],
    pub transmission_depth_mm: f64,
    pub opacity: f64,
}

impl ReferenceSurface {
    /// Floats in the WGSL declaration order.
    pub const FLOATS: usize = 37;
    /// The name of each float, in [`Self::to_floats`] order.
    pub const NAMES: [&'static str; Self::FLOATS] = [
        "base_color.r",
        "base_color.g",
        "base_color.b",
        "metalness",
        "roughness",
        "ior",
        "coat_weight",
        "coat_roughness",
        "coat_ior",
        "normal.x",
        "normal.y",
        "normal.z",
        "coat_normal.x",
        "coat_normal.y",
        "coat_normal.z",
        "specular_weight",
        "specular_color.r",
        "specular_color.g",
        "specular_color.b",
        "anisotropy",
        "rotation",
        "tangent.x",
        "tangent.y",
        "tangent.z",
        "coat_color.r",
        "coat_color.g",
        "coat_color.b",
        "emission_luminance",
        "emission_color.r",
        "emission_color.g",
        "emission_color.b",
        "transmission_weight",
        "transmission_color.r",
        "transmission_color.g",
        "transmission_color.b",
        "transmission_depth_mm",
        "opacity",
    ];

    pub fn to_floats(&self) -> [f64; Self::FLOATS] {
        let s = self;
        let mut out = [0.0; Self::FLOATS];
        let parts: [&[f64]; 21] = [
            &s.base_color,
            &[s.metalness],
            &[s.roughness],
            &[s.ior],
            &[s.coat_weight],
            &[s.coat_roughness],
            &[s.coat_ior],
            &s.normal,
            &s.coat_normal,
            &[s.specular_weight],
            &s.specular_color,
            &[s.anisotropy],
            &[s.rotation],
            &s.tangent,
            &s.coat_color,
            &[s.emission_luminance],
            &s.emission_color,
            &[s.transmission_weight],
            &s.transmission_color,
            &[s.transmission_depth_mm],
            &[s.opacity],
        ];
        for (o, v) in out.iter_mut().zip(parts.iter().flat_map(|p| p.iter())) {
            *o = *v;
        }
        out
    }

    /// From a GPU readback of `Surface`, its fields packed in declaration order (no padding).
    pub fn from_floats(f: &[f32]) -> Result<ReferenceSurface, ReferenceError> {
        if f.len() != Self::FLOATS {
            return err(format!("a Surface is {} floats, got {}", Self::FLOATS, f.len()));
        }
        let x = |k: usize| f.get(k).copied().map_or(f64::NAN, f64::from);
        let v = |k: usize| [x(k), x(k + 1), x(k + 2)];
        Ok(ReferenceSurface {
            base_color: v(0),
            metalness: x(3),
            roughness: x(4),
            ior: x(5),
            coat_weight: x(6),
            coat_roughness: x(7),
            coat_ior: x(8),
            normal: v(9),
            coat_normal: v(12),
            specular_weight: x(15),
            specular_color: v(16),
            anisotropy: x(19),
            rotation: x(20),
            tangent: v(21),
            coat_color: v(24),
            emission_luminance: x(27),
            emission_color: v(28),
            transmission_weight: x(31),
            transmission_color: v(32),
            transmission_depth_mm: x(35),
            opacity: x(36),
        })
    }

    /// The field that differs most from `other`, and by how much (NaN counts as infinite).
    pub fn max_difference(&self, other: &ReferenceSurface) -> (&'static str, f64) {
        let (a, b) = (self.to_floats(), other.to_floats());
        let mut worst = ("", 0.0);
        for (k, name) in Self::NAMES.iter().enumerate() {
            let d = (a[k] - b[k]).abs();
            let d = if d.is_nan() { f64::INFINITY } else { d };
            if d > worst.1 {
                worst = (name, d);
            }
        }
        worst
    }
}

/// The reference's result at one point.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    /// Every instruction's value, indexed by its [`ValueId`].
    pub values: Vec<Value>,
    pub surface: ReferenceSurface,
}

impl Evaluation {
    pub fn value(&self, id: ValueId) -> Option<&Value> {
        self.values.get(usize::try_from(id.0).ok()?)
    }
}

/// Validate and lower a graph and fill its uniform words from `values` (a look's interface
/// values), exactly as the product does.
pub fn prepare(graph: &MaterialGraph, values: &BTreeMap<String, Constant>) -> Result<(Ir, Vec<u32>), ReferenceError> {
    let checked = validate(graph).map_err(|e| ReferenceError(e.to_string()))?;
    let ir = lower(&checked).map_err(|e| ReferenceError(e.to_string()))?;
    let words = uniform_words(&ir, values).map_err(|e| ReferenceError(e.to_string()))?;
    Ok((ir, words))
}

/// [`prepare`] from a graph's JSON.
pub fn prepare_json(text: &str, values: &BTreeMap<String, Constant>) -> Result<(Ir, Vec<u32>), ReferenceError> {
    let graph: MaterialGraph = serde_json::from_str(text).map_err(|e| ReferenceError(e.to_string()))?;
    prepare(&graph, values)
}

/// An operand, resolved.
#[derive(Clone, Debug)]
enum Arg {
    Num(Num),
    Tc(Texcoord),
    Seed(u32),
    Texture(u8),
    Absent,
}

struct Machine<'a> {
    ir: &'a Ir,
    words: &'a [u32],
    textures: &'a [Option<&'a ReferenceTexture>],
    p: &'a Point,
    /// Rows of the inverse object-to-world map: the world gradients of object coordinates.
    object_rows: [[f64; 3]; 3],
    /// Every instruction's outputs, in catalogue order (a selection: its one value).
    outputs: Vec<Vec<Value>>,
}

/// Evaluate `ir` at `point` with one material's `words` ([`uniform_words`]) and textures by
/// their index in [`Ir::textures`] (`None` or missing: unbound, the image's `default`).
pub fn evaluate(ir: &Ir, words: &[u32], textures: &[Option<&ReferenceTexture>], point: &Point) -> Result<Evaluation, ReferenceError> {
    let inv = inverse(point.object_to_world)?;
    let mut m = Machine { ir, words, textures, p: point, object_rows: inv, outputs: Vec::with_capacity(ir.instrs.len()) };
    for (i, instr) in ir.instrs.iter().enumerate() {
        if usize::try_from(instr.result.0).ok() != Some(i) {
            return err(format!("instruction {i} has result {}", instr.result.0));
        }
        let out = m.instr(instr)?;
        m.outputs.push(out);
    }
    let mut surface = m.surface(&ir.surface)?;
    for layer in &ir.mix_layers {
        let fg = m.surface(&layer.fg)?;
        let t = match m.arg(&layer.mix)? {
            Arg::Num(n) => n.c[0].v,
            _ => 0.5,
        };
        let t = if t.is_nan() { 0.5 } else { t.clamp(0.0, 1.0) };
        surface = blend(&surface, &fg, t);
    }
    if surface.anisotropy == 0.0 {
        surface.tangent = ZERO3;
    }
    let values = m.outputs.into_iter().map(|o| o.into_iter().next().unwrap_or(Value::Num(Num::constant(&[0.0])))).collect();
    Ok(Evaluation { values, surface })
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn blend(bg: &ReferenceSurface, fg: &ReferenceSurface, t: f64) -> ReferenceSurface {
    let l = |a: f64, b: f64| lerp(a, b, t);
    let c = |a: [f64; 3], b: [f64; 3]| mix3(a, b, t);
    let n = |a: [f64; 3], b: [f64; 3]| unit(mix3(a, b, t));
    ReferenceSurface {
        base_color: c(bg.base_color, fg.base_color),
        metalness: l(bg.metalness, fg.metalness),
        roughness: l(bg.roughness, fg.roughness),
        ior: l(bg.ior, fg.ior),
        coat_weight: l(bg.coat_weight, fg.coat_weight),
        coat_roughness: l(bg.coat_roughness, fg.coat_roughness),
        coat_ior: l(bg.coat_ior, fg.coat_ior),
        normal: n(bg.normal, fg.normal),
        coat_normal: n(bg.coat_normal, fg.coat_normal),
        specular_weight: l(bg.specular_weight, fg.specular_weight),
        specular_color: c(bg.specular_color, fg.specular_color),
        anisotropy: l(bg.anisotropy, fg.anisotropy),
        rotation: l(bg.rotation, fg.rotation),
        tangent: n(bg.tangent, fg.tangent),
        coat_color: c(bg.coat_color, fg.coat_color),
        emission_luminance: l(bg.emission_luminance, fg.emission_luminance),
        emission_color: c(bg.emission_color, fg.emission_color),
        transmission_weight: l(bg.transmission_weight, fg.transmission_weight),
        transmission_color: c(bg.transmission_color, fg.transmission_color),
        transmission_depth_mm: l(bg.transmission_depth_mm, fg.transmission_depth_mm),
        opacity: l(bg.opacity, fg.opacity),
    }
}

fn width(ty: PortType) -> usize {
    match ty {
        PortType::Vector2 => 2,
        PortType::Vector3 | PortType::Color3 | PortType::Normal => 3,
        _ => 1,
    }
}

impl Machine<'_> {
    fn arg(&self, op: &Operand) -> Result<Arg, ReferenceError> {
        Ok(match *op {
            Operand::Value(ValueId(v)) => {
                let i = usize::try_from(v).unwrap_or(usize::MAX);
                match self.outputs.get(i).and_then(|o| o.first()) {
                    Some(Value::Num(n)) => Arg::Num(*n),
                    Some(Value::Texcoord(t)) => Arg::Tc(t.clone()),
                    None => return err(format!("value {v} is read before it is computed")),
                }
            }
            Operand::Uniform { offset, width } => {
                let slot = self.ir.slots.iter().find(|s| s.offset == offset);
                let at = |k: u32| usize::try_from(offset + k).ok().and_then(|i| self.words.get(i)).copied();
                if slot.is_some_and(|s| s.ty == PortType::Int) {
                    return at(0).map(Arg::Seed).ok_or_else(|| ReferenceError(format!("no uniform word {offset}")));
                }
                let mut c = Vec::new();
                for k in 0..u32::from(width) {
                    let w = at(k).ok_or_else(|| ReferenceError(format!("no uniform word {}", offset + k)))?;
                    c.push(f64::from(f32::from_bits(w)));
                }
                Arg::Num(Num::constant(&c))
            }
            Operand::Texture(t) => Arg::Texture(t),
            Operand::Geometry(g) => Arg::Num(self.geometry(g)),
            Operand::Absent => Arg::Absent,
        })
    }

    fn geometry(&self, g: GeometryInput) -> Num {
        let p = self.p;
        let rows = |v: [f64; 3], r: [[f64; 3]; 3]| Num::vec(&[D::new(v[0], r[0]), D::new(v[1], r[1]), D::new(v[2], r[2])]);
        match g {
            GeometryInput::ObjectPosition => rows(p.object_position, self.object_rows),
            GeometryInput::WorldPosition => rows(p.world_position, IDENTITY),
            GeometryInput::ObjectNormal => Num::constant(&p.object_normal),
            GeometryInput::WorldNormal => Num::constant(&p.world_normal),
            GeometryInput::Facing => Num::constant(&[p.facing]),
            GeometryInput::Tangent => Num::constant(&p.tangent),
        }
    }

    /// The position and normal of a projection's `space` (0 object, 1 world).
    fn space(&self, space: i64) -> ([D; 3], [f64; 3]) {
        let g = if space == 0 { GeometryInput::ObjectPosition } else { GeometryInput::WorldPosition };
        let n = if space == 0 { self.p.object_normal } else { self.p.world_normal };
        (self.geometry(g).c, n)
    }

    fn instr(&self, instr: &Instr) -> Result<Vec<Value>, ReferenceError> {
        if let Some(name) = instr.selects() {
            let Some(Operand::Value(ValueId(call))) = instr.args.first() else {
                return err(format!("{} selects nothing", instr.op));
            };
            let spec = node_spec(instr.kind()).ok_or_else(|| ReferenceError(format!("unknown kind {}", instr.kind())))?;
            let k = spec.outputs.iter().position(|o| o.name == name).unwrap_or(usize::MAX);
            let value = usize::try_from(*call).ok().and_then(|c| self.outputs.get(c)).and_then(|o| o.get(k)).cloned();
            return value.map(|v| vec![v]).ok_or_else(|| ReferenceError(format!("{}: no output {name}", instr.op)));
        }
        let args = instr.args.iter().map(|a| self.arg(a)).collect::<Result<Vec<_>, _>>()?;
        let param = |k: usize| instr.params.get(k).copied().unwrap_or(0);
        let w = width(instr.ty);
        let num = |k: usize| -> Result<Num, ReferenceError> {
            match args.get(k) {
                Some(Arg::Num(n)) => Ok(*n),
                _ => err(format!("{}: argument {k} is not a number", instr.op)),
            }
        };
        let d = |k: usize| num(k).map(|n| n.c[0]);
        let f = |k: usize| d(k).map(|x| x.v);
        let v2 = |k: usize| num(k).map(|n| [n.at(0).v, n.at(1).v]);
        let v3 = |k: usize| num(k).map(|n| n.v3());
        let seed = |k: usize| match args.get(k) {
            Some(Arg::Seed(s)) => *s,
            _ => 0,
        };
        let tc = |k: usize| -> Result<&Texcoord, ReferenceError> {
            match args.get(k) {
                Some(Arg::Tc(t)) => Ok(t),
                _ => err(format!("{}: argument {k} is not a texcoord", instr.op)),
            }
        };
        let map = |op: &dyn Fn(&[D]) -> D| -> Result<Vec<Value>, ReferenceError> {
            let nums = (0..args.len()).map(num).collect::<Result<Vec<_>, _>>()?;
            let c: Vec<D> = (0..w).map(|k| op(&nums.iter().map(|n| n.at(k)).collect::<Vec<_>>())).collect();
            Ok(vec![Value::Num(Num::vec(&c))])
        };
        let a = |x: &[D], k: usize| x.get(k).copied().unwrap_or_default();
        let p = self.p;
        let one = |n: Num| Ok(vec![Value::Num(n)]);
        match instr.op {
            "constant" => map(&|x| a(x, 0)),
            "add" => map(&|x| a(x, 0) + a(x, 1)),
            "subtract" => map(&|x| a(x, 0) - a(x, 1)),
            "multiply" => map(&|x| a(x, 0) * a(x, 1)),
            "divide" => map(&|x| a(x, 0) / dual::guard(a(x, 1))),
            "min" => map(&|x| a(x, 0).min(a(x, 1))),
            "max" => map(&|x| a(x, 0).max(a(x, 1))),
            "clamp" => map(&|x| a(x, 0).clamp(a(x, 1), a(x, 2))),
            "abs" => map(&|x| a(x, 0).abs()),
            "floor" => map(&|x| a(x, 0).floor()),
            "fract" => map(&|x| a(x, 0).fract()),
            "power" => map(&|x| a(x, 0).pow0(a(x, 1))),
            "sqrt" => map(&|x| a(x, 0).sqrt0()),
            "sin" => map(&|x| a(x, 0).sin()),
            "cos" => map(&|x| a(x, 0).cos()),
            "mix" => map(&|x| a(x, 0) + (a(x, 1) - a(x, 0)) * a(x, 2)),
            "step" => map(&|x| D::c(if a(x, 1).v >= a(x, 0).v { 1.0 } else { 0.0 })),
            "smoothstep" => map(&|x| dual::smoothstep(a(x, 0), a(x, 1), a(x, 2))),
            "remap" => map(&|x| {
                let t = ((a(x, 0) - a(x, 1)) / dual::guard(a(x, 2) - a(x, 1))).clamp(D::c(0.0), D::c(1.0));
                a(x, 3) + t * (a(x, 4) - a(x, 3))
            }),
            "invert" => map(&|x| D::c(1.0) - a(x, 0)),
            "dot" => {
                let (x, y) = (num(0)?, num(1)?);
                let n = usize::from(x.width.max(y.width));
                one(Num::scalar((0..n).fold(D::c(0.0), |s, k| s + x.at(k) * y.at(k))))
            }
            "length" => {
                let x = num(0)?;
                one(Num::scalar((0..usize::from(x.width)).fold(D::c(0.0), |s, k| s + x.at(k) * x.at(k)).sqrt0()))
            }
            "normalize" => {
                let x = num(0)?;
                let n = usize::from(x.width);
                let len = (0..n).fold(D::c(0.0), |s, k| s + x.at(k) * x.at(k)).sqrt0();
                let c: Vec<D> = if len.v == 0.0 {
                    let z = [0.0, 0.0, 1.0];
                    (0..n).map(|k| D::c(z.get(k).copied().unwrap_or(0.0))).collect()
                } else {
                    (0..n).map(|k| x.at(k) / len).collect()
                };
                one(Num::vec(&c))
            }
            "combine3" => one(Num::vec(&[d(0)?, d(1)?, d(2)?])),
            "separate3" => {
                let x = num(0)?;
                Ok((0..3).map(|k| Value::Num(Num::scalar(x.at(k)))).collect())
            }
            "luminance" => {
                let x = num(0)?;
                one(Num::scalar((0..3).fold(D::c(0.0), |s, k| s + x.at(k) * LUMINANCE[k])))
            }
            "hsv_adjust" => one(Num::constant(&hsv_adjust(v3(0)?, f(1)?, f(2)?, f(3)?))),
            "convert.float_to_color3" | "convert.float_to_vector3" | "convert.color3_to_vector3" | "convert.vector3_to_color3" => {
                let x = num(0)?;
                one(Num::vec(&[x.at(0), x.at(1), x.at(2)]))
            }
            "texcoord.planar" => {
                let (q, _) = self.space(param(0));
                let t = patterns::planar(q, v3(0)?, v3(1)?, v3(2)?, p);
                Ok(texcoord_outputs(t, true))
            }
            "texcoord.triplanar" => {
                let (q, n) = self.space(param(0));
                let t = patterns::triplanar(q, n, v3(0)?, v3(1)?, f(2)?, p);
                let tangent = Value::Num(Num::constant(&t.tangent()));
                Ok(vec![Value::Texcoord(t), tangent])
            }
            "texcoord.cylindrical" => {
                let (q, _) = self.space(param(0));
                let t = patterns::cylindrical(q, v3(0)?, v3(1)?, v3(2)?, f(3)?, param(1) == 0, p);
                Ok(texcoord_outputs(t, true))
            }
            "texcoord.transform" => {
                let t = patterns::transform(tc(0)?, v2(1)?, v2(2)?, f(3)?, p);
                let mut out = texcoord_outputs(t, false);
                out.truncate(2);
                Ok(out)
            }
            "pattern.noise" => {
                let (kind, dims, octaves) = (param(0), param(1), param(2));
                let (scale_mm, lac, gain, s) = (f(2)?, f(3)?, f(4)?, seed(5));
                if dims == 3 {
                    let q = num(1)?;
                    let gmax = (0..3).map(|k| length3(q.at(k).g)).fold(0.0, f64::max);
                    let (v, gp) = patterns::noise3(q.v3(), p.footprint_mm * gmax, scale_mm, lac, gain, s, kind, octaves);
                    let g = (0..3).fold(ZERO3, |acc, k| dual::add3(acc, dual::scale(q.at(k).g, gp[k])));
                    return Ok(vec![Value::Num(Num::scalar(D::new(v, g))), Value::Num(Num::constant(&[0.0, 0.0]))]);
                }
                Ok(blend_pattern(tc(0)?, &|pl| patterns::noise2(pl, scale_mm, lac, gain, s, kind, octaves), 2))
            }
            "pattern.brushed" => {
                let t = tc(0)?;
                let (density, streak, contrast, jitter, s) = (f(1)?, f(2)?, f(3)?, f(4)?, seed(5));
                let mut out = blend_pattern(t, &|pl| patterns::brushed(pl, density, streak, contrast, jitter, s), 2);
                out.push(Value::Num(Num::constant(&t.tangent())));
                Ok(out)
            }
            "pattern.weave" => {
                let (pitch, crown, fibres, fc) = (v2(1)?, f(2)?, f(3)?, f(4)?);
                Ok(blend_pattern(tc(0)?, &|pl| patterns::weave(pl, pitch, crown, fibres, fc, param(0)), 2))
            }
            "pattern.checker" => {
                let size = v2(1)?;
                Ok(blend_pattern(tc(0)?, &|pl| patterns::checker(pl, size), 1))
            }
            "pattern.grid" => {
                let (size, line) = (v2(1)?, f(2)?);
                Ok(blend_pattern(tc(0)?, &|pl| patterns::grid(pl, size, line), 1))
            }
            "pattern.gradient" => {
                let (start, end) = (f(1)?, f(2)?);
                Ok(blend_pattern(tc(0)?, &|pl| patterns::gradient(pl, start, end, param(0) == 1), 2))
            }
            "image" => {
                let texture = match args.get(1) {
                    Some(Arg::Texture(i)) => self.textures.get(usize::from(*i)).copied().flatten(),
                    _ => None,
                };
                self.image(tc(0)?, texture, v3(2)?, param(0) == 0, param(1), param(2))
            }
            "normal.from_height" => one(Num::constant(&patterns::from_height(&num(0)?, f(1)?, p))),
            "normal.map" => {
                let tangent = self.frame_tangent(instr);
                one(Num::constant(&patterns::normal_map(v3(0)?, f(1)?, param(0) == 1, tangent, p)))
            }
            "normal.blend" => one(Num::constant(&unit(mix3(v3(0)?, v3(1)?, f(2)?)))),
            other => err(format!("the reference has no node {other}")),
        }
    }

    fn image(
        &self,
        t: &Texcoord,
        texture: Option<&ReferenceTexture>,
        default: [f64; 3],
        srgb: bool,
        wrap: i64,
        channel: i64,
    ) -> Result<Vec<Value>, ReferenceError> {
        let pick = |rgba: [f64; 4]| match channel {
            1 => rgba[0],
            2 => rgba[1],
            3 => rgba[2],
            4 => rgba[3],
            _ => LUMINANCE[0] * rgba[0] + LUMINANCE[1] * rgba[1] + LUMINANCE[2] * rgba[2],
        };
        let Some(tex) = texture.filter(|t| t.width > 0 && t.height > 0) else {
            let rgba = [default[0], default[1], default[2], 0.0];
            return Ok(vec![Value::Num(Num::constant(&default)), Value::Num(Num::constant(&[pick(rgba)])), Value::Num(Num::constant(&[0.0]))]);
        };
        let h = [1.0 / f64::from(tex.width), 1.0 / f64::from(tex.height)];
        // Per output component (r, g, b, value, alpha): the blended value and world gradient.
        let mut acc = [D::c(0.0); 5];
        for (pl, w) in &t.planes {
            let at = |uv: [f64; 2]| {
                let s = patterns::sample(tex, uv, wrap, srgb);
                [s[0], s[1], s[2], pick(s), s[3]]
            };
            let c = at(pl.uv);
            let cu = at([pl.uv[0] + h[0], pl.uv[1]]);
            let cv = at([pl.uv[0], pl.uv[1] + h[1]]);
            for k in 0..5 {
                let g = dual::add3(dual::scale(pl.gu, (cu[k] - c[k]) / h[0]), dual::scale(pl.gv, (cv[k] - c[k]) / h[1]));
                acc[k] = acc[k] + D::new(c[k], g) * *w;
            }
        }
        Ok(vec![Value::Num(Num::vec(&acc[0..3])), Value::Num(Num::scalar(acc[3])), Value::Num(Num::scalar(acc[4]))])
    }

    /// The tangent frame of a `normal.map`: the texcoord of the nearest `image` upstream of its
    /// colour, following each instruction's first value argument; else the geometric tangent.
    fn frame_tangent(&self, instr: &Instr) -> [f64; 3] {
        let mut at = instr.args.first().copied();
        for _ in 0..=self.ir.instrs.len() {
            let Some(Operand::Value(ValueId(v))) = at else { break };
            let Some(source) = usize::try_from(v).ok().and_then(|i| self.ir.instrs.get(i)) else { break };
            if source.kind() == "image" {
                let call = if source.selects().is_some() { source.args.first().copied() } else { Some(Operand::Value(source.result)) };
                let image = match call {
                    Some(Operand::Value(ValueId(c))) => usize::try_from(c).ok().and_then(|i| self.ir.instrs.get(i)),
                    _ => None,
                };
                if let Some(Ok(Arg::Tc(t))) = image.and_then(|i| i.args.first()).map(|a| self.arg(a)) {
                    return t.tangent();
                }
                break;
            }
            at = source.args.iter().find(|a| matches!(a, Operand::Value(_))).copied();
        }
        self.p.tangent
    }

    fn surface(&self, ops: &[(&'static str, Operand)]) -> Result<ReferenceSurface, ReferenceError> {
        let spec = node_spec("standard_surface").ok_or_else(|| ReferenceError("no standard_surface".into()))?;
        let mut values: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        for input in spec.inputs {
            let op = ops.iter().find(|(n, _)| *n == input.name).map(|(_, o)| o);
            let given = match op {
                Some(op) => match self.arg(op)? {
                    Arg::Num(n) => Some(n),
                    _ => None,
                },
                None => None,
            };
            let w = width(input.ty);
            let default: Vec<f64> = match input.default {
                InputDefault::Float(x) => vec![x; w],
                InputDefault::Vec3(v) => v.to_vec(),
                InputDefault::Vec2(v) => v.to_vec(),
                _ => match input.ty {
                    PortType::Normal => self.p.world_normal.to_vec(),
                    _ => self.p.tangent.to_vec(),
                },
            };
            let mut v: Vec<f64> = (0..w).map(|k| given.map_or(f64::NAN, |n| n.at(k).v)).collect();
            for (k, x) in v.iter_mut().enumerate() {
                if x.is_nan() {
                    *x = default.get(k).copied().unwrap_or(0.0);
                }
                if let Some((lo, hi)) = input.range {
                    *x = x.clamp(lo, hi);
                }
            }
            values.insert(input.name, v);
        }
        let s = |name: &str| values.get(name).and_then(|v| v.first()).copied().unwrap_or(0.0);
        let c = |name: &str| {
            let v = values.get(name);
            let at = |k: usize| v.and_then(|v| v.get(k)).copied().unwrap_or(0.0);
            [at(0), at(1), at(2)]
        };
        let normal_of = |name: &str| {
            let v = c(name);
            if v.iter().any(|x| !x.is_finite()) { self.p.world_normal } else { unit(v) }
        };
        let normal = normal_of("geometry_normal");
        let t = tangential(c("geometry_tangent"), normal);
        let tangent = if length3(t) < 1e-12 || t.iter().any(|x| !x.is_finite()) { self.p.tangent } else { unit(t) };
        let weight = s("base_weight");
        Ok(ReferenceSurface {
            base_color: c("base_color").map(|x| x * weight),
            metalness: s("base_metalness"),
            roughness: s("specular_roughness"),
            ior: s("specular_ior"),
            coat_weight: s("coat_weight"),
            coat_roughness: s("coat_roughness"),
            coat_ior: s("coat_ior"),
            normal,
            coat_normal: normal_of("geometry_coat_normal"),
            specular_weight: s("specular_weight"),
            specular_color: c("specular_color"),
            anisotropy: s("specular_roughness_anisotropy"),
            rotation: s("specular_rotation"),
            tangent,
            coat_color: c("coat_color"),
            emission_luminance: s("emission_luminance"),
            emission_color: c("emission_color"),
            transmission_weight: s("transmission_weight"),
            transmission_color: c("transmission_color"),
            transmission_depth_mm: s("transmission_depth_mm"),
            opacity: s("geometry_opacity"),
        })
    }
}

/// A projection's outputs: the texcoord, then (with `tangent`) uv and tangent, else uv.
fn texcoord_outputs(t: Texcoord, tangent: bool) -> Vec<Value> {
    let first = t.planes.first().map(|(p, _)| *p);
    let uv = first.map_or(Num::constant(&[0.0, 0.0]), |p| Num::vec(&[D::new(p.uv[0], p.gu), D::new(p.uv[1], p.gv)]));
    let tan = Num::constant(&t.tangent());
    let mut out = vec![Value::Texcoord(t), Value::Num(uv)];
    if tangent {
        out.push(Value::Num(tan));
    }
    out
}

/// A pattern blended over a texcoord's planes: its value (with the world gradient) and, when
/// `outputs` is 2, its slope.
fn blend_pattern(t: &Texcoord, f: &dyn Fn(&Plane) -> patterns::PatternOut, outputs: usize) -> Vec<Value> {
    let mut value = D::c(0.0);
    let mut slope = [0.0; 2];
    for (pl, w) in &t.planes {
        let o = f(pl);
        let g = dual::add3(dual::scale(pl.gu, o.slope[0]), dual::scale(pl.gv, o.slope[1]));
        value = value + D::new(o.value, g) * *w;
        slope = [slope[0] + w * o.slope[0], slope[1] + w * o.slope[1]];
    }
    let mut out = vec![Value::Num(Num::scalar(value))];
    if outputs == 2 {
        out.push(Value::Num(Num::constant(&slope)));
    }
    out
}

/// `hsv_adjust`: hex-cone HSV, hue turned by `hue` radians, saturation scaled and clamped to
/// [0, 1], value scaled.
pub fn hsv_adjust(c: [f64; 3], hue: f64, saturation: f64, value: f64) -> [f64; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let mut h = if delta <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    h = (h + hue / std::f64::consts::TAU).rem_euclid(1.0);
    let s = (s * saturation).clamp(0.0, 1.0);
    let v = max * value;
    let k = |n: f64| {
        let k = (n + h * 6.0).rem_euclid(6.0);
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [k(5.0), k(3.0), k(1.0)]
}
