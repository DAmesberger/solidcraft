//! Graph appearances captured for a GPU path job: bounds, bindings and the approximation report.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use solidcraft_viewport::appearance_graph::{AppearanceGraph, lower, validate};

const STEEL: &str = solidcraft_viewport::appearance_graph::fixtures::WORKED_EXAMPLES[4].1;
const BRUSHED: &str = solidcraft_viewport::appearance_graph::fixtures::WORKED_EXAMPLES[1].1;

fn ir(text: &str) -> Arc<Ir> {
    let graph: AppearanceGraph = serde_json::from_str(text).unwrap();
    Arc::new(lower(&validate(&graph).unwrap()).unwrap())
}

/// `pbr-steel` with one input changed to a constant.
fn steel_with(input: &str, value: serde_json::Value) -> Arc<Ir> {
    let mut graph: serde_json::Value = serde_json::from_str(STEEL).unwrap();
    graph["nodes"][0]["inputs"][input] = value;
    ir(&graph.to_string())
}

fn appearance(ir: Arc<Ir>) -> GraphAppearance {
    let textures = vec![None; ir.textures.len()];
    GraphAppearance::new(ir, &BTreeMap::new(), textures).unwrap()
}

fn binding(appearance: usize) -> GraphBinding {
    GraphBinding { body_index: 0, body_name: "body".into(), face: None, appearance, shot_coat: None, object_to_world: None }
}

fn inputs(m: &GraphAppearance) -> Vec<&'static str> {
    m.approximations().iter().map(|a| a.input).collect()
}

/// A `pbr`-equivalent graph is exact in the path tracer; inputs it has no lobe for are named.
#[test]
fn approximations_name_the_inputs_the_path_tracer_lacks() {
    assert!(appearance(ir(STEEL)).approximations().is_empty());
    assert_eq!(inputs(&appearance(steel_with("specular_roughness_anisotropy", 0.5.into()))), ["specular_roughness_anisotropy"]);
    assert_eq!(inputs(&appearance(steel_with("coat_color", serde_json::json!([1.0, 0.5, 0.5])))), ["coat_color"]);
    // At their neutral values the same inputs are exact.
    assert!(appearance(steel_with("specular_roughness_anisotropy", 0.0.into())).approximations().is_empty());
    assert!(appearance(steel_with("coat_color", serde_json::json!([1.0, 1.0, 1.0]))).approximations().is_empty());
    let brushed = appearance(ir(BRUSHED));
    assert!(brushed.approximations().iter().any(|a| a.input == "specular_roughness_anisotropy" && a.treatment == "isotropic"), "{brushed:?}");
}

/// An interface value decides: overriding a neutral default away from neutral is reported.
#[test]
fn approximations_follow_the_look_values() {
    let mut graph: serde_json::Value = serde_json::from_str(STEEL).unwrap();
    graph["interface"] = serde_json::json!([{"name": "aniso", "type": "float", "default": 0.0}]);
    graph["nodes"][0]["inputs"]["specular_roughness_anisotropy"] = serde_json::json!({"interface": "aniso"});
    let ir = ir(&graph.to_string());
    assert!(appearance(ir.clone()).approximations().is_empty());
    let values = BTreeMap::from([("aniso".to_owned(), solidcraft_viewport::appearance_graph::Constant::Number(0.7))]);
    let set = GraphAppearance::new(ir, &values, Vec::new()).unwrap();
    assert_eq!(inputs(&set), ["specular_roughness_anisotropy"]);
}

