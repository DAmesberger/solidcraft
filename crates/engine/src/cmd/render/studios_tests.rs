use super::*;
use crate::Session;
use serde_json::json;

fn run(s: &mut Session, command: &str, params: Value) -> Value {
    s.execute(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"))
}

/// The built-in studios ship as data: every catalog entry validates, ids are unique, the 25 native
/// compositions are there, and the Realistic studios (Product, Soft, Contrast) have rigs.
#[test]
fn the_built_in_studios_are_data() {
    let all = builtin();
    let mut ids = std::collections::BTreeSet::new();
    for studio in all {
        studio.validate().unwrap_or_else(|e| panic!("{}: {e}", studio.id));
        assert!(ids.insert(studio.id.clone()), "{} is unique", studio.id);
    }
    assert_eq!(all.iter().filter(|s| s.path.is_some()).count(), 25);
    let rigs: Vec<&str> = all.iter().filter(|s| s.realistic.is_some()).map(|s| s.name.as_str()).collect();
    assert_eq!(rigs, ["Product", "Soft", "Contrast"]);
    assert_eq!(StudioLibrary::default().realistic().map(|s| s.name.as_str()), Some("Product"), "Product is the default");
}

/// A user studio is duplicated from a built-in one, edited, chosen as the Realistic default,
/// exported and imported again, and deleted; built-in studios refuse edits and deletion. None of
/// it touches the design or its history.
#[test]
fn user_studios_are_managed_without_touching_the_design() {
    let mut s = Session::default();
    let before = (s.doc.to_json(), s.undo.len());
    let listed = run(&mut s, "render.studio.list", json!({"kind": "realistic"}));
    assert_eq!(listed["realistic_default"], DEFAULT_REALISTIC);
    let contrast = listed["studios"].as_array().unwrap().iter().find(|st| st["name"] == "Contrast").unwrap()["id"].as_str().unwrap().to_owned();
    let copy = run(&mut s, "render.studio.duplicate", json!({"id": contrast, "name": "Board studio"}));
    let id = copy["studio"]["id"].as_str().unwrap().to_owned();
    assert_eq!(copy["studio"]["source"], contrast.as_str());
    let mut rig = copy["studio"]["realistic"].clone();
    rig["lights"][0]["radiance"] = json!([100.0, 100.0, 100.0]);
    let edited = run(&mut s, "render.studio.edit", json!({"id": id, "realistic": rig}));
    assert_eq!(edited["studio"]["realistic"]["lights"][0]["radiance"], json!([100.0, 100.0, 100.0]));
    // Refused edits change nothing.
    let mut bad_rig = rig.clone();
    bad_rig["lights"][0]["radiance"] = json!([-1.0, 0.0, 0.0]);
    assert!(s.execute("render.studio.edit", &json!({"id": id, "realistic": bad_rig})).is_err());
    assert_eq!(run(&mut s, "render.studio.get", json!({"id": id}))["studio"], edited["studio"]);
    assert!(s.execute("render.studio.edit", &json!({"id": contrast, "name": "Mine"})).is_err(), "built-in studios are read-only");
    assert!(s.execute("render.studio.delete", &json!({"id": contrast})).is_err());
    // The default names the user studio, and falls back to Product when it is deleted.
    assert_eq!(run(&mut s, "render.studio.default", json!({"id": id}))["id"], id.as_str());
    assert_eq!(s.studios.realistic().map(|st| st.id.as_str()), Some(id.as_str()));
    // Export, delete, import.
    let path = std::env::temp_dir().join(format!("studios-{}.json", std::process::id()));
    let path = path.to_string_lossy().to_string();
    let _ = std::fs::remove_file(&path);
    assert_eq!(run(&mut s, "render.studio.export", json!({"path": path, "ids": [id]}))["count"], 1);
    assert_eq!(run(&mut s, "render.studio.delete", json!({"id": id}))["realistic_default"], DEFAULT_REALISTIC);
    assert!(s.studios.user.is_empty());
    assert_eq!(run(&mut s, "render.studio.import", json!({"path": path}))["imported"], json!([id]));
    assert!(s.execute("render.studio.import", &json!({"path": path})).is_err(), "an existing id needs replace");
    run(&mut s, "render.studio.import", json!({"path": path, "replace": true}));
    let _ = std::fs::remove_file(&path);
    assert_eq!(s.studios.user.len(), 1);
    assert_eq!((s.doc.to_json(), s.undo.len()), before);
}

/// Hostile imports are refused whole: a wrong schema, a built-in id, an invalid record.
#[test]
fn hostile_imports_add_nothing() {
    let mut s = Session::default();
    let product = builtin().iter().find(|st| st.name == "Product").cloned().unwrap();
    let mut mine = product.clone();
    mine.id = "MINE".into();
    let mut broken = product.clone();
    broken.id = "BROKEN".into();
    if let Some(rig) = broken.realistic.as_mut() {
        rig.white_under_key = -1.0;
    }
    for document in [
        json!({"schema": "other/1", "items": [mine]}),
        json!({"schema": FORMAT, "items": [mine, product]}),
        json!({"schema": FORMAT, "items": [mine, broken]}),
        json!({"schema": FORMAT, "items": [mine], "extra": true}),
    ] {
        assert!(s.execute("render.studio.import", &json!({"document": document})).is_err(), "{document}");
        assert!(s.studios.user.is_empty());
    }
}

/// A GPU setup lit by a user studio keeps the studio's record in the design, so it renders the
/// same elsewhere; built-in studios are named only. A setup that no longer names it drops it.
#[test]
fn a_setup_keeps_the_user_studio_it_names() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 20, "width": 15, "height": 10})).unwrap();
    let copy = run(&mut s, "render.studio.duplicate", json!({"id": DEFAULT_REALISTIC, "new_id": "MY-STUDIO"}));
    assert_eq!(copy["studio"]["id"], "MY-STUDIO");
    let job =
        |id: &str| json!({"method": "gpu_path", "options": {"samples": 4, "studio": {"id": id, "rotation_rad": 0.5}, "tone_map": "pbr_neutral"}});
    run(&mut s, "render.settings.set", json!({"name": "Shot", "set": {"job": job("MY-STUDIO")}}));
    assert!(s.doc.render.studios.contains_key("MY-STUDIO"));
    let reopened = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
    assert_eq!(reopened.render.studios, s.doc.render.studios);
    // Another machine has no such user studio: the design's copy lights the job.
    let elsewhere = Session { doc: std::sync::Arc::new(reopened), ..Default::default() };
    assert!(resolve(&elsewhere, "MY-STUDIO").is_some());
    run(&mut s, "render.settings.set", json!({"name": "Shot", "set": {"job": job(DEFAULT_REALISTIC)}}));
    assert!(s.doc.render.studios.is_empty(), "built-in studios are named, not copied");
    assert!(s.execute("render.settings.set", &json!({"name": "Shot", "set": {"job": job("NOWHERE")}})).is_err());
}

