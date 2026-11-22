# Material graph fixtures

The worked examples of `docs/material-graph.md` §3.8, as saved graphs, and `pbr-steel.json`,
the one-node graph equivalent to a `pbr` look (the library's "Steel - Satin PBR").

- `crates/viewport/src/material_graph/tests.rs` parses them and checks that every kind, port and
  connection names something in the catalogue.
- Validation, lowering and the test-only reference test against the same files. Expected values
  are analytic where possible, else from the reference.

All values are original visual estimates, not measured optical data.
