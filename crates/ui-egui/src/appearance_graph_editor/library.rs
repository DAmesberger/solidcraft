//! Browse graph entries in the shared appearance editor. Importing copies a graph into
//! the design. The editor's usual Use action assigns the copy to the picked objects.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::schema::appearance_graph::GraphLibraryListResult;
use solidcraft_ui_text::{format, tr};

use super::{Action, Commands};
use crate::ui_kit;
#[cfg(not(target_arch = "wasm32"))]
use {super::Editor, crate::SolveApp};

#[derive(Clone, Debug, Default)]
pub(super) struct Browser {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) token: Arc<()>,
    #[cfg(not(target_arch = "wasm32"))]
    pending: Option<Pending>,
    folder: Option<String>,
    listing: Option<Result<Arc<GraphLibraryListResult>, String>>,
}

impl Browser {
    pub(super) fn source_params(&self) -> Value {
        self.folder.as_ref().map_or_else(|| json!({}), |folder| json!({ "folder": folder }))
    }

    pub(super) fn load(&mut self, c: &mut dyn Commands, folder: Option<String>) {
        self.folder = folder;
        self.listing = Some(
            c.query("appearance.graph.library.list", self.source_params())
                .and_then(|r| serde_json::from_value::<GraphLibraryListResult>(r).map(Arc::new).map_err(|e| e.to_string())),
        );
    }

