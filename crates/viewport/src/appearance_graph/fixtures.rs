//! The worked examples of docs/appearance-graph.md §3.8 and the `pbr`-equivalent graph, as JSON
//! (`tests/fixtures/appearance_graph/`): test data for this crate and, through the
//! `test-reference` feature, for the GPU back end's tests.

/// Each example's name and graph.
pub const WORKED_EXAMPLES: &[(&str, &str)] = &[
    ("gloss-black-clear-coat", include_str!("../../tests/fixtures/appearance_graph/gloss-black-clear-coat.json")),
    ("brushed-stainless", include_str!("../../tests/fixtures/appearance_graph/brushed-stainless.json")),
    ("printed-label", include_str!("../../tests/fixtures/appearance_graph/printed-label.json")),
    ("laser-etched-stainless", include_str!("../../tests/fixtures/appearance_graph/laser-etched-stainless.json")),
    ("pbr-steel", include_str!("../../tests/fixtures/appearance_graph/pbr-steel.json")),
    ("candy-coat-over-aluminium", include_str!("../../tests/fixtures/appearance_graph/candy-coat-over-aluminium.json")),
];

/// The worked example called `name`, if there is one.
pub fn worked_example(name: &str) -> Option<&'static str> {
    WORKED_EXAMPLES.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}
