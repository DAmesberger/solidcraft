//! Studio lighting for the view being drawn, including an inactive tile.
use crate::SolveApp;

pub(crate) fn available(app: &SolveApp) -> bool {
    app.viewport.gpu.is_some() && !app.safe_graphics
}

pub(crate) fn enabled(app: &SolveApp) -> bool {
    app.ui.realistic && available(app)
}

pub(crate) fn product_look(app: &SolveApp) -> Option<crate::gpu::ProductLook> {
    enabled(app).then(|| crate::gpu::ProductLook { studio: studio(app), ..Default::default() })
}

/// The studio the Realistic looks light with: the user's Realistic studio
/// (`render.studio.default`, Product by default), registered for the renderer. Also kept for views
/// without the app at hand ([`last_studio`]).
pub(crate) fn studio(app: &SolveApp) -> solidcraft_viewport_gpu::view::StudioPreset {
    let preset =
        app.ui.studios.realistic().and_then(|s| s.realistic.as_ref()).map(solidcraft_viewport_gpu::view::StudioPreset::register).unwrap_or_default();
    if let Ok(mut last) = LAST.lock() {
        *last = preset;
    }
    preset
}

static LAST: std::sync::Mutex<solidcraft_viewport_gpu::view::StudioPreset> = std::sync::Mutex::new(solidcraft_viewport_gpu::view::StudioPreset::NONE);

/// The Realistic studio as last worked out for a view ([`studio`]): for pictures drawn without the
/// app at hand, such as the material swatch.
pub(crate) fn last_studio() -> solidcraft_viewport_gpu::view::StudioPreset {
    LAST.lock().map(|last| *last).unwrap_or_default()
}

/// [`product_look`] with the studio's room fitted to the shown subject's `bounds`, so its lights
/// stand about the subject and reflections are box-projected (`StudioRoom::around`). An empty box
/// leaves the studio at infinity.
pub(crate) fn product_look_around(app: &SolveApp, bounds: &solidcraft_engine::geom::Aabb3) -> Option<crate::gpu::ProductLook> {
    let room = (!bounds.is_empty()).then(|| solidcraft_viewport_gpu::view::StudioRoom::around(bounds.min.to_f32(), bounds.max.to_f32())).flatten();
    product_look(app).map(|look| crate::gpu::ProductLook { room, ..look })
}
