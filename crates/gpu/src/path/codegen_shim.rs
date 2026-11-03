//! TEMPORARY (track PT): a stand-in for the shared generator core that track C1 is extracting in
//! `crate::material::codegen` (`generate(ir, &dyn Target)`), with the same interface. It covers
//! constant graphs only (no instructions), enough to test the path back end's table, hook and
//! pipeline. Deleted when C1's core lands; it is never a second generator.
#![allow(dead_code)]

use solidcraft_render::material_graph::catalogue::{InputDefault, PortType, node_spec};
use solidcraft_render::material_graph::ir::{Ir, Operand};

/// vec4s of the base response at the head of every material's record (C1's `BASE_VEC4`).
pub const BASE_VEC4: u32 = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodegenError(pub String);

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "material graph code generation: {}", self.0)
    }
}

/// An `image` node's compile-time params.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageParams {
    pub srgb: bool,
    /// 0 repeat, 1 clamp, 2 mirror.
    pub wrap: u32,
    /// 0 rgb (luminance), 1–3 r, g, b, 4 alpha.
    pub channel: u32,
}

/// What differs between the two back ends.
pub trait Target {
    fn name(&self) -> &str;
    fn vec4(&self, index: &str) -> String;
    fn image(&self, binding: u8, tc: &str, default: &str, params: ImageParams) -> String;
}

fn lit(x: f64) -> String {
    let s = format!("{:?}", x as f32);
    if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
}

pub fn generate(ir: &Ir, target: &dyn Target) -> Result<String, CodegenError> {
    if !ir.instrs.is_empty() || !ir.mix_layers.is_empty() {
        return Err(CodegenError("the stand-in generator covers constant graphs only".to_owned()));
    }
    let spec = node_spec("standard_surface").ok_or_else(|| CodegenError("no standard_surface".to_owned()))?;
    let field = |name: &str| -> Result<String, CodegenError> {
        let input = spec.input(name).ok_or_else(|| CodegenError(format!("no input {name}")))?;
        let op = ir.surface.iter().find(|(n, _)| *n == name).map(|(_, o)| *o).unwrap_or(Operand::Absent);
        let (lo, hi) = input.range.unwrap_or((0.0, 1.0));
        let read = |offset: u32, width: u8| {
            let swizzle = match (width, offset % 4) {
                (1, 0) => "x",
                (1, 1) => "y",
                (1, 2) => "z",
                (1, _) => "w",
                (2, 0) => "xy",
                (2, _) => "zw",
                _ => "xyz",
            };
            format!("{}.{swizzle}", target.vec4(&format!("k + {}u", offset / 4)))
        };
        Ok(match (input.ty, op) {
            (PortType::Normal, _) => "c.n".to_owned(),
            (PortType::Vector3, _) => "sc_default_tangent(c.n)".to_owned(),
            (PortType::Color3, op) => {
                let d = match input.default {
                    InputDefault::Vec3(d) => format!("vec3<f32>({}, {}, {})", lit(d[0]), lit(d[1]), lit(d[2])),
                    _ => "vec3<f32>(1.0)".to_owned(),
                };
                match op {
                    Operand::Uniform { offset, width } => format!("sc_clean3({}, {}, {}, {d})", read(offset, width), lit(lo), lit(hi)),
                    _ => d,
                }
            }
            (_, op) => {
                let d = match input.default {
                    InputDefault::Float(d) => lit(d),
                    _ => "0.0".to_owned(),
                };
                match op {
                    Operand::Uniform { offset, width } => format!("sc_clean({}, {}, {}, {d})", read(offset, width), lit(lo), lit(hi)),
                    _ => d,
                }
            }
        })
    };
    let names = [
        "base_color",
        "base_metalness",
        "specular_roughness",
        "specular_ior",
        "coat_weight",
        "coat_roughness",
        "coat_ior",
        "geometry_normal",
        "geometry_coat_normal",
        "specular_weight",
        "specular_color",
        "specular_roughness_anisotropy",
        "specular_rotation",
        "geometry_tangent",
        "coat_color",
        "emission_luminance",
        "emission_color",
        "transmission_weight",
        "transmission_color",
        "transmission_depth_mm",
        "geometry_opacity",
    ];
    let mut args = Vec::with_capacity(names.len());
    for name in names {
        args.push(field(name)?);
    }
    if let Some(first) = args.first_mut() {
        *first = format!("{first} * {}", field("base_weight")?);
    }
    Ok(format!(
        "fn {}(c: sc_GraphCtx) -> sc_Surface {{\n    let k = c.record + {BASE_VEC4}u;\n    return sc_Surface({});\n}}\n",
        target.name(),
        args.join(", ")
    ))
}
