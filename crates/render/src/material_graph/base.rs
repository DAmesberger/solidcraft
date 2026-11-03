//! The base response (docs/material-graph.md §2, §5.7): a static read of the output surface's
//! inputs, constants as they are and driven inputs at their fallback or default. No node is
//! evaluated. Also the uniform words of one material, from the IR's slots and a look's values.

use std::collections::BTreeMap;

use solidcraft_geom::Vec3;

use super::catalogue::{InputSpec, PortType};
use super::graph::Constant;
use super::ir::{GraphIssue, Ir};
use super::lower::surface_chain;
use super::sample::SurfaceSample;
use super::validate::{Analysis, Bound, Issues, Ty, check_constant, components, default_components, shown};
use crate::clear_coat::ClearCoat;
use crate::surface::OpaqueSurface;

/// What input `input` reads without evaluating anything.
fn read(an: &Analysis, input: &InputSpec, bound: &Bound) -> Vec<f64> {
    let width = Ty::from_port(input.ty).map_or(1, |t| usize::from(t.width()));
    let given = match bound {
        Bound::Constant(c) => Some(c),
        Bound::Connection { fallback, .. } => fallback.as_ref(),
        Bound::Interface(j) => an.interface.get(*j).and_then(|i| i.default.as_ref()),
        Bound::Unset => None,
    };
    let mut v = match given {
        Some(c) => components(c, width),
        None => default_components(input),
    };
    v.resize(width, v.first().copied().unwrap_or(0.0));
    v
}

/// The base sample of a resolved graph: the `standard_surface` at the bottom of its `bg` chain.
pub(super) fn sample(an: &Analysis) -> Result<SurfaceSample, Vec<GraphIssue>> {
    let mut issues = Issues::default();
    let Some(node) = surface_chain(an).1.and_then(|n| an.nodes.get(n)) else {
        issues.push(None, None, "output", "the output reaches no standard_surface".to_owned());
        return Err(issues.0);
    };
    let values: BTreeMap<&str, Vec<f64>> = node.spec.inputs.iter().zip(&node.inputs).map(|(i, b)| (i.name, read(an, i, b))).collect();
    let scalar = |name: &str| values.get(name).and_then(|v| v.first()).copied().unwrap_or(0.0);
    let color = |name: &str| {
        values
            .get(name)
            .map_or(Vec3::ZERO, |v| Vec3::new(v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0), v.get(2).copied().unwrap_or(0.0)))
    };
    let weight = scalar("base_weight");
    let base = color("base_color");
    // A weight of 1 leaves the colour bit-identical, so a `pbr`-equivalent graph matches exactly.
    let base = if weight == 1.0 { base } else { base * weight };
    let opaque = OpaqueSurface::new(base, scalar("base_metalness"), scalar("specular_roughness"), scalar("specular_ior"))
        .and_then(|s| ClearCoat::new(scalar("coat_weight"), scalar("coat_roughness"), scalar("coat_ior")).map(|c| s.with_clear_coat(c)));
    match opaque {
        Ok(opaque) => Ok(SurfaceSample {
            opaque,
            specular_weight: scalar("specular_weight"),
            specular_color: color("specular_color"),
            anisotropy: scalar("specular_roughness_anisotropy"),
            rotation: scalar("specular_rotation"),
            coat_color: color("coat_color"),
            emission_luminance: scalar("emission_luminance"),
            emission_color: color("emission_color"),
            transmission_weight: scalar("transmission_weight"),
            transmission_color: color("transmission_color"),
            transmission_depth_mm: scalar("transmission_depth_mm"),
            opacity: scalar("geometry_opacity"),
        }),
        Err(e) => {
            issues.push(None, None, "base_response", format!("the base response is not a valid surface: {e}"));
            Err(issues.0)
        }
    }
}

/// The components of a look's value for a slot, checked against its type and range.
fn override_components(slot: &super::ir::UniformSlot, name: &str, c: &Constant) -> Result<Vec<f64>, GraphIssue> {
    let ty = Ty::from_port(slot.ty).unwrap_or(Ty::Float);
    // A stand-in input carrying the slot's type and range, so the one constant check applies.
    let input = InputSpec { name: "value", ty: slot.ty, range: slot.range, default: super::catalogue::InputDefault::Required };
    let issue = |code: &'static str, message: String| GraphIssue {
        node: None,
        port: Some(shown(name)),
        code,
        message: format!("interface {}: {message}", shown(name)),
    };
    if slot.ty == PortType::Int && !matches!(c, Constant::Number(x) if x.is_finite() && x.fract() == 0.0 && (0.0..=f64::from(i32::MAX)).contains(x)) {
        return Err(issue("constant_type", "takes an integer in [0, 2147483647]".to_owned()));
    }
    check_constant(&input, ty, c).map_err(|(code, message)| issue(code, message))?;
    let mut v = components(c, usize::from(slot.width));
    v.resize(usize::from(slot.width), v.first().copied().unwrap_or(0.0));
    Ok(v)
}

pub(super) fn uniform_words(ir: &Ir, values: &BTreeMap<String, Constant>) -> Result<Vec<u32>, Vec<GraphIssue>> {
    let len = usize::try_from(ir.uniform_floats).unwrap_or(0).min(super::limits::UNIFORM_FLOATS);
    let mut words = vec![0u32; len];
    let mut issues = Vec::new();
    for slot in &ir.slots {
        let value = match slot.interface.as_deref().and_then(|name| values.get(name).map(|c| (name, c))) {
            Some((name, c)) => match override_components(slot, name, c) {
                Ok(v) => v,
                Err(issue) => {
                    if issues.len() < super::validate::MAX_ISSUES {
                        issues.push(issue);
                    }
                    continue;
                }
            },
            None => slot.value.clone(),
        };
        let base = usize::try_from(slot.offset).unwrap_or(usize::MAX);
        for (k, x) in value.iter().take(usize::from(slot.width)).enumerate() {
            let word = if slot.ty == PortType::Int { x.clamp(0.0, f64::from(u32::MAX)) as u32 } else { (*x as f32).to_bits() };
            if let Some(w) = base.checked_add(k).and_then(|i| words.get_mut(i)) {
                *w = word;
            }
        }
    }
    if issues.is_empty() { Ok(words) } else { Err(issues) }
}
