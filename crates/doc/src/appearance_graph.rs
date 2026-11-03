//! The design-level records of material graphs (docs/material-graph.md §4.1): a table of named
//! graphs that looks reference by id (decision O1), and the texture assets they read.
//!
//! Contract status: the types are fixed; track B wires them into `Appearances` (`graphs`,
//! `textures`, both omitted when empty) and `Look` (`graph`, `values`), with the format change
//! `format/pending/appearance-graphs.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use solidcraft_render::material_graph::limits;
pub use solidcraft_render::material_graph::{Constant, MaterialGraph};

/// A graph's id in the design's table: 1–64 bytes of `[a-z0-9_.-]`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GraphId(String);

impl GraphId {
    pub fn new(id: &str) -> crate::Result<Self> {
        let ok = !id.is_empty()
            && id.len() <= limits::ID_BYTES
            && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'));
        if ok {
            Ok(GraphId(id.to_string()))
        } else {
            Err(crate::DocError::Invalid(format!("a graph id is 1–{} characters of a–z, 0–9, _ . -", limits::ID_BYTES)))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GraphId {
    type Error = crate::DocError;
    fn try_from(id: String) -> crate::Result<Self> {
        GraphId::new(&id)
    }
}

impl From<GraphId> for String {
    fn from(id: GraphId) -> String {
        id.0
    }
}

/// A texture asset, keyed by the SHA-256 of its exact encoded bytes. In a container its bytes
/// are `image_blob` (`blobs/<sha256>.png|jpeg`, the canvas blob kinds); in plain JSON, `data`
/// holds base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureAsset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// `png` or `jpeg`.
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_blob: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
}

/// The design's graphs and textures, as `Appearances` will hold them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphTables {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub graphs: BTreeMap<GraphId, MaterialGraph>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub textures: BTreeMap<String, TextureAsset>,
}

impl GraphTables {
    pub fn is_empty(&self) -> bool {
        self.graphs.is_empty() && self.textures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_ids_are_bounded_and_plain() {
        assert!(GraphId::new("brushed-nickel-silver").is_ok());
        assert!(GraphId::new("pbr.1_a").is_ok());
        for bad in ["", "Upper", "with space", "../x", &"a".repeat(65)] {
            assert!(GraphId::new(bad).is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<GraphId>("\"Bad Id\"").is_err());
    }

    #[test]
    fn empty_tables_serialise_to_nothing() {
        assert_eq!(serde_json::to_value(GraphTables::default()).ok(), Some(serde_json::json!({})));
    }
}
