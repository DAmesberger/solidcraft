//! Preferences › Graphics › Realistic studios: the studio library's studios with a rig, the one
//! the Realistic view lights with, and the user's own (duplicate, rename, adjust, import, export,
//! delete). Every change goes through `render.studio.*`, as the CLI and MCP make it.

use serde_json::{Value, json};
use solidcraft_viewport::studio::Studio;

use crate::{SolveApp, ui_kit};

/// The lights' and the room's adjustments, in stops (EV) from the studio as saved: applied whole
/// when the slider is let go.
#[derive(Clone, Debug, Default)]
pub(crate) struct Edit {
    id: String,
    name: String,
    lights_ev: f64,
    room_ev: f64,
}

fn run(app: &mut SolveApp, id: &str, params: Value) {
    if let Err(error) = app.run(id, params) {
        app.ui.studio_message = Some(error);
    } else {
        app.ui.studio_message = None;
    }
}

/// `studio`'s rig with its lights and room scaled by `2^lights_ev` and `2^room_ev`.
fn adjusted(studio: &Studio, lights_ev: f64, room_ev: f64) -> Option<Value> {
    let mut rig = studio.realistic.clone()?;
    let (lights, room) = (2f64.powf(lights_ev), 2f64.powf(room_ev));
    for light in &mut rig.lights {
        light.radiance = light.radiance.map(|c| c * lights);
    }
    let c = &mut rig.cyclorama;
    for share in [&mut c.floor, &mut c.wall, &mut c.band, &mut c.ceiling] {
        *share = share.map(|v| (v * room).min(100.0));
    }
    serde_json::to_value(rig).ok()
}

pub(super) fn section(app: &mut SolveApp, ui: &mut egui::Ui) {
    ui_kit::settings_section(ui, "realistic_studios", app.tr("prefs.studios"), |ui| {
        ui_kit::text(ui, app.tr("prefs.studios_help"), ui_kit::Text::Hint("studios_help"));
        let realistic: Vec<(Studio, bool)> = solidcraft_engine::cmd::render::studios::builtin()
            .iter()
            .map(|s| (s.clone(), true))
            .chain(app.ui.studios.user.iter().map(|s| (s.clone(), false)))
            .filter(|(s, _)| s.realistic.is_some())
            .collect();
        let current = app.ui.studios.realistic().cloned();
        let current_id = current.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        let label = current.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        ui_kit::row(ui, app.tr("prefs.studios_studio"), |ui| {
            let response = ui_kit::dropdown_menu(ui, "realistic_studio", &label, |ui| {
                for (studio, builtin) in &realistic {
                    let text = if *builtin { studio.name.clone() } else { format!("{} ({})", studio.name, app.tr("prefs.studios_mine")) };
                    if ui_kit::list_picker(ui, &studio.id, &text, studio.id == current_id).clicked() {
                        run(app, "render.studio.default", json!({"id": studio.id}));
                        ui.close();
                    }
                }
            });
            crate::scenario::publish_target("prefs:studio", response.rect);
        });
        let builtin = realistic.iter().any(|(s, b)| *b && s.id == current_id);
        ui.horizontal(|ui| {
            let duplicate = ui_kit::icon_label_button(ui, app.tr("prefs.studios_duplicate"), "copy", current.is_some(), false);
            crate::scenario::publish_target("prefs:studio:duplicate", duplicate.rect);
            if duplicate.clicked() {
                let reply = app.run("render.studio.duplicate", json!({"id": current_id}));
                match reply {
                    Ok(reply) => {
                        if let Some(id) = reply["studio"]["id"].as_str() {
                            run(app, "render.studio.default", json!({"id": id}));
                        }
                    }
                    Err(error) => app.ui.studio_message = Some(error),
                }
            }
            let import = ui_kit::icon_label_button(ui, app.tr("prefs.general.import"), "import_tray", true, false);
            crate::scenario::publish_target("prefs:studio:import", import.rect);
            if import.clicked() {
                app.pick_open_command("render.studio.import", json!({}), &["json"]);
            }
            let export = ui_kit::icon_label_button(ui, app.tr("prefs.studios_export"), "export", current.is_some(), false);
            crate::scenario::publish_target("prefs:studio:export", export.rect);
            if export.clicked() {
                let name = current.as_ref().map_or_else(|| "studio".to_owned(), |s| s.name.clone());
                app.pick_save_command(&name, &["json"], "render.studio.export", json!({"ids": [current_id]}));
            }
            let delete = ui_kit::icon_label_button(ui, app.tr("prefs.studios_delete"), "delete", !builtin && current.is_some(), true);
            crate::scenario::publish_target("prefs:studio:delete", delete.rect);
            if delete.clicked() {
                run(app, "render.studio.delete", json!({"id": current_id}));
            }
        });
        // The user's own studio: its name, and its lights and room in stops.
        if let (Some(studio), false) = (current.as_ref(), builtin) {
            if app.ui.studio_edit.id != studio.id {
                app.ui.studio_edit = Edit { id: studio.id.clone(), name: studio.name.clone(), lights_ev: 0.0, room_ev: 0.0 };
            }
            let mut edit = std::mem::take(&mut app.ui.studio_edit);
            ui_kit::row(ui, app.tr("prefs.studios_name"), |ui| {
                let (enter, _) = ui_kit::text_field(ui, &mut edit.name, &studio.name);
                if enter && !edit.name.trim().is_empty() && edit.name != studio.name {
                    run(app, "render.studio.edit", json!({"id": studio.id, "name": edit.name.trim()}));
                }
            });
            let ev = || ui_kit::Slider { format: |v| format!("{v:+.1} EV"), ..Default::default() };
            let mut released = false;
            ui_kit::row(ui, app.tr("prefs.studios_lights"), |ui| {
                released |= ui_kit::slider(ui, &mut edit.lights_ev, -3.0..=3.0, ev()).drag_stopped();
            });
            ui_kit::row(ui, app.tr("prefs.studios_room"), |ui| {
                released |= ui_kit::slider(ui, &mut edit.room_ev, -3.0..=3.0, ev()).drag_stopped();
            });
            if released && (edit.lights_ev != 0.0 || edit.room_ev != 0.0) {
                if let Some(rig) = adjusted(studio, edit.lights_ev, edit.room_ev) {
                    run(app, "render.studio.edit", json!({"id": studio.id, "realistic": rig}));
                }
                edit.lights_ev = 0.0;
                edit.room_ev = 0.0;
            }
            app.ui.studio_edit = edit;
        }
        if let Some(message) = app.ui.studio_message.clone() {
            ui_kit::text(ui, &message, ui_kit::Text::Hint("studio_message"));
        }
    });
}
