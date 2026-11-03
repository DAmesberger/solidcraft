//! The look of a view (`view.look`, docs/material-graph.md §6): Standard, today's drawing and the
//! default, or Realistic, faces lit by the studio through the product look
//! ([`crate::gpu::ProductLook`]). The modelling viewport and the Render workspace preview each
//! have their own. Both are UI preferences remembered like the visual style, never saved in the
//! design or its undo history; opening a design never changes them.

use serde_json::{Value, json};

use crate::SolveApp;

/// Standard, the default look of every view.
pub(crate) const STANDARD: u8 = 0;
/// Realistic.
pub(crate) const REALISTIC: u8 = 1;

/// The look's name in `view.look`.
fn name(look: u8) -> &'static str {
    if look == REALISTIC { "realistic" } else { "standard" }
}

fn from_name(look: &str) -> Option<u8> {
    match look {
        "standard" => Some(STANDARD),
        "realistic" => Some(REALISTIC),
        _ => None,
    }
}

/// The view a command without `view` acts on: the Render preview while the Render workspace is
/// shown, else the modelling viewport.
fn active_view(app: &SolveApp) -> &'static str {
    if crate::render_workspace::active(app) { "render_preview" } else { "model" }
}

/// The look `view` (`model` or `render_preview`) has now.
pub(crate) fn current(app: &SolveApp, view: &str) -> u8 {
    if view == "render_preview" { app.ui.render_preview_look } else { app.ui.view_look }
}

/// Why Realistic can't be switched on here, if it can't: it needs the GPU viewport.
pub(crate) fn unavailable(app: &SolveApp) -> Option<String> {
    app.viewport.gpu.is_none().then(|| crate::i18n::tr("view.look.no_gpu").to_owned())
}

/// Make a `view.look` request explicit before the engine checks it: the active view when it names
/// none, the other look when it names none (a toggle), and Realistic refused without a GPU.
/// Parameters of the wrong shape are passed on as they are, for the engine to refuse.
pub(crate) fn resolve(app: &SolveApp, params: Value) -> Result<Value, String> {
    let Value::Object(mut request) = params else { return Ok(params) };
    let view = match request.get("view") {
        None => active_view(app),
        Some(view) => match view.as_str() {
            Some("model") => "model",
            Some("render_preview") => "render_preview",
            _ => return Ok(Value::Object(request)),
        },
    };
    let look = match request.get("look") {
        None => {
            if current(app, view) == REALISTIC {
                STANDARD
            } else {
                REALISTIC
            }
        }
        Some(look) => match look.as_str().and_then(from_name) {
            Some(look) => look,
            None => return Ok(Value::Object(request)),
        },
    };
    if look == REALISTIC
        && let Some(reason) = unavailable(app)
    {
        return Err(reason);
    }
    request.insert("look".into(), json!(name(look)));
    request.insert("view".into(), json!(view));
    Ok(Value::Object(request))
}

/// Apply the engine's validated reply (`{look, view}`).
pub(crate) fn apply(app: &mut SolveApp, reply: &Value) {
    let Some(look) = reply.get("look").and_then(Value::as_str).and_then(from_name) else { return };
    match reply.get("view").and_then(Value::as_str) {
        Some("render_preview") => app.ui.render_preview_look = look,
        Some("model") => app.ui.view_look = look,
        _ => {}
    }
}

/// What a view's frame asks the renderer for: the product look's default studio in Realistic,
/// nothing (the standard drawing) in Standard or without a GPU.
pub(crate) fn product_look(app: &SolveApp, view: &str) -> Option<crate::gpu::ProductLook> {
    (current(app, view) == REALISTIC && unavailable(app).is_none()).then(crate::gpu::ProductLook::default)
}

