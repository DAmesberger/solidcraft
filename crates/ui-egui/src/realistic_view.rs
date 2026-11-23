//! Studio lighting for the view being drawn, including an inactive tile.
use crate::SolveApp;

pub(crate) fn available(app: &SolveApp) -> bool {
    app.viewport.gpu.is_some() && !app.safe_graphics
}

pub(crate) fn enabled(app: &SolveApp) -> bool {
    app.ui.realistic && available(app)
}

pub(crate) fn product_look(app: &SolveApp) -> Option<crate::gpu::ProductLook> {
    enabled(app).then(|| crate::gpu::ProductLook { studio: studio(), ..Default::default() })
}

/// The studio used by the Model and Render views and by appearance swatches.
pub(crate) fn studio() -> solidcraft_viewport_gpu::view::StudioPreset {
    solidcraft_engine::cmd::studios::default_rig().map(solidcraft_viewport_gpu::view::StudioPreset::register).unwrap_or_default()
}

/// [`product_look`] with the studio's room fitted to the shown subject's `bounds`, so its lights
/// stand about the subject and reflections are box-projected (`StudioRoom::around`). An empty box
/// leaves the studio at infinity.
pub(crate) fn product_look_around(app: &SolveApp, bounds: impl FnOnce() -> solidcraft_engine::geom::Aabb3) -> Option<crate::gpu::ProductLook> {
    let look = product_look(app)?;
    let bounds = bounds();
    let room = (!bounds.is_empty()).then(|| solidcraft_viewport_gpu::view::StudioRoom::around(bounds.min.to_f32(), bounds.max.to_f32())).flatten();
    Some(crate::gpu::ProductLook { room, ..look })
}

/// Keep an idle view painting while its studio or appearance pipelines are being prepared.
pub(crate) fn repaint_while_preparing(app: &SolveApp, ctx: &egui::Context, look: Option<crate::gpu::ProductLook>) {
    let (Some(look), Some(stats)) = (look, app.viewport.draw_stats.as_ref()) else { return };
    if !solidcraft_viewport_egui::studio_ready(stats, look.studio) || stats.lock().is_ok_and(|s| s.appearances_pending) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}
