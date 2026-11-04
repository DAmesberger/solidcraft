//! Looks can name a material graph (`appearances.graphs`, a table by id; `Look::graph` and
//! `Look::values`) and graphs can use texture assets (`appearances.textures`, keyed by the
//! SHA-256 of their bytes: container blobs, or base64 in a plain JSON design). Both tables, and
//! a look's `graph` and `values`, are omitted when empty. docs/material-graph.md §4.
//!
//! An older design gets none: its scalar and `pbr` looks are unchanged and are never migrated
//! (decision O9), so there is no `step`. A design without graphs is still written in the older
//! format, byte for byte, because `Part::used` reads [`USED`] (decision O8); the last
//! release refuses a design that has graphs (the newer format number).

/// Whether a design's saved JSON uses this change: it has a material graph.
pub const USED: fn(&serde_json::Value) -> bool = crate::appearance_graph::graphs_used;
