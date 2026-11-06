//! The oldest format that holds a design (`Part::used`, `Family::written_for`,
//! docs/material-graph.md §4.4): a format whose changes all say which files use them, and none of
//! which a design uses, is skipped when it is written; a change that says nothing still forces
//! its number; and reading a file written that way still runs exactly the steps it is missing.

use serde_json::{Value, json};

use super::*;

fn trace(v: &mut Value, s: &str) -> Result<()> {
    let o = v.as_object_mut().ok_or_else(|| DocError::Invalid("no object".into()))?;
    o.entry("trace").or_insert_with(|| Value::Array(Vec::new())).as_array_mut().into_iter().for_each(|t| t.push(Value::from(s)));
    Ok(())
}
fn step_a(v: &mut Value) -> Result<()> {
    trace(v, "a")
}
fn step_c(v: &mut Value) -> Result<()> {
    trace(v, "c")
}
fn step_d(v: &mut Value) -> Result<()> {
    trace(v, "d")
}
/// A file uses change `x` when it has a top-level `x`.
fn uses_a(v: &Value) -> bool {
    v.get("a").is_some()
}
fn uses_c(v: &Value) -> bool {
    v.get("c").is_some()
}
fn uses_d(v: &Value) -> bool {
    v.get("d").is_some()
}
fn uses_e(v: &Value) -> bool {
    v.get("e").is_some()
}

/// 23: `a` (says), 24: `b` (says nothing), 25: `c` (says), pending: `d` (says), `e` (says).
const FOLDED_23_25: &[(u32, &[Part])] = &[
    (23, &[Part { name: "a", step: Some(step_a), used: Some(uses_a) }]),
    (24, &[Part { name: "b", step: None, used: None }]),
    (25, &[Part { name: "c", step: Some(step_c), used: Some(uses_c) }]),
];
const SAYING: &[Part] = &[Part { name: "d", step: Some(step_d), used: Some(uses_d) }, Part { name: "e", step: None, used: Some(uses_e) }];
/// As [`SAYING`], with a change that says nothing between them.
const ONE_SILENT: &[Part] = &[
    Part { name: "d", step: Some(step_d), used: Some(uses_d) },
    Part { name: "silent", step: None, used: None },
    Part { name: "e", step: None, used: Some(uses_e) },
];

