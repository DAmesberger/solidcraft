//! Looks can name an appearance graph: `appearances.graphs` is a table of graphs by id, and a look
//! names one with `graph` and sets its interface inputs with `values`. Graphs can read texture
//! assets: `appearances.textures`, keyed by the SHA-256 of their encoded bytes (blobs in a
//! container, base64 in a plain JSON design). Both tables, and a look's `graph` and `values`, are
//! omitted when empty, so a design without graphs saves the same JSON as before.
//!
//! An older design gets none of this: its scalar and `pbr` looks are unchanged and are not
//! rewritten, so there is no step. The last release warns about the newer format number, shows
//! each look's `color`, and keeps the tables it cannot read when it saves.