    pub(super) fn rows(&self, ui: &mut egui::Ui, can_pick_folder: bool, actions: &mut Vec<Action>) {
        ui_kit::section(ui, "ap_graph_library", tr("appearance.graph.library.add"), false, |ui| {
            #[cfg(not(target_arch = "wasm32"))]
            if self.pending.is_some() {
                ui.spinner();
                return;
            }
            ui.horizontal_wrapped(|ui| {
                if self.folder.is_some() {
                    if ui_kit::button(ui, tr("appearance.graph.library.starters"), ui_kit::Button::Normal).clicked() {
                        actions.push(Action::Browse(None));
                    }
                    if ui_kit::button(ui, tr("library.refresh"), ui_kit::Button::Normal).clicked() {
                        actions.push(Action::Browse(self.folder.clone()));
                    }
                }
                if can_pick_folder && ui_kit::button(ui, tr("appearance.graph.library.folder"), ui_kit::Button::Normal).clicked() {
                    actions.push(Action::PickLibrary);
                }
            });
            let listing = match &self.listing {
                None => {
                    actions.push(Action::Browse(None));
                    return;
                }
                Some(Err(error)) => {
                    ui_kit::status_line(ui, error, ui_kit::StatusTone::Error);
                    return;
                }
                Some(Ok(listing)) => listing,
            };
            let source = self.folder.as_deref().unwrap_or_else(|| tr("appearance.graph.library.starters"));
            ui_kit::text(ui, source, ui_kit::Text::Report("graph_library_name", false)).on_hover_text(listing.library.as_str());
            if listing.entries.is_empty() {
                ui_kit::text(ui, tr("appearance.graph.library.empty"), ui_kit::Text::Note("graph_library_empty"));
            }
            let height = ui_kit::row_height();
            egui::ScrollArea::vertical().id_salt("graph_library_entries").max_height(height * 6.0).show_rows(
                ui,
                height,
                listing.entries.len() + listing.refused.len(),
                |ui, rows| {
                    for row in rows {
                        if let Some(entry) = listing.entries.get(row) {
                            let button = ui_kit::button(ui, entry.name.as_str(), ui_kit::Button::Normal).on_hover_ui(|ui| {
                                let license = format("appearance.graph.library.provenance", &[("license", &entry.provenance.license)]);
                                ui.label(license);
                                if let Some(author) = &entry.provenance.author {
                                    ui.label(author.as_str());
                                }
                                if let Some(source) = &entry.provenance.source_url {
                                    ui.label(source.as_str());
                                }
                            });
                            crate::scenario::publish_handle(&format!("appearance:graph_import:{}", entry.id), button.rect.center());
                            if button.clicked() {
                                actions.push(Action::ImportEntry { id: entry.id.clone(), revision: entry.revision.clone() });
                            }
                        } else if let Some(refusal) = listing.refused.get(row.saturating_sub(listing.entries.len())) {
                            let text = format("appearance.graph.library.refusal", &[("label", &refusal.label), ("reason", &refusal.reason)]);
                            ui_kit::status_line(ui, &text, ui_kit::StatusTone::Warning);
                        }
                    }
                },
            );
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderCommand {
    List,
    Import,
}

#[cfg(not(target_arch = "wasm32"))]
impl FolderCommand {
    fn id(self) -> &'static str {
        match self {
            Self::List => "appearance.graph.library.list",
            Self::Import => "appearance.graph.library.import",
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct Pending {
    worker: Arc<std::sync::Mutex<crate::starting::Worker<Result<Reply, String>>>>,
    command: FolderCommand,
    params: Value,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending").field("command", &self.command).finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
enum Reply {
    List(Arc<GraphLibraryListResult>),
    Import(Box<solidcraft_engine::ComputedCommands>),
}

/// The native library scanner reads and verifies image files beside the window. A prepared
/// import is the ordinary command run on a scratch session, still subject to live undo and
/// design checks when accepted.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn begin(app: &mut SolveApp, ed: &mut Editor, ctx: egui::Context, command: FolderCommand, params: Value) {
    if ed.library.pending.is_some() {
        return;
    }
    if command == FolderCommand::List {
        ed.library.folder = params["folder"].as_str().map(str::to_owned);
        ed.library.listing = None;
    }
    let scope = solidcraft_engine::io::vfs::current_scope();
    let mut session = app.session.scratch();
    let given = params.clone();
    match crate::starting::Worker::spawn("appearance-graph-library", ctx, move || {
        solidcraft_engine::io::vfs::scoped(scope, || {
            if command == FolderCommand::List {
                let value = session.execute(command.id(), &given).map_err(|error| error.to_string())?;
                serde_json::from_value(value).map(|data| Reply::List(Arc::new(data))).map_err(|error| error.to_string())
            } else {
                session
                    .compute_commands(&[(command.id().into(), given)], None)
                    .map(|prepared| Reply::Import(Box::new(prepared)))
                    .map_err(|e| e.to_string())
            }
        })
    }) {
        Ok(worker) => {
            ed.note = None;
            ed.library.pending = Some(Pending { worker: Arc::new(std::sync::Mutex::new(worker)), command, params });
        }
        Err(error) => library_error(ed, command, error.to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn library_error(ed: &mut Editor, command: FolderCommand, error: String) {
    if command == FolderCommand::List {
        ed.library.listing = Some(Err(error));
    } else {
        ed.note = Some((error, true));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn poll(app: &mut SolveApp, ed: &mut Editor) {
    use crate::starting::Poll;
    let Some(pending) = &ed.library.pending else { return };
    let reply = match pending.worker.lock().map_or(Poll::Failed, |worker| worker.poll()) {
        Poll::Running => return,
        Poll::Failed => Err(tr("library.scan_stopped").into()),
        Poll::Done(reply) => reply,
    };
    let Some(pending) = ed.library.pending.take() else { return };
    match reply {
        Ok(Reply::List(list)) => ed.library.listing = Some(Ok(list)),
        Ok(Reply::Import(prepared)) => {
            let result = accept(app, *prepared, &pending.params);
            match result {
                Ok(reply) => {
                    ed.chosen = reply["id"].as_str().map(str::to_owned);
                    ed.layer = 0;
                    ed.refusal = None;
                    ed.stale();
                }
                Err(error) => ed.note = Some((error, true)),
            }
        }
        Err(error) => library_error(ed, pending.command, error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn accept(app: &mut SolveApp, prepared: solidcraft_engine::ComputedCommands, params: &Value) -> Result<Value, String> {
    let revision = app.session.revision;
    let selection = app.session.selection.clone();
    if app.file_load_holds() || app.file_picker_busy() || app.saving.is_some() {
        return Err(tr("library.file_working").to_owned());
    }
    let reply = app
        .session
        .accept_commands(prepared)
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .map(|(_, reply)| reply)
        .ok_or_else(|| tr("library.scan_stopped").to_owned());
    app.after_command("appearance.graph.library.import", params, revision, selection, reply)
}

/// Only the editor that opened the picker can receive the folder, even across a document switch.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn pick_folder(app: &mut SolveApp, current: fn(&mut SolveApp) -> Option<&mut Editor>, token: Arc<()>, ctx: egui::Context) {
    let origin = Arc::downgrade(&app.session.doc);
    app.pick_folder(move |app, folder| {
        let Some(folder) = folder else { return };
        if !origin.ptr_eq(&Arc::downgrade(&app.session.doc)) {
            return;
        }
        let Some(target) = current(app).filter(|ed| Arc::ptr_eq(&ed.library.token, &token)) else { return };
        let mut ed = std::mem::take(target);
        begin(app, &mut ed, ctx, FolderCommand::List, json!({"folder": folder}));
        if let Some(target) = current(app) {
            *target = ed;
        }
    });
}
