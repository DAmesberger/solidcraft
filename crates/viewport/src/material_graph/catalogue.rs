//! The node catalogue, v1: every node kind a graph may use, with its typed inputs (ranges and
//! defaults), compile-time params, outputs and cost. Validation, the code generators, the
//! commands (`appearance.graph.nodes`) and the editor all read this one table.
//!
//! Input names of `standard_surface` follow OpenPBR Surface; other node names follow the
//! MaterialX standard library where it has the same node. Lengths are millimetres, angles
//! radians, colours linear Rec.709.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

/// The catalogue's version. A new node kind, input or param is a design-format change (its own
/// file in `crates/doc/src/format/pending/`) and bumps this.
pub const CATALOGUE_VERSION: u32 = 1;

/// The type of a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    Float,
    /// An integer constant (seeds). Never connected.
    Int,
    /// Linear Rec.709 RGB.
    Color3,
    Vector2,
    Vector3,
    /// A projected coordinate: uv in mm, the tangent frame it defines and the pixel footprint.
    Texcoord,
    /// A unit shading normal in world space.
    Normal,
    /// A texture asset (by SHA-256), from a constant or an interface input; only `image` reads it.
    Texture,
    /// The output of a surface node.
    Surface,
    /// Float, Vector2, Vector3 or Color3: maths nodes take one of these and keep it. Validation
    /// resolves it per node; a Float broadcasts.
    Numeric,
}

/// What an input is when nothing drives it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputDefault {
    /// It must be connected (or, for a texture, given).
    Required,
    /// It may stay unconnected; the node says what that means (a noise reads either `texcoord`
    /// or `position`).
    Optional,
    /// The geometric normal or the texcoord's tangent.
    Geometric,
    Float(f64),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
}

/// One input of a node kind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct InputSpec {
    pub name: &'static str,
    #[serde(rename = "type")]
    pub ty: PortType,
    /// Closed range of each component of a constant; a constant outside it refuses on load.
    /// Wires computed at run time are clamped to it by the surface node.
    pub range: Option<(f64, f64)>,
    pub default: InputDefault,
}

/// A compile-time param: part of a graph's shape (it changes generated code).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Choice { options: &'static [&'static str], default: &'static str },
    Int { min: i64, max: i64, default: i64 },
}

/// One param of a node kind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
}

/// One output of a node kind. A connection without an output name reads the first.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct OutputSpec {
    pub name: &'static str,
    #[serde(rename = "type")]
    pub ty: PortType,
}

/// A node kind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NodeSpec {
    pub kind: &'static str,
    pub inputs: &'static [InputSpec],
    pub params: &'static [ParamSpec],
    pub outputs: &'static [OutputSpec],
    /// Cost units before expansion (§3.6): noise is per octave; slopes and triplanar multiply.
    pub cost: u32,
}

impl NodeSpec {
    pub fn input(&self, name: &str) -> Option<&'static InputSpec> {
        self.inputs.iter().find(|i| i.name == name)
    }
    pub fn param(&self, name: &str) -> Option<&'static ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }
    pub fn output(&self, name: &str) -> Option<&'static OutputSpec> {
        self.outputs.iter().find(|o| o.name == name)
    }
}

/// The node kind called `kind`, if the catalogue has it.
pub fn node_spec(kind: &str) -> Option<&'static NodeSpec> {
    CATALOGUE.iter().find(|n| n.kind == kind)
}