/// The preference entries [`SolveApp::prefs`] writes.
pub(crate) fn prefs(app: &SolveApp) -> [(&'static str, Value); 2] {
    [("view_look", json!(app.ui.view_look)), ("render_preview_look", json!(app.ui.render_preview_look))]
}

/// Restore the looks from saved preferences. A fork's saved Graphics preference
/// `render_studio_look: true` (studio lighting in the Render preview, which the view mode
/// replaces) becomes the Render preview's Realistic once: only while the file has no
/// `render_preview_look` yet, which the next save writes and which drops the old key.
pub(crate) fn load(app: &mut SolveApp, saved: &Value) {
    let look = |key: &str| saved.get(key).and_then(Value::as_u64).map(|look| if look == u64::from(REALISTIC) { REALISTIC } else { STANDARD });
    if let Some(look) = look("view_look") {
        app.ui.view_look = look;
    }
    match look("render_preview_look") {
        Some(look) => app.ui.render_preview_look = look,
        None if saved.pointer("/preferences/render_studio_look").and_then(Value::as_bool) == Some(true) => {
            app.ui.render_preview_look = REALISTIC;
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{REALISTIC, STANDARD};
    use crate::SolveApp;

    fn with_gpu(app: &mut SolveApp) {
        app.viewport.gpu =
            Some(crate::gpu::GpuTarget { format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm, depth: None, samples: 1, tess: false, oit: false });
    }

    /// The command toggles without a look, sets one when named, acts on the modelling view unless
    /// told otherwise, answers with the look the view now has and never touches the design or its
    /// undo; the frame asks for the product look only in Realistic.
    #[test]
    fn view_look_toggles_sets_and_never_edits_the_design() {
        let mut app = SolveApp::new(Default::default(), Default::default());
        with_gpu(&mut app);
        app.run("solid.box", json!({"length": 10, "width": 10, "height": 10})).unwrap_or_else(|e| panic!("{e}"));
        let (design, undo) = (app.session.doc.to_json(), app.session.undo.len());
        assert_eq!((app.ui.view_look, app.ui.render_preview_look), (STANDARD, STANDARD), "Standard by default");
        assert_eq!(super::product_look(&app, "model"), None);
        let r = app.run("view.look", json!({})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r, json!({"look": "realistic", "view": "model"}));
        assert_eq!(app.ui.view_look, REALISTIC);
        assert_eq!(super::product_look(&app, "model"), Some(crate::gpu::ProductLook::default()));
        assert_eq!(super::product_look(&app, "render_preview"), None, "the Render preview has its own look");
        let r = app.run("view.look", json!({})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r, json!({"look": "standard", "view": "model"}), "a second toggle goes back");
        for _ in 0..2 {
            let r = app.run("view.look", json!({"look": "realistic"})).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(r["look"], "realistic", "naming the look sets it, again and again");
        }
        let r = app.run("view.look", json!({"view": "render_preview"})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r, json!({"look": "realistic", "view": "render_preview"}));
        assert_eq!((app.ui.view_look, app.ui.render_preview_look), (REALISTIC, REALISTIC));
        assert!(app.run("view.look", json!({"look": "photoreal"})).is_err());
        assert!(app.run("view.look", json!({"view": "sketch"})).is_err());
        assert!(app.run("view.look", json!({"look": "standard", "extra": 1})).is_err());
        assert_eq!((app.ui.view_look, app.ui.render_preview_look), (REALISTIC, REALISTIC), "a refused request changes nothing");
        assert_eq!((app.session.doc.to_json(), app.session.undo.len()), (design, undo), "never in the design or its undo");
    }

    /// Without a GPU viewport (the CPU viewport) Realistic is refused with the reason, the view
    /// stays Standard, and Standard can still be named.
    #[test]
    fn realistic_is_refused_without_a_gpu() {
        let mut app = SolveApp::new(Default::default(), Default::default());
        assert!(app.viewport.gpu.is_none());
        for params in [json!({}), json!({"look": "realistic"}), json!({"look": "realistic", "view": "render_preview"})] {
            let error = app.run("view.look", params.clone()).err().unwrap_or_default();
            assert_eq!(error, crate::i18n::tr("view.look.no_gpu"), "{params}");
        }
        assert_eq!((app.ui.view_look, app.ui.render_preview_look), (STANDARD, STANDARD));
        assert_eq!(app.run("view.look", json!({"look": "standard"})).ok(), Some(json!({"look": "standard", "view": "model"})));
        // A look saved on a machine with a GPU draws nothing it can't.
        app.ui.view_look = REALISTIC;
        assert_eq!(super::product_look(&app, "model"), None);
    }

    /// In the window: Shift+P toggles the modelling view, the navigation bar's visual-style menu
    /// has a Realistic row that does the same, and without a GPU viewport the row is disabled and
    /// the key says why in the status line.
    #[test]
    fn shift_p_and_the_menu_row_switch_the_look() {
        let mut h = crate::scenario::Harness::new();
        h.step(&json!({"key": "P", "shift": true})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(h.app.ui.view_look, STANDARD, "no GPU: refused");
        assert_eq!(h.app.status.as_ref().map(|s| s.0.as_str()), Some(crate::i18n::tr("view.look.no_gpu")));
        h.step(&json!({"click": {"handle": "nav:slot:visual_style"}})).unwrap_or_else(|e| panic!("{e}"));
        h.step(&json!({"click": {"handle": "nav:style:realistic"}})).ok();
        assert_eq!(h.app.ui.view_look, STANDARD, "the row is disabled without a GPU");
        h.step(&json!({"key": "Escape"})).ok();
        with_gpu(&mut h.app);
        h.step(&json!({"key": "P", "shift": true})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(h.app.ui.view_look, REALISTIC, "Shift+P");
        h.step(&json!({"click": {"handle": "nav:slot:visual_style"}})).unwrap_or_else(|e| panic!("{e}"));
        h.step(&json!({"click": {"handle": "nav:style:realistic"}})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(h.app.ui.view_look, STANDARD, "the menu row toggles it back");
        assert_eq!(h.app.ui.visual_style, 0, "the visual style is a setting of its own");
    }

    /// Both looks are remembered between runs like the visual style; opening a design leaves them
    /// as they were and Restore Defaults on the Graphics page sets both to Standard.
    #[test]
    fn the_looks_round_trip_through_the_preferences() {
        let mut app = SolveApp::new(Default::default(), Default::default());
        with_gpu(&mut app);
        app.run("view.look", json!({"look": "realistic"})).unwrap_or_else(|e| panic!("{e}"));
        let saved: Value = serde_json::from_str(&app.prefs()).unwrap_or_default();
        assert_eq!((saved["view_look"].clone(), saved["render_preview_look"].clone()), (json!(1), json!(0)));
        let mut restored = SolveApp::new(Default::default(), Default::default());
        restored.load_prefs(&app.prefs());
        assert_eq!((restored.ui.view_look, restored.ui.render_preview_look), (REALISTIC, STANDARD));
        restored.ui.render_preview_look = REALISTIC;
        let mut again = SolveApp::new(Default::default(), Default::default());
        again.load_prefs(&restored.prefs());
        assert_eq!((again.ui.view_look, again.ui.render_preview_look), (REALISTIC, REALISTIC));
        // Opening another design keeps the user's looks.
        again.run("file.new", json!({})).ok();
        assert_eq!((again.ui.view_look, again.ui.render_preview_look), (REALISTIC, REALISTIC));
        crate::prefs::defaults(&mut again, crate::prefs::PAGE_GRAPHICS);
        assert_eq!((again.ui.view_look, again.ui.render_preview_look), (STANDARD, STANDARD), "Restore Defaults");
        // An unknown number reads as Standard.
        again.load_prefs(r#"{"view_look": 7, "render_preview_look": 1}"#);
        assert_eq!((again.ui.view_look, again.ui.render_preview_look), (STANDARD, REALISTIC));
    }

    /// A fork user's `render_studio_look: true` becomes the Render preview's Realistic once; the
    /// next save writes the look and no longer carries the old key, so it never comes back.
    #[test]
    fn the_old_studio_preference_carries_over_once() {
        let old = r#"{"visual_style": 1, "preferences": {"render_studio_look": true}}"#;
        let mut app = SolveApp::new(Default::default(), Default::default());
        app.load_prefs(old);
        assert_eq!((app.ui.view_look, app.ui.render_preview_look), (STANDARD, REALISTIC));
        let saved: Value = serde_json::from_str(&app.prefs()).unwrap_or_default();
        assert_eq!(saved["render_preview_look"], json!(1));
        assert_eq!(saved.pointer("/preferences/render_studio_look"), None, "the old key is dropped");
        // Switched back to Standard and saved: the old key, were it still there, is not read again.
        app.ui.render_preview_look = STANDARD;
        let mut saved: Value = serde_json::from_str(&app.prefs()).unwrap_or_default();
        if let Some(preferences) = saved.get_mut("preferences").and_then(Value::as_object_mut) {
            preferences.insert("render_studio_look".into(), json!(true));
        }
        let mut next = SolveApp::new(Default::default(), Default::default());
        next.load_prefs(&saved.to_string());
        assert_eq!(next.ui.render_preview_look, STANDARD);
        // `false` or no old key: nothing to carry.
        for old in [r#"{"preferences": {"render_studio_look": false}}"#, "{}"] {
            let mut app = SolveApp::new(Default::default(), Default::default());
            app.load_prefs(old);
            assert_eq!(app.ui.render_preview_look, STANDARD, "{old}");
        }
    }
}
