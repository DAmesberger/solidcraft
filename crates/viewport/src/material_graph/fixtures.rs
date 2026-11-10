//! The worked examples of docs/material-graph.md §3.8 and the `pbr`-equivalent graph, as JSON
//! (`tests/fixtures/material_graph/`): test data for this crate and, through the
//! `test-reference` feature, for the GPU back end's tests.

/// Each example's name and graph.
pub const WORKED_EXAMPLES: &[(&str, &str)] = &[
    ("black-epoxy-clear-coat", include_str!("../../tests/fixtures/material_graph/black-epoxy-clear-coat.json")),
    ("brushed-nickel-silver", include_str!("../../tests/fixtures/material_graph/brushed-nickel-silver.json")),
    ("electrolytic-sleeve", include_str!("../../tests/fixtures/material_graph/electrolytic-sleeve.json")),
    ("laser-etched-nickel-silver", include_str!("../../tests/fixtures/material_graph/laser-etched-nickel-silver.json")),
    ("pbr-copper", include_str!("../../tests/fixtures/material_graph/pbr-copper.json")),
    ("solder-mask-over-copper", include_str!("../../tests/fixtures/material_graph/solder-mask-over-copper.json")),
];

/// The worked example called `name`, if there is one.
pub fn worked_example(name: &str) -> Option<&'static str> {
    WORKED_EXAMPLES.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}
