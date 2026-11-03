//! Designs without material graphs save exactly as they did before the change existed
//! (docs/material-graph.md §4.4): every design of the CLI corpus and the Selene models, in
//! plain JSON and in a container, is written in the format and with the `format_pending` that a
//! build without `appearance-graphs` writes, byte for byte. A design with a graph takes this
//! build's number and lists the change; deleting it writes the older form again.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::format::{FORMAT, format_string, pending_names};

fn designs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut v: Vec<PathBuf> = std::fs::read_dir(root.join("apps/solidcraft-cli/tests/corpus"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "solidcraft"))
        .collect();
    v.sort();
    assert!(v.len() >= 7, "corpus designs");
    v.extend(
        ["assets/render/selene/source/SELENE-showcase.solidcraft", "assets/render/selene/models/SELENE-functional.solidcraft"].map(|p| root.join(p)),
    );
    v
}

fn document_json(container: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(container)).unwrap();
    let mut entry = zip.by_name("document.json").unwrap();
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    bytes
}

/// What a build without the `appearance-graphs` change writes for `doc`: the other pending
/// changes (or, with none, the last folded number) and the same everything else.
fn without_the_change(doc: &Document) -> Document {
    let mut d = doc.clone();
    d.format_pending.retain(|name| name != "appearance-graphs");
    if d.format_pending.is_empty() {
        d.format = format_string(FORMAT - 1);
    }
    d
}

#[test]
fn designs_without_graphs_save_as_a_build_without_the_change_does() {
    assert!(pending_names().iter().any(|n| n == "appearance-graphs"), "the change is pending in this build");
    for path in designs() {
        let doc = solidcraft_io::read_design(&std::fs::read(&path).unwrap()).unwrap();
        assert!(doc.appearances.tables.is_empty(), "{}", path.display());
        let expected = without_the_change(&doc);
        let json = solidcraft_io::write_design(&doc);
        assert_eq!(String::from_utf8(json.clone()).unwrap(), expected.to_json(), "{}: JSON", path.display());
        let saved: Value = serde_json::from_slice(&json).unwrap();
        assert!(!saved["format_pending"].as_array().into_iter().flatten().any(|n| n == "appearance-graphs"));
        // The container's document.json is the same JSON (compact, with blobs for large data).
        let container = document_json(&solidcraft_io::write_design_container(&doc).unwrap());
        let stored: Value = serde_json::from_slice(&container).unwrap();
        assert_eq!(stored["format"], Value::from(expected.format.clone()), "{}: container", path.display());
        assert_eq!(stored.get("format_pending").cloned(), saved.get("format_pending").cloned(), "{}: container", path.display());
        // Reading either back and saving again writes the same bytes.
        assert_eq!(solidcraft_io::write_design(&solidcraft_io::read_design(&json).unwrap()), json, "{}", path.display());
        let again = solidcraft_io::write_design_container(&doc).unwrap();
        assert_eq!(document_json(&again), container, "{}", path.display());
        let reopened = solidcraft_io::read_design(&again).unwrap();
        assert_eq!((reopened.format.clone(), reopened.format_pending.clone()), (format_string(FORMAT), pending_names()));
    }
}

#[test]
fn a_design_with_a_graph_takes_the_new_number_until_its_last_graph_is_deleted() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../render/tests/fixtures/material_graph/pbr-copper.json");
    let graph = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut doc = Document::new("Graphs");
    doc.appearances.tables.graphs.insert(solidcraft_doc::appearance_graph::GraphId::new("copper").unwrap(), graph);
    for saved in [solidcraft_io::write_design(&doc), document_json(&solidcraft_io::write_design_container(&doc).unwrap())] {
        let v: Value = serde_json::from_slice(&saved).unwrap();
        assert_eq!(v["format"], Value::from(format_string(FORMAT)));
        assert_eq!(v["format_pending"], Value::from(pending_names()));
    }
    let back = solidcraft_io::read_design(&solidcraft_io::write_design(&doc)).unwrap();
    assert_eq!(back.appearances.tables.graphs, doc.appearances.tables.graphs);
    let mut deleted = back.clone();
    deleted.appearances.tables.graphs.clear();
    assert_eq!(String::from_utf8(solidcraft_io::write_design(&deleted)).unwrap(), without_the_change(&deleted).to_json());
}