const fn float(name: &'static str, min: f64, max: f64, default: f64) -> InputSpec {
    InputSpec { name, ty: PortType::Float, range: Some((min, max)), default: InputDefault::Float(default) }
}
const fn color(name: &'static str, default: [f64; 3]) -> InputSpec {
    InputSpec { name, ty: PortType::Color3, range: Some((0.0, 1.0)), default: InputDefault::Vec3(default) }
}
const fn vec3(name: &'static str, min: f64, max: f64, default: [f64; 3]) -> InputSpec {
    InputSpec { name, ty: PortType::Vector3, range: Some((min, max)), default: InputDefault::Vec3(default) }
}
const fn vec2(name: &'static str, min: f64, max: f64, default: [f64; 2]) -> InputSpec {
    InputSpec { name, ty: PortType::Vector2, range: Some((min, max)), default: InputDefault::Vec2(default) }
}
const fn port(name: &'static str, ty: PortType, default: InputDefault) -> InputSpec {
    InputSpec { name, ty, range: None, default }
}
const fn numeric(name: &'static str, default: f64) -> InputSpec {
    InputSpec { name, ty: PortType::Numeric, range: None, default: InputDefault::Float(default) }
}
const fn out(name: &'static str, ty: PortType) -> OutputSpec {
    OutputSpec { name, ty }
}
const fn choice(name: &'static str, options: &'static [&'static str], default: &'static str) -> ParamSpec {
    ParamSpec { name, kind: ParamKind::Choice { options, default } }
}
const fn int(name: &'static str, min: i64, max: i64, default: i64) -> ParamSpec {
    ParamSpec { name, kind: ParamKind::Int { min, max, default } }
}
const fn node(kind: &'static str, inputs: &'static [InputSpec], params: &'static [ParamSpec], outputs: &'static [OutputSpec], cost: u32) -> NodeSpec {
    NodeSpec { kind, inputs, params, outputs, cost }
}

const SEED: InputSpec = InputSpec { name: "seed", ty: PortType::Int, range: Some((0.0, 2_147_483_647.0)), default: InputDefault::Float(0.0) };
const SPACE: ParamSpec = choice("space", &["object", "world"], "object");
const TEXCOORD: InputSpec = port("texcoord", PortType::Texcoord, InputDefault::Required);
const NUMERIC_OUT: &[OutputSpec] = &[out("out", PortType::Numeric)];
const FLOAT_OUT: &[OutputSpec] = &[out("out", PortType::Float)];
const PATTERN_OUT: &[OutputSpec] = &[out("value", PortType::Float), out("slope", PortType::Vector2)];
const LENGTH: f64 = 1e6;

/// The v1 node catalogue (docs/material-graph.md §3).
pub const CATALOGUE: &[NodeSpec] = &[
    // Surfaces.
    node(
        "standard_surface",
        &[
            float("base_weight", 0.0, 1.0, 1.0),
            color("base_color", [0.8, 0.8, 0.8]),
            float("base_metalness", 0.0, 1.0, 0.0),
            float("specular_weight", 0.0, 1.0, 1.0),
            color("specular_color", [1.0, 1.0, 1.0]),
            float("specular_roughness", crate::surface::MIN_ROUGHNESS, 1.0, 0.3),
            float("specular_ior", 1.0, 3.0, 1.5),
            float("specular_roughness_anisotropy", 0.0, 1.0, 0.0),
            float("specular_rotation", -PI, PI, 0.0),
            port("geometry_tangent", PortType::Vector3, InputDefault::Geometric),
            float("coat_weight", 0.0, 1.0, 0.0),
            color("coat_color", [1.0, 1.0, 1.0]),
            float("coat_roughness", crate::surface::MIN_ROUGHNESS, 1.0, 0.1),
            float("coat_ior", 1.0, 3.0, 1.5),
            port("geometry_coat_normal", PortType::Normal, InputDefault::Geometric),
            port("geometry_normal", PortType::Normal, InputDefault::Geometric),
            float("transmission_weight", 0.0, 1.0, 0.0),
            color("transmission_color", [1.0, 1.0, 1.0]),
            float("transmission_depth_mm", 0.001, 1000.0, 1.0),
            float("emission_luminance", 0.0, 1000.0, 0.0),
            color("emission_color", [1.0, 1.0, 1.0]),
            float("geometry_opacity", 0.0, 1.0, 1.0),
        ],
        &[],
        &[out("surface", PortType::Surface)],
        0,
    ),
    node(
        "surface.mix",
        &[port("fg", PortType::Surface, InputDefault::Required), port("bg", PortType::Surface, InputDefault::Required), float("mix", 0.0, 1.0, 0.5)],
        &[],
        &[out("surface", PortType::Surface)],
        10,
    ),
    // Geometry and projections.
    node("geometry.position", &[], &[SPACE], &[out("value", PortType::Vector3)], 0),
    node("geometry.normal", &[], &[SPACE], &[out("value", PortType::Vector3)], 0),
    node("geometry.facing", &[], &[], &[out("value", PortType::Float)], 1),
    node(
        "texcoord.planar",
        &[vec3("origin", -1e7, 1e7, [0.0, 0.0, 0.0]), vec3("u", -LENGTH, LENGTH, [1.0, 0.0, 0.0]), vec3("v", -LENGTH, LENGTH, [0.0, 1.0, 0.0])],
        &[SPACE],
        &[out("texcoord", PortType::Texcoord), out("uv", PortType::Vector2), out("tangent", PortType::Vector3)],
        4,
    ),
    node(
        "texcoord.triplanar",
        &[vec3("origin", -1e7, 1e7, [0.0, 0.0, 0.0]), vec3("scale_mm", 0.001, LENGTH, [1.0, 1.0, 1.0]), float("sharpness", 1.0, 64.0, 4.0)],
        &[SPACE],
        &[out("texcoord", PortType::Texcoord), out("tangent", PortType::Vector3)],
        6,
    ),
    node(
        "texcoord.cylindrical",
        &[
            vec3("origin", -1e7, 1e7, [0.0, 0.0, 0.0]),
            vec3("axis", -1.0, 1.0, [0.0, 0.0, 1.0]),
            vec3("reference", -1.0, 1.0, [1.0, 0.0, 0.0]),
            float("radius_mm", 0.001, LENGTH, 1.0),
        ],
        &[SPACE, choice("unwrap", &["arc_length", "angle"], "arc_length")],
        &[out("texcoord", PortType::Texcoord), out("uv", PortType::Vector2), out("tangent", PortType::Vector3)],
        8,
    ),
    node(
        "texcoord.transform",
        &[TEXCOORD, vec2("offset", -LENGTH, LENGTH, [0.0, 0.0]), vec2("scale", 1e-3, 1e3, [1.0, 1.0]), float("rotation", -PI, PI, 0.0)],
        &[],
        &[out("texcoord", PortType::Texcoord), out("uv", PortType::Vector2)],
        4,
    ),
    // Patterns.
    node(
        "pattern.noise",
        &[
            port("texcoord", PortType::Texcoord, InputDefault::Optional),
            port("position", PortType::Vector3, InputDefault::Optional),
            float("scale_mm", 0.001, 1e4, 1.0),
            float("lacunarity", 1.0, 4.0, 2.0),
            float("gain", 0.0, 1.0, 0.5),
            SEED,
        ],
        &[
            choice("kind", &["value", "gradient", "cellular"], "value"),
            int("dimensions", 2, 3, 2),
            int("octaves", 1, super::limits::NOISE_OCTAVES, 1),
        ],
        PATTERN_OUT,
        20,
    ),
    node(
        "pattern.brushed",
        &[
            TEXCOORD,
            float("density_per_mm", 0.1, 1000.0, 20.0),
            float("streak_mm", 0.01, 1000.0, 2.0),
            float("contrast", 0.0, 1.0, 0.5),
            float("jitter", 0.0, 1.0, 0.3),
            SEED,
        ],
        &[],
        &[out("value", PortType::Float), out("slope", PortType::Vector2), out("tangent", PortType::Vector3)],
        30,
    ),
    node(
        "pattern.weave",
        &[
            TEXCOORD,
            vec2("pitch_mm", 0.01, 100.0, [1.0, 1.0]),
            float("crown", 0.0, 1.0, 1.0),
            float("fibres", 0.0, 32.0, 7.0),
            float("fibre_contrast", 0.0, 1.0, 0.3),
        ],
        &[choice("kind", &["plain", "twill_2x2"], "plain")],
        PATTERN_OUT,
        60,
    ),
    node("pattern.checker", &[TEXCOORD, vec2("size_mm", 0.001, LENGTH, [1.0, 1.0])], &[], &[out("value", PortType::Float)], 6),
    node(
        "pattern.grid",
        &[TEXCOORD, vec2("size_mm", 0.001, LENGTH, [1.0, 1.0]), float("line_mm", 0.0001, LENGTH, 0.05)],
        &[],
        &[out("value", PortType::Float)],
        8,
    ),
    node(
        "pattern.gradient",
        &[TEXCOORD, float("start", -LENGTH, LENGTH, 0.0), float("end", -LENGTH, LENGTH, 1.0)],
        &[choice("kind", &["linear", "radial"], "linear")],
        PATTERN_OUT,
        4,
    ),
    node(
        "image",
        &[TEXCOORD, port("file", PortType::Texture, InputDefault::Required), color("default", [0.5, 0.5, 0.5])],
        &[
            choice("space", &["srgb", "linear"], "srgb"),
            choice("wrap", &["repeat", "clamp", "mirror"], "repeat"),
            choice("channel", &["rgb", "r", "g", "b", "a"], "rgb"),
        ],
        &[out("color", PortType::Color3), out("value", PortType::Float), out("alpha", PortType::Float)],
        8,
    ),
    // Normals.
    node(
        "normal.from_height",
        &[port("height", PortType::Float, InputDefault::Required), float("relief_mm", 0.0, 1.0, 0.01)],
        &[],
        &[out("normal", PortType::Normal)],
        6,
    ),
    node(
        "normal.map",
        &[port("color", PortType::Color3, InputDefault::Required), float("strength", 0.0, 1.0, 1.0)],
        &[choice("convention", &["opengl", "directx"], "opengl")],
        &[out("normal", PortType::Normal)],
        8,
    ),
    node(
        "normal.blend",
        &[port("a", PortType::Normal, InputDefault::Geometric), port("b", PortType::Normal, InputDefault::Geometric), float("mix", 0.0, 1.0, 0.5)],
        &[],
        &[out("normal", PortType::Normal)],
        4,
    ),
    // Maths. Inputs default to a value with no effect.
    node("constant", &[numeric("value", 0.0)], &[], NUMERIC_OUT, 0),
    node("add", &[numeric("a", 0.0), numeric("b", 0.0)], &[], NUMERIC_OUT, 1),
    node("subtract", &[numeric("a", 0.0), numeric("b", 0.0)], &[], NUMERIC_OUT, 1),
    node("multiply", &[numeric("a", 1.0), numeric("b", 1.0)], &[], NUMERIC_OUT, 1),
    node("divide", &[numeric("a", 1.0), numeric("b", 1.0)], &[], NUMERIC_OUT, 2),
    node("min", &[numeric("a", 0.0), numeric("b", 0.0)], &[], NUMERIC_OUT, 1),
    node("max", &[numeric("a", 0.0), numeric("b", 0.0)], &[], NUMERIC_OUT, 1),
    node("clamp", &[numeric("in", 0.0), numeric("low", 0.0), numeric("high", 1.0)], &[], NUMERIC_OUT, 2),
    node("abs", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 1),
    node("floor", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 1),
    node("fract", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 1),
    node("power", &[numeric("a", 1.0), numeric("b", 1.0)], &[], NUMERIC_OUT, 4),
    node("sqrt", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 2),
    node("sin", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 4),
    node("cos", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 4),
    node("mix", &[numeric("a", 0.0), numeric("b", 1.0), float("t", 0.0, 1.0, 0.5)], &[], NUMERIC_OUT, 2),
    node("step", &[numeric("edge", 0.5), numeric("in", 0.0)], &[], NUMERIC_OUT, 1),
    node("smoothstep", &[numeric("edge0", 0.0), numeric("edge1", 1.0), numeric("in", 0.0)], &[], NUMERIC_OUT, 4),
    node(
        "remap",
        &[numeric("in", 0.0), numeric("in_min", 0.0), numeric("in_max", 1.0), numeric("out_min", 0.0), numeric("out_max", 1.0)],
        &[],
        NUMERIC_OUT,
        2,
    ),
    node("invert", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 1),
    node("dot", &[numeric("a", 0.0), numeric("b", 0.0)], &[], FLOAT_OUT, 2),
    node("length", &[numeric("in", 0.0)], &[], FLOAT_OUT, 2),
    node("normalize", &[numeric("in", 0.0)], &[], NUMERIC_OUT, 3),
    node(
        "combine3",
        &[float("x", -LENGTH, LENGTH, 0.0), float("y", -LENGTH, LENGTH, 0.0), float("z", -LENGTH, LENGTH, 0.0)],
        &[],
        &[out("out", PortType::Vector3)],
        0,
    ),
    node("separate3", &[numeric("in", 0.0)], &[], &[out("x", PortType::Float), out("y", PortType::Float), out("z", PortType::Float)], 0),
    node("luminance", &[color("in", [0.0, 0.0, 0.0])], &[], FLOAT_OUT, 2),
    node(
        "hsv_adjust",
        &[color("in", [0.0, 0.0, 0.0]), float("hue", -PI, PI, 0.0), float("saturation", 0.0, 4.0, 1.0), float("value", 0.0, 4.0, 1.0)],
        &[],
        &[out("out", PortType::Color3)],
        12,
    ),
    node("convert.float_to_color3", &[float("in", -LENGTH, LENGTH, 0.0)], &[], &[out("out", PortType::Color3)], 0),
    node("convert.float_to_vector3", &[float("in", -LENGTH, LENGTH, 0.0)], &[], &[out("out", PortType::Vector3)], 0),
    node("convert.color3_to_vector3", &[color("in", [0.0, 0.0, 0.0])], &[], &[out("out", PortType::Vector3)], 0),
    node("convert.vector3_to_color3", &[vec3("in", -LENGTH, LENGTH, [0.0, 0.0, 0.0])], &[], &[out("out", PortType::Color3)], 0),
];
