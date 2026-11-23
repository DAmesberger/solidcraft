//! Graph records live in library folders. Content revisions include provenance and the graph,
//! whose texture keys already identify the exact encoded bytes. No paths come from the graph.
use super::{BUILT_IN_ID, Cap, Kind, Limits, Listed, Provenance, Reason, decode, decode_entry, file_label, identifier, listing_reason, read_capped};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use solidcraft_doc::appearance_graph::{
    AppearanceGraph, GraphId, GraphRevision, GraphSource, TextureAsset, Textures, check_graph_in, check_tables, graph_textures, is_texture_key,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "../../../assets/appearances/graphs/library.rs"]
pub(super) mod starters;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case", deny_unknown_fields)]
enum Definition<G> {
    Graph(G),
}

fn serialize_graph<S: Serializer>(graph: &AppearanceGraph, serializer: S) -> Result<S::Ok, S::Error> {
    Definition::Graph(graph).serialize(serializer)
}

fn deserialize_graph<'de, D: Deserializer<'de>>(deserializer: D) -> Result<AppearanceGraph, D::Error> {
    let Definition::Graph(graph) = Definition::deserialize(deserializer)?;
    Ok(graph)
}

/// A folder's graph and its checked image assets. Only its descriptor is serialized into
/// `graphs/<label>.json`. The encoded image files are named `textures/<sha256>.png|jpeg`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEntry {
    pub id: String,
    pub revision: String,
    pub provenance: Provenance,
    #[serde(rename = "definition", serialize_with = "serialize_graph", deserialize_with = "deserialize_graph")]
    pub graph: AppearanceGraph,
    #[serde(skip)]
    pub textures: Textures,
}
impl GraphEntry {
    /// The same identity-independent digest convention as a scalar library entry.
    pub fn content_revision(&self) -> crate::Result<String> {
        self.provenance.validate()?;
        let bytes = serde_json::to_vec(&(&self.provenance, Definition::Graph(&self.graph))).map_err(|e| crate::IoError::Invalid(e.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Make a document-owned copy and record its immediate source. The folder reader has
    /// already verified the qualified identity and the full content digest.
    pub fn design_graph(&self, library: &str) -> AppearanceGraph {
        let mut graph = self.graph.clone();
        let local = self.id.split_once('/').map_or(self.id.as_str(), |(_, local)| local);
        graph.source = Some(GraphSource { library: library.into(), entry: local.into(), revision: GraphRevision::Digest(self.revision.clone()) });
        graph
    }
}

fn invalid(e: impl std::fmt::Display) -> Reason {
    Reason::Invalid(e.to_string())
}

pub(super) fn built_in(provenance: impl Fn(&str) -> Provenance) -> Vec<Listed<GraphEntry>> {
    starters::GRAPHS
        .iter()
        .map(|&(slug, source)| {
            let make = || {
                let local = format!("graph-{slug}");
                let mut entry = GraphEntry {
                    id: format!("{BUILT_IN_ID}/{local}"),
                    revision: String::new(),
                    provenance: provenance(&format!("assets/appearances/graphs/{slug}.json")),
                    graph: decode(source.as_bytes())?,
                    textures: Textures::new(),
                };
                check_graph_in(&GraphId::new(&local).map_err(invalid)?, &entry.graph, &entry.textures).map_err(invalid)?;
                entry.revision = entry.content_revision().map_err(invalid)?;
                Ok(entry)
            };
            let state = make();
            let label = state.as_ref().map_or_else(|_| slug.to_string(), |entry| entry.graph.name.clone());
            Listed { label, kind: Kind::Graph, state }
        })
        .collect()
}

/// File names come only from a bounded listing of regular files in a real textures subfolder.
/// A failure in that listing is relevant only to graphs that actually name a texture.
pub(super) type TextureFiles = Result<BTreeMap<String, String>, Reason>;

pub(super) fn texture_files(folders: &[String], limits: &Limits) -> TextureFiles {
    let Some(dir) = folders.iter().find(|p| file_label(p) == "textures") else { return Ok(BTreeMap::new()) };
    let files = crate::vfs::files_in(dir, limits.entries_per_kind).map_err(|e| listing_reason(&e, Cap::TextureFiles, limits.entries_per_kind))?;
    Ok(files.into_iter().map(|p| (file_label(&p), p)).collect())
}

pub(super) fn read(bytes: &[u8], library: &str, files: &TextureFiles, used: &mut u64, limits: &Limits) -> Result<GraphEntry, Reason> {
    let mut entry = decode_entry::<GraphEntry, crate::catalog::Entry>(bytes)?;
    let (pack, local) = entry.id.split_once('/').ok_or(Reason::ForeignId)?;
    if pack != library {
        return Err(Reason::ForeignId);
    }
    if !identifier(local) || entry.content_revision().map_err(invalid)? != entry.revision {
        return Err(invalid("invalid graph entry identity or content revision"));
    }
    let graph_id = GraphId::new(local).map_err(invalid)?;
    for key in graph_textures(&entry.graph).collect::<BTreeSet<_>>() {
        if !is_texture_key(key) {
            return Err(invalid("a texture key must be a lowercase SHA-256 digest"));
        }
        let files = files.as_ref().map_err(Clone::clone)?;
        let png = files.get(&format!("{key}.png"));
        let jpeg = files.get(&format!("{key}.jpeg"));
        let (path, format) = match (png, jpeg) {
            (Some(path), None) => (path, "png"),
            (None, Some(path)) => (path, "jpeg"),
            (None, None) => return Err(invalid(format!("texture {key} is missing"))),
            (Some(_), Some(_)) => return Err(invalid(format!("texture {key} has two files"))),
        };
        let remaining = limits.library_bytes.saturating_sub(*used);
        // Charge each copy retained in a graph bundle, even for two graphs sharing a file.
        let bytes = read_capped(path, Cap::LibraryBytes, remaining).map_err(|reason| match reason {
            Reason::OverLimit { .. } => Reason::OverLimit { cap: Cap::LibraryBytes, limit: limits.library_bytes },
            other => other,
        })?;
        *used = used.saturating_add(bytes.len() as u64);
        let (actual, texture) = TextureAsset::from_bytes(&file_label(path), &bytes).map_err(invalid)?;
        if actual != key || texture.format != format {
            return Err(invalid(format!("texture {key} does not match its file name")));
        }
        entry.textures.insert(actual, texture);
    }
    check_tables(&[(graph_id, entry.graph.clone())].into(), &entry.textures).map_err(invalid)?;
    Ok(entry)
}
