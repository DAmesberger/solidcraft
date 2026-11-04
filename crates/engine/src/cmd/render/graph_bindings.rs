//! The direction table of `render.library.appearance.apply`'s `graph` variant
//! (`assets/appearances/graphs/render-library-bindings.json`, docs/render-material-graphs.md):
//! a starter surface or Selene appearance with a direction gets a specular anisotropy and a
//! tangent turned from its projection's u axis. The amounts are authored estimates, and the
//! reply says so.
use serde::Deserialize;
use serde_json::{Value, json};
use solidcraft_render::material_graph::{Constant, InputValue, MaterialGraph, Node};

use crate::{Result, params::bad};

const TABLE: &str = include_str!("../../../../../assets/appearances/graphs/render-library-bindings.json");
const FORMAT: &str = "solidcraft-render-library-graph-bindings/1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    format: String,
    about: String,
    basis: String,
    surfaces: Vec<Row>,
    selene_appearances: Vec<Row>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Row {
    id: String,
    anisotropy: f64,
    rotation_rad: f64,
    note: String,
}

/// The table row for `id` (an entry not listed is isotropic: `None`).
pub(crate) fn row(command: &str, id: &str) -> Result<Option<(Row, String)>> {
    let table: Table = serde_json::from_str(TABLE).map_err(|e| bad(command, format!("graph binding table: {e}")))?;
    if table.format != FORMAT || table.about.is_empty() {
        return Err(bad(command, "unsupported graph binding table"));
    }
    let rows = if id.starts_with("SURF-") { &table.surfaces } else { &table.selene_appearances };
    let Some(row) = rows.iter().find(|r| r.id == id) else { return Ok(None) };
    let pi = std::f64::consts::PI;
    if !(0.0..=1.0).contains(&row.anisotropy) || !(-pi..=pi).contains(&row.rotation_rad) {
        return Err(bad(command, format!("graph binding for {id} is out of range")));
    }
    Ok(Some((row.clone(), table.basis)))
}

/// Give the graph's `surface` node the row's anisotropy, its rotation and the tangent of the
/// projection `uv` (added from `projection` when the graph has none). Returns the reply's
/// `anisotropy` record.
pub(crate) fn bind(command: &str, graph: &mut MaterialGraph, binding: Option<(Row, String)>, projection: impl FnOnce() -> Value) -> Result<Value> {
    let Some((row, basis)) = binding else {
        return Ok(json!({"value": 0.0, "rotation_rad": 0.0, "basis": "isotropic"}));
    };
    let record = json!({"value": row.anisotropy, "rotation_rad": row.rotation_rad, "basis": basis, "note": row.note});
    if row.anisotropy == 0.0 {
        return Ok(record);
    }
    if !graph.nodes.iter().any(|n| n.id == "uv") {
        let node: Node = serde_json::from_value(projection()).map_err(|e| bad(command, e.to_string()))?;
        graph.nodes.insert(0, node);
    }
    let surface = graph.nodes.iter_mut().find(|n| n.id == "surface").ok_or_else(|| bad(command, "graph has no surface node"))?;
    let number = |x: f64| InputValue::Constant(Constant::Number(x));
    surface.inputs.insert("specular_roughness_anisotropy".into(), number(row.anisotropy));
    if row.rotation_rad != 0.0 {
        surface.inputs.insert("specular_rotation".into(), number(row.rotation_rad));
    }
    let tangent = serde_json::from_value(json!({"from": "uv.tangent"})).map_err(|e| bad(command, e.to_string()))?;
    surface.inputs.insert("geometry_tangent".into(), tangent);
    Ok(record)
}

/// The tangent-only projection a Selene entry gets: planar in `frame`, else triplanar in object
/// space.
pub(crate) fn tangent_projection(frame: Option<([f64; 3], [f64; 3], [f64; 3])>) -> Value {
    match frame {
        Some((origin, u, v)) => {
            json!({"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}, "inputs": {"origin": origin, "u": u, "v": v}})
        }
        None => json!({"id": "uv", "kind": "texcoord.triplanar", "params": {"space": "object"}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row names an authored entry once, within the input ranges, and the basis says the
    /// amounts are estimates.
    #[test]
    fn the_binding_table_is_well_formed_and_marked_as_estimates() {
        let table: Table = serde_json::from_str(TABLE).unwrap();
        assert_eq!(table.format, FORMAT);
        assert_eq!(table.basis, "authored_estimate");
        let mut seen = std::collections::BTreeSet::new();
        for listed in table.surfaces.iter().chain(&table.selene_appearances) {
            assert!(seen.insert(listed.id.clone()), "{} twice", listed.id);
            assert!(listed.id.starts_with("SURF-") || listed.id.starts_with("SELENE-AP-"));
            assert!(listed.note.len() > 8);
            assert!(row("test", &listed.id).unwrap().is_some(), "{} in range", listed.id);
        }
        assert!(row("test", "SURF-003").unwrap().is_none(), "grain is isotropic");
    }
}
