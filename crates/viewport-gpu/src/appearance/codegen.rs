//! The raster program for a constant standard surface. Uniform values never enter WGSL text.
use solidcraft_viewport::appearance_graph::catalogue::{InputDefault, node_spec};
use solidcraft_viewport::appearance_graph::ir::{Ir, Operand};

/// The base response occupies seven vec4s before the graph's uniform slots.
pub const BASE_VEC4: u32 = 7;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("appearance graph code generation: {0}")]
pub struct CodegenError(pub String);

/// Read an aligned value through its uniform slot, or use the catalogue default.
fn field(ir: &Ir, name: &str) -> Result<String, CodegenError> {
    let port = node_spec("standard_surface").and_then(|s| s.input(name)).ok_or_else(|| CodegenError(format!("unknown surface input {name}")))?;
    let default = match port.default {
        InputDefault::Float(x) => format!("{:?}", x as f32),
        InputDefault::Vec3(v) => format!("vec3<f32>({:?}, {:?}, {:?})", v[0] as f32, v[1] as f32, v[2] as f32),
        _ => return Err(CodegenError(format!("unsupported surface input {name}"))),
    };
    let op = ir.surface.iter().find(|(key, _)| *key == name).map(|(_, op)| *op);
    let Some(op) = op.filter(|v| *v != Operand::Absent) else { return Ok(default) };
    let Operand::Uniform { offset, width } = op else { return Err(CodegenError(format!("{name} requires a constant"))) };
    let (lo, hi) = port.range.unwrap_or((0.0, 1.0));
    let index = BASE_VEC4 + offset / 4;
    let component = match (width, offset % 4) {
        (1, 0) => "x",
        (1, 1) => "y",
        (1, 2) => "z",
        (1, 3) => "w",
        (3, 0) => "xyz",
        _ => return Err(CodegenError(format!("invalid uniform at float {offset}"))),
    };
    let value = format!("mat_page.m[record + {index}u].{component}");
    let clean = if *width == 3 { "sc_clean3" } else { "sc_clean" };
    Ok(format!("{clean}({value}, {:?}, {:?}, {default})", lo as f32, hi as f32))
}

/// Generate the evaluator for the opaque part of a standard surface.
pub fn appearance_eval(ir: &Ir) -> Result<String, CodegenError> {
    if !ir.instrs.is_empty() || !ir.mix_layers.is_empty() {
        return Err(CodegenError("this shape requires node evaluation".into()));
    }
    let base = field(ir, "base_color")?;
    let weight = field(ir, "base_weight")?;
    let metallic = field(ir, "base_metalness")?;
    let roughness = field(ir, "specular_roughness")?;
    let ior = field(ir, "specular_ior")?;
    Ok(format!(
        "fn appearance_eval(record: u32) -> AppearanceSurface {{ return AppearanceSurface({base} * {weight}, vec3<f32>({metallic}, {roughness}, {ior})); }}"
    ))
}

/// Draw an appearance's prepared base response while its shape pipeline is pending.
pub const FALLBACK_EVAL: &str = "fn appearance_eval(record: u32) -> AppearanceSurface { let a = mat_page.m[record]; let b = mat_page.m[record + 1u]; return AppearanceSurface(a.xyz, vec3<f32>(a.w, b.x, b.y)); }";
