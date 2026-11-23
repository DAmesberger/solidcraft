//! Browse a single library and copy a validated graph and its images into the design.
use std::collections::BTreeSet;

use super::{changed_reply, commit, new_id, parse, reply};
use crate::schema::{
    TypedContract,
    appearance_graph::{
        GraphLibraryImportParams, GraphLibraryItem, GraphLibraryListParams, GraphLibraryListResult, GraphLibraryProvenance, GraphLibraryRefusal,
    },
};
use crate::{Result, Session, params::bad};
use serde_json::Value;
use solidcraft_io::library::{self, Library, Place, Status};

pub(super) const LIST: TypedContract<GraphLibraryListParams> = TypedContract::new();
pub(super) const IMPORT: TypedContract<GraphLibraryImportParams> = TypedContract::new();

fn with_library<T>(cmd: &str, folder: Option<String>, read: impl FnOnce(&Library, &str) -> Result<T>) -> Result<T> {
    let place = match folder {
        Some(path) => {
            if path.trim().is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                return Err(bad(cmd, "invalid library folder path"));
            }
            solidcraft_io::vfs::check(&path).map_err(|e| bad(cmd, e.to_string()))?;
            Place::Library(path)
        }
        None => Place::BuiltIn,
    };
    let scan = library::scan(&place);
    if let Status::Unavailable(reason) = scan.status() {
        return Err(bad(cmd, reason.to_string()));
    }
    let [library] = scan.libraries() else {
        return Err(bad(cmd, "choose one library folder"));
    };
    if let Status::Unavailable(reason) = library.status() {
        return Err(bad(cmd, reason.to_string()));
    }
    let id = library.id().ok_or_else(|| bad(cmd, "missing library identifier"))?;
    read(library, id)
}

pub(super) fn list(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.library.list";
    let params: GraphLibraryListParams = parse(cmd, p)?;
    with_library(cmd, params.folder, |library, id| {
        let mut entries = Vec::new();
        let mut refused = Vec::new();
        for listed in library.graphs() {
            match listed.entry() {
                Ok(entry) => entries.push(GraphLibraryItem {
                    id: entry.id.clone(),
                    name: entry.graph.name.clone(),
                    revision: entry.revision.clone(),
                    provenance: GraphLibraryProvenance {
                        license: entry.provenance.license.clone(),
                        author: entry.provenance.author.clone(),
                        source_url: entry.provenance.source_url.clone(),
                    },
                }),
                Err(reason) => refused.push(GraphLibraryRefusal { label: listed.label().into(), reason: reason.to_string() }),
            }
        }
        reply(cmd, GraphLibraryListResult { library: id.into(), entries, refused, param_warnings: Vec::new() })
    })
}

pub(super) fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "appearance.graph.library.import";
    let params: GraphLibraryImportParams = parse(cmd, p)?;
    with_library(cmd, params.folder, |library, library_id| {
        let entry = library.graph(&params.entry).ok_or_else(|| bad(cmd, "no available graph with that entry id in this library"))?;
        if params.revision.as_ref().is_some_and(|revision| revision != &entry.revision) {
            return Err(bad(cmd, "library entry changed since it was listed"));
        }
        let id = new_id(cmd, s, params.id.as_deref(), &entry.graph.name)?;
        let mut doc = (*s.doc).clone();
        let tables = &mut doc.appearances.tables;
        let mut added_textures = false;
        for (key, texture) in &entry.textures {
            if let std::collections::btree_map::Entry::Vacant(slot) = tables.textures.entry(key.clone()) {
                slot.insert(texture.clone());
                added_textures = true;
            }
        }
        tables.graphs.insert(id.clone(), entry.design_graph(library_id));
        commit(cmd, s, doc, &BTreeSet::from([id.clone()]), added_textures)?;
        changed_reply(cmd, s, &id)
    })
}
