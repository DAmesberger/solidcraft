# Material graph fixtures

The worked examples of `docs/material-graph.md` §3.8, as saved graphs, and `pbr-copper.json`,
the one-node graph equivalent to a `pbr` look (the library's "Copper - Clean PBR").

- `crates/render/src/material_graph/tests.rs` parses them and checks that every kind, port and
  connection names something in the catalogue.
- Validation, lowering and both GPU back ends (viewport, path tracer) test against the same
  files. Expected values are analytic where possible, else from the test-only reference.

All values are original visual estimates, not measured optical data.