fn formats(pending: &'static [Part]) -> Formats<'static> {
    Formats { hand: &JSON_STEPS, folded: FOLDED_23_25, pending }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A file that uses no pending change steps back past them and past every older format whose
/// changes say and are unused, and stops at the first change that says nothing.
#[test]
fn a_file_is_written_in_the_oldest_format_that_holds_it() {
    let f = formats(SAYING);
    assert_eq!(f.current(), 26);
    assert_eq!(f.written_for(&json!({})), (24, vec![]), "26 and 25 are unused; 24's change says nothing");
    assert_eq!(f.written_for(&json!({"c": 1})), (25, vec![]), "25 is used");
    assert_eq!(f.written_for(&json!({"a": 1})), (24, vec![]), "a used change behind one that says nothing is never reached");
    assert_eq!(f.written_for(&json!({"e": 1})), (26, names(&["e"])), "only the used pending change is listed");
    assert_eq!(f.written_for(&json!({"d": 1, "e": 1, "c": 1})), (26, names(&["d", "e"])), "in file order");
}

/// A pending change that says nothing is used by every file: the build's number, and its name
/// in the list, as before the rule.
#[test]
fn a_change_that_says_nothing_still_forces_its_format() {
    let f = formats(ONE_SILENT);
    assert_eq!(f.written_for(&json!({})), (26, names(&["silent"])));
    assert_eq!(f.written_for(&json!({"e": 1})), (26, names(&["silent", "e"])));
    // With nothing pending, the last folded format is the build's; one that says steps back.
    let folded = formats(&[]);
    assert_eq!(folded.current(), 25);
    assert_eq!(folded.written_for(&json!({})), (24, vec![]));
    // Formats made only of changes that say nothing are always written.
    let silent: &[(u32, &[Part])] = &[(23, &[Part { name: "s", step: None, used: None }])];
    assert_eq!(Formats { hand: &JSON_STEPS, folded: silent, pending: &[] }.written_for(&json!({})), (23, vec![]));
}

/// The walk never steps past the formats before changes were files, nor past an empty format.
#[test]
fn the_walk_stops_at_the_hand_written_formats() {
    let saying: &[(u32, &[Part])] = &[(23, &[Part { name: "a", step: Some(step_a), used: Some(uses_a) }])];
    let f = Formats { hand: &JSON_STEPS, folded: saying, pending: SAYING };
    assert_eq!(f.written_for(&json!({})), (HAND_WRITTEN, vec![]));
    let empty: &[(u32, &[Part])] = &[(23, &[]), (24, &[Part { name: "a", step: None, used: Some(uses_a) }])];
    assert_eq!(Formats { hand: &JSON_STEPS, folded: empty, pending: &[] }.written_for(&json!({})), (23, vec![]));
    // Drawings (no hand-written formats) stop at their first.
    let drawing: &[(u32, &[Part])] = &[(2, &[Part { name: "a", step: None, used: Some(uses_a) }])];
    assert_eq!(Formats { hand: &[], folded: drawing, pending: SAYING }.written_for(&json!({})), (1, vec![]));
}

/// A file written in an older format, or listing only the pending changes it uses, opens with
/// exactly the steps it is missing: those of the formats it skipped and of the unlisted pending
/// changes, never those of a listed one.
#[test]
fn reading_a_file_written_that_way_runs_the_steps_it_is_missing() {
    let f = formats(SAYING);
    let open = |file: Value| {
        let mut v = file.clone();
        let (version, applied) = (version_of(v["format"].as_str().unwrap_or_default()).unwrap(), v.get("format_pending").cloned());
        let applied: Option<Vec<String>> = applied.map(|a| serde_json::from_value(a).unwrap());
        f.upgrade("design", &mut v, version, applied.as_deref()).map(|()| v["trace"].clone())
    };
    // Written as 24: 25's step and every pending step run.
    assert_eq!(open(json!({"format": "solidcraft/24"})).unwrap(), json!(["c", "d"]));
    // Written as 25: only the pending ones.
    assert_eq!(open(json!({"format": "solidcraft/25", "c": 1})).unwrap(), json!(["d"]));
    // Written as 26 listing `e`: `d` is not listed, so its step runs; `e`'s never would.
    assert_eq!(open(json!({"format": "solidcraft/26", "format_pending": ["e"], "e": 1})).unwrap(), json!(["d"]));
    assert_eq!(open(json!({"format": "solidcraft/26", "format_pending": ["d", "e"], "d": 1, "e": 1})).unwrap(), Value::Null);
    // What the writer chose always reads back, and a second write chooses the same.
    for file in [json!({}), json!({"c": 1}), json!({"e": 1}), json!({"d": 1, "a": 1})] {
        let (n, listed) = f.written_for(&file);
        let mut written = file.clone();
        written["format"] = format_string(n).into();
        if !listed.is_empty() {
            written["format_pending"] = listed.clone().into();
        }
        assert!(f.newer(n, (!listed.is_empty()).then_some(listed.as_slice())).is_none());
        assert!(open(written).is_ok());
    }
}

/// This build's designs: a design without graphs never lists `appearance-graphs`, nor takes a
/// number for it; one with graphs takes this build's number and lists it; deleting the last graph
/// writes the older form again. Both writers (plain JSON and a container's `document.json`) agree
/// because both call [`write_oldest_format`].
#[test]
fn this_build_writes_appearance_graphs_only_for_designs_with_graphs() {
    let others: Vec<&Part> = PENDING.iter().filter(|part| part.name != "appearance-graphs").collect();
    assert!(PENDING.iter().any(|part| part.name == "appearance-graphs" && part.used.is_some()));
    // What a build without the change writes: the other pending changes, or the last folded number.
    let without = |doc: &Document| {
        let mut v = serde_json::to_value(doc).unwrap();
        if others.is_empty() {
            v["format"] = format_string(FORMAT - 1).into();
            v.as_object_mut().unwrap().remove("format_pending");
        } else {
            v["format_pending"] = others.iter().map(|p| p.name).collect::<Vec<_>>().into();
        }
        v
    };
    let plain = Document::new("Plain");
    let mut written = serde_json::to_value(&plain).unwrap();
    write_oldest_format(&mut written);
    // The other pending changes say nothing today (`join-to-body-apart`), so the number is this
    // build's; once they say, or fold, it is theirs.
    if others.iter().all(|p| p.used.is_some()) {
        assert_ne!(written["format"], format_string(FORMAT));
    }
    assert_eq!(written, without(&plain));
    assert_eq!(serde_json::to_value(&*plain.in_written_format()).unwrap(), written);

    let mut graphs = plain.clone();
    graphs.appearances.tables.graphs.insert(
        crate::appearance_graph::GraphId::new("copper").unwrap(),
        serde_json::from_str(include_str!("../../render/tests/fixtures/material_graph/pbr-copper.json")).unwrap(),
    );
    let mut written = serde_json::to_value(&graphs).unwrap();
    write_oldest_format(&mut written);
    assert_eq!(written["format"], format_string(FORMAT));
    assert_eq!(written["format_pending"], json!(pending_names()), "every pending change this build has, in file order");
    assert!(matches!(graphs.in_written_format(), std::borrow::Cow::Borrowed(_)), "already this build's format");

    graphs.appearances.tables.graphs.clear();
    assert_eq!(serde_json::to_value(&*graphs.in_written_format()).unwrap(), without(&plain));
}

/// A design whose format fields a tool or test set itself is written as they are.
#[test]
fn a_design_with_its_own_format_fields_is_written_as_it_is() {
    let mut older = Document::new("Older");
    older.format = format_string(30);
    older.format_pending.clear();
    let v = serde_json::to_value(&older).unwrap();
    let mut written = v.clone();
    write_oldest_format(&mut written);
    assert_eq!(written, v);
    assert!(matches!(older.in_written_format(), std::borrow::Cow::Borrowed(_)));
    let mut listed = Document::new("Listed");
    listed.format_pending.push("experimental-card".into());
    assert!(matches!(listed.in_written_format(), std::borrow::Cow::Borrowed(_)));
    // A damaged list in the JSON is left alone too.
    let mut damaged = json!({"format": format_string(FORMAT), "format_pending": 7});
    write_oldest_format(&mut damaged);
    assert_eq!(damaged, json!({"format": format_string(FORMAT), "format_pending": 7}));
}

/// A design written in the older form opens in this build as it was: the steps it is missing
/// leave a design without graphs unchanged, and a second save writes the same bytes.
#[test]
fn a_design_written_in_the_older_form_reads_back_unchanged() {
    let mut doc = Document::new("Back");
    doc.params.push(crate::Param { name: "w".into(), expr: "10 mm".into(), unit: "mm".into(), comment: String::new(), model: false });
    let text = doc.in_written_format().to_json();
    let back = Document::from_json(&text).unwrap();
    assert_eq!((back.format.as_str(), back.format_pending.as_slice()), (format_string(FORMAT).as_str(), pending_names().as_slice()));
    assert_eq!(back.params, doc.params);
    assert_eq!(back.in_written_format().to_json(), text);
}