/// A job's studio becomes its lighting: the rig's lights as area lights and its cyclorama as the
/// environment; a studio without a rig, or one beside explicit lighting, is refused.
#[test]
fn a_jobs_studio_becomes_its_lighting() {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 20, "width": 15, "height": 10})).unwrap();
    let bounds = crate::product_scene::extract(&s).unwrap().bounds;
    let choice = solidcraft_doc::render_job::StudioChoice { id: DEFAULT_REALISTIC.into(), rotation_rad: 0.0 };
    let (lighting, environment) = job_lighting(&s, &choice, bounds, "test").unwrap();
    assert_eq!(lighting.area_lights.iter().count(), 3);
    assert_eq!(environment.info().kind, "studio_cyclorama");
    let native = solidcraft_doc::render_job::StudioChoice { id: "NATIVE-STUDIO-001".into(), rotation_rad: 0.0 };
    assert!(job_lighting(&s, &native, bounds, "test").is_err(), "a composition without a rig lights no job");
    s.execute("render.studio.duplicate", &json!({"id": DEFAULT_REALISTIC, "new_id": "MINE"})).unwrap();
    let mine = solidcraft_doc::render_job::StudioChoice { id: "MINE".into(), rotation_rad: 0.0 };
    assert!(job_lighting(&s, &mine, bounds, "test").is_ok());
}
