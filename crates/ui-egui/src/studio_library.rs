//! The user's studios and the Realistic studio, kept with the preferences (`studios`), as the
//! settings profiles are: they belong to the user, not to a design. The engine's studio library
//! (`render.studio.*`, `solidcraft_engine::cmd::render::studios`) works on the session's copy;
//! the window gives every session the user's library before a render command and takes it back
//! after a `render.studio.*` command, so all design tabs share one library.

use serde_json::{Value, json};
use solidcraft_engine::cmd::render::studios::{MAX_USER_STUDIOS, StudioLibrary};
use solidcraft_viewport::studio::Studio;

use crate::SolveApp;

/// The preference key.
pub(crate) const KEY: &str = "studios";

/// Before command `id`: the session works on the user's library.
pub(crate) fn before(app: &mut SolveApp, id: &str) {
    if id.starts_with("render.") && app.session.studios != app.ui.studios {
        app.session.studios = app.ui.studios.clone();
    }
}

/// After command `id` succeeded: a studio command's changes become the user's.
pub(crate) fn after(app: &mut SolveApp, id: &str) {
    if id.starts_with("render.studio.") && app.ui.studios != app.session.studios {
        app.ui.studios = app.session.studios.clone();
    }
}

/// A built-in studio's rig by name or id (case does not matter), registered for the renderer: for
/// evidence runs that name a studio (`GOLDEN_STUDIO`, `LIGHTING_REVIEW_STUDIO`).
#[cfg(test)]
pub(crate) fn builtin_preset(name: &str) -> Option<solidcraft_viewport_gpu::view::StudioPreset> {
    solidcraft_engine::cmd::render::studios::builtin()
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name) || s.id.eq_ignore_ascii_case(name))
        .and_then(|s| s.realistic.as_ref())
        .map(solidcraft_viewport_gpu::view::StudioPreset::register)
}

/// The preference entry [`SolveApp::prefs`] writes.
pub(crate) fn prefs(app: &SolveApp) -> Value {
    json!({"user": app.ui.studios.user, "realistic_default": app.ui.studios.realistic_default})
}

/// Restore the user's studios from saved preferences. A studio that does not check out, or a
/// duplicate id, is left out; a default naming a studio that is gone falls back to Product.
pub(crate) fn load(app: &mut SolveApp, saved: &Value) {
    let Some(entry) = saved.get(KEY) else { return };
    let mut library = StudioLibrary::default();
    for item in entry.get("user").and_then(Value::as_array).into_iter().flatten().take(MAX_USER_STUDIOS) {
        let Ok(studio) = serde_json::from_value::<Studio>(item.clone()) else { continue };
        if studio.validate().is_ok() && library.get(&studio.id).is_none() {
            library.user.push(studio);
        }
    }
    library.realistic_default =
        entry.get("realistic_default").and_then(Value::as_str).filter(|id| library.get(id).is_some_and(|s| s.realistic.is_some())).map(str::to_owned);
    app.ui.studios = library;
    app.session.studios = app.ui.studios.clone();
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::SolveApp;

    /// A user studio and the Realistic default survive the preferences, and come back into every
    /// session the window runs a render command in.
    #[test]
    fn the_users_studios_are_kept_with_the_preferences() {
        let mut app = SolveApp::new(Default::default(), Default::default());
        app.run("render.studio.duplicate", json!({"id": "NATIVE-STUDIO-028", "new_id": "MINE", "name": "Mine"})).unwrap();
        app.run("render.studio.default", json!({"id": "MINE"})).unwrap();
        let saved = app.prefs();
        let mut other = SolveApp::new(Default::default(), Default::default());
        other.load_prefs(&saved);
        assert_eq!(other.ui.studios.realistic().map(|s| s.id.as_str()), Some("MINE"));
        other.session = solidcraft_engine::Session::default();
        let listed = other.run("render.studio.list", json!({})).unwrap();
        assert_eq!(listed["realistic_default"], "MINE");
    }
}
