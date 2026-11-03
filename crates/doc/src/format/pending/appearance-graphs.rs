//! Looks can name a material graph (`appearances.graphs`, a table by id; `Look::graph` and
//! `Look::values`) and graphs can use texture assets (`appearances.textures`, container blobs
//! or base64 in plain JSON). docs/material-graph.md §4.
//!
//! An older design gets none: its scalar and `pbr` looks are unchanged and are never migrated
//! (decision O9), so there is no `step`. The last release refuses a design that has graphs (the
//! newer format number). Once `Part::used` lands (track K), a design without graphs is written
//! in the older format, byte for byte (decision O8).
//!
//! Contract status: the record types are in `crate::appearance_graph`; until track B wires them
//! into `Appearances` and `Look`, nothing is saved by this change.