#[test]
fn bindings_are_bounded_and_unique() {
    let m = || vec![appearance(ir(STEEL))];
    assert!(GraphAppearances::new(m(), vec![binding(0)]).is_ok());
    assert!(GraphAppearances::new(m(), vec![binding(1)]).is_err(), "appearance out of range");
    assert!(GraphAppearances::new(m(), vec![binding(0), binding(0)]).is_err(), "duplicate binding");
    assert!(GraphAppearances::new(m(), vec![binding(0), GraphBinding { face: Some(3), ..binding(0) }]).is_ok(), "a face overrides its body");
    assert!(GraphAppearances::new(m(), Vec::new()).is_err());
    assert!(GraphAppearances::new(Vec::new(), vec![binding(0)]).is_err());
    let stage = GraphBinding { body_name: "__render_stage__/floor".into(), ..binding(0) };
    assert!(GraphAppearances::new(m(), vec![stage]).is_err(), "stage bodies keep interior.z for their flag");
    let singular = GraphBinding { object_to_world: Some([[1.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]), ..binding(0) };
    assert!(GraphAppearances::new(m(), vec![singular]).is_err());
    let huge = GraphBinding { object_to_world: Some([[1.0, 0.0, 0.0, 1e9], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]), ..binding(0) };
    assert!(GraphAppearances::new(m(), vec![huge]).is_err());
    let nan = GraphBinding { object_to_world: Some([[f64::NAN, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]), ..binding(0) };
    assert!(GraphAppearances::new(m(), vec![nan]).is_err());
}

#[test]
fn a_face_binding_takes_precedence_over_its_body() {
    let graphs =
        GraphAppearances::new(vec![appearance(ir(STEEL)), appearance(ir(BRUSHED))], vec![binding(0), GraphBinding { face: Some(2), ..binding(1) }])
            .unwrap();
    assert_eq!(graphs.binding_for(0, 2).map(|b| b.appearance), Some(1));
    assert_eq!(graphs.binding_for(0, 5).map(|b| b.appearance), Some(0));
    assert!(graphs.binding_for(1, 0).is_none());
    assert_eq!(graphs.shapes().len(), 2);
}

/// The request serialises the summary only, never textures or words, and never deserialises it.
#[test]
fn the_summary_reports_approximations_per_binding() {
    let graphs =
        GraphAppearances::new(vec![appearance(ir(STEEL)), appearance(ir(BRUSHED))], vec![binding(0), GraphBinding { face: Some(2), ..binding(1) }])
            .unwrap();
    let value = serde_json::to_value(&graphs).unwrap();
    assert_eq!(value["appearances"], 2);
    assert_eq!(value["bindings"], 2);
    let reported = value["approximations"].as_array().unwrap();
    assert_eq!(reported.len(), 1, "only the brushed face is approximated: {value}");
    assert_eq!(reported[0]["face"], 2);
    assert!(reported[0]["inputs"].as_array().unwrap().iter().any(|i| i["input"] == "specular_roughness_anisotropy"));
    let mut request: serde_json::Value = serde_json::json!({"camera": serde_json::to_value(solidcraft_viewport::Camera::default()).unwrap(),
        "width": 4, "height": 4, "background": [0, 0, 0], "transparent_background": false});
    assert!(serde_json::from_value::<super::super::ImageRequest>(request.clone()).is_ok());
    request["graphs"] = value;
    assert!(serde_json::from_value::<super::super::ImageRequest>(request).is_err(), "graphs are captured, never read from a request");
}

#[test]
fn material_values_are_checked() {
    let mut graph: serde_json::Value = serde_json::from_str(STEEL).unwrap();
    graph["interface"] = serde_json::json!([{"name": "rough", "type": "float", "default": 0.5}]);
    graph["nodes"][0]["inputs"]["specular_roughness"] = serde_json::json!({"interface": "rough"});
    let ir = ir(&graph.to_string());
    let bad = BTreeMap::from([("rough".to_owned(), solidcraft_viewport::appearance_graph::Constant::Number(7.0))]);
    assert!(GraphAppearance::new(ir.clone(), &bad, Vec::new()).is_err(), "out of the surface input's range");
    assert!(GraphAppearance::new(ir, &BTreeMap::new(), vec![None]).is_err(), "one texture entry per binding");
}

/// A constant see-through appearance is glass absorbing to its colour at its depth; a weight below 1
/// is reported as full glass; an opaque one has none.
#[test]
fn see_through_materials_become_glass() {
    assert!(appearance(ir(STEEL)).see_through().is_none());
    let mut graph: serde_json::Value = serde_json::from_str(STEEL).unwrap();
    let inputs = &mut graph["nodes"][0]["inputs"];
    inputs["base_metalness"] = 0.0.into();
    inputs["transmission_weight"] = 1.0.into();
    inputs["transmission_color"] = serde_json::json!([0.5, 1.0, 0.25]);
    inputs["transmission_depth_mm"] = 2.0.into();
    let glass = appearance(ir(&graph.to_string()));
    let t = glass.see_through().unwrap();
    assert!(t.thin_wall_thickness_mm().is_none());
    let a = t.extinction();
    for (got, want) in [a.x, a.y, a.z].into_iter().zip([0.5f64.ln() / -2.0, 0.0, 0.25f64.ln() / -2.0]) {
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }
    assert!(glass.approximations().is_empty(), "weight 1 is exact");
    graph["nodes"][0]["inputs"]["transmission_weight"] = 0.5.into();
    let half = appearance(ir(&graph.to_string()));
    assert!(half.see_through().is_some());
    assert_eq!(half.approximations(), [Approximation { input: "transmission_weight", treatment: "full_glass" }]);
}
