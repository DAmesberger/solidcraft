//! The material editor: the Appearance dialog's **Material graph** section, a layered-stack
//! view of a design's material graphs (docs/material-graph.md §7.1).
//!
//! The editor holds no design state of its own. It reads command replies
//! (`appearance.graph.nodes`, `.list`, `.get`, `.validate`, `appearance.texture.list`) whenever
//! the design's revision changes, and every edit is one command: values through
//! `appearance.graph.edit` (a drag or a typed number is one step, sent when it settles) and
//! structure (layers, drivers) through `appearance.graph.set`. A refused command leaves the
//! design unchanged, so the editor simply shows the refusal and keeps the replies it had.
//!
//! A graph that is not stack-shaped opens read-only with a summary and "Edit as JSON".

mod model;
mod swatch;
#[cfg(test)]
mod tests;

use serde_json::{Value, json};
use solidcraft_engine::render::material_graph::{Constant, InterfaceInput, MaterialGraph, Node, ParamValue, PortType, limits};
use solidcraft_engine::render::surface::OpaqueSurface;

pub(crate) use model::{Catalogue, Driver, Loaded, Stack};
use model::{Drive, GraphEntry, KindSpec, ParamKind, PortSpec, TextureEntry, Unit, Validation};
pub(crate) use swatch::SwatchTargets;

use crate::SolveApp;
use crate::i18n::{format, tr};
use crate::theme::Tokens;
use crate::ui_kit;

/// Bytes the "Edit as JSON" box holds.
const JSON_LIMIT: usize = 1 << 20;

/// How the editor reaches the engine. The app runs read-only commands directly and changes
/// through `SolveApp::run` (undo, recovery); tests answer with schema-shaped replies.
pub(crate) trait Commands {
    /// A read-only command.
    fn query(&mut self, id: &str, params: Value) -> Result<Value, String>;
    /// A command that changes the design (one undo step).
    fn change(&mut self, id: &str, params: Value) -> Result<Value, String>;
    /// The design's revision: replies are read again when it moves.
    fn revision(&self) -> u64;
}

impl Commands for SolveApp {
    fn query(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.session.execute(id, &params).map_err(|e| e.to_string())
    }
    fn change(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.run(id, params)
    }
    fn revision(&self) -> u64 {
        self.session.revision
    }
}

/// A value being changed that has not been sent: shown in place of the reply's until it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Pending {
    Input { node: String, port: String, value: Constant },
    Param { node: String, param: String, value: ParamValue },
    Interface(InterfaceInput),
}

/// Where a texture picked or added goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TextureUse {
    /// An `image` node's `file`.
    File { node: String, port: String },
    /// Drive an input with a new `image` reading the texture.
    Drive { node: String, port: String },
    /// An interface input's default.
    Interface(InterfaceInput),
}

/// What a frame of the editor asks for; applied after drawing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Choose(String),
    Create(String),
    Duplicate,
    Delete,
    SelectLayer(usize),
    Edit(Vec<Value>),
    Set(MaterialGraph),
    Pending(Pending),
    Texture(String, TextureUse),
    /// Open the file picker, then add the texture and use it.
    AddTexture(TextureUse),
    Json(String),
    /// The name typed for a new graph.
    Name(String),
    Refused(String),
}

/// The editor's state inside the Appearance dialog.
#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub(crate) catalogue: Option<Result<Catalogue, String>>,
    pub(crate) graphs: Option<Result<Vec<GraphEntry>, String>>,
    pub(crate) chosen: Option<String>,
    pub(crate) loaded: Option<Result<Loaded, String>>,
    pub(crate) validation: Option<Result<Validation, String>>,
    pub(crate) textures: Option<Result<Vec<TextureEntry>, String>>,
    /// The revision the replies were read at.
    synced: Option<u64>,
    /// The selected layer (0: the base).
    pub(crate) layer: usize,
    pub(crate) pending: Option<Pending>,
    /// The last refusal or result to show.
    pub(crate) note: Option<(String, bool)>,
    pub(crate) new_name: String,
    /// The loaded graph's base response, read once per reply (`None` until validation is built).
    base: Option<OpaqueSurface>,
}

impl Editor {
    /// Read the replies again if the design changed since they were read (or never were).
    pub(crate) fn sync(&mut self, c: &mut dyn Commands) {
        if self.synced == Some(c.revision()) {
            return;
        }
        self.synced = Some(c.revision());
        if !matches!(self.catalogue, Some(Ok(_))) {
            self.catalogue = Some(c.query("appearance.graph.nodes", json!({})).and_then(|r| Catalogue::from_reply(&r)));
        }
        let graphs = c.query("appearance.graph.list", json!({})).and_then(|r| model::graph_list(&r));
        if let Ok(list) = &graphs
            && self.chosen.as_ref().is_none_or(|id| !list.iter().any(|g| &g.id == id))
        {
            self.chosen = list.first().map(|g| g.id.clone());
        }
        self.graphs = Some(graphs);
        self.textures = Some(c.query("appearance.texture.list", json!({})).and_then(|r| model::texture_list(&r)));
        self.reload(c);
    }

    fn reload(&mut self, c: &mut dyn Commands) {
        self.pending = None;
        let Some(id) = self.chosen.clone() else {
            self.loaded = None;
            self.validation = None;
            self.base = None;
            return;
        };
        let loaded = c.query("appearance.graph.get", json!({"id": id})).and_then(|r| Loaded::from_reply(&r));
        self.base = loaded.as_ref().ok().and_then(|l| {
            use solidcraft_engine::render::material_graph as mg;
            mg::validate(&l.graph).and_then(|v| mg::base_response(&v)).ok()
        });
        self.validation =
            loaded.as_ref().ok().map(|l| c.query("appearance.graph.validate", json!({"graph": l.graph})).and_then(|r| Validation::from_reply(&r)));
        if let Ok(l) = &loaded
            && let Ok(stack) = model::stack(&l.graph)
        {
            self.layer = self.layer.min(stack.layers.len().saturating_sub(1));
        }
        self.loaded = Some(loaded);
    }

    /// Mark the replies stale, so the next frame reads them again.
    fn stale(&mut self) {
        self.synced = None;
    }

    fn graph(&self) -> Option<&Loaded> {
        self.loaded.as_ref()?.as_ref().ok()
    }

    /// Send one changing command; on refusal keep the replies and show why.
    fn change(&mut self, c: &mut dyn Commands, id: &str, params: Value) -> Option<Value> {
        match c.change(id, params) {
            Ok(reply) => {
                self.note = None;
                self.stale();
                Some(reply)
            }
            Err(e) => {
                self.note = Some((e, true));
                self.pending = None;
                None
            }
        }
    }

    /// Send the pending value as one edit.
    pub(crate) fn commit(&mut self, c: &mut dyn Commands) {
        let (Some(pending), Some(id)) = (self.pending.take(), self.graph().map(|l| l.id.clone())) else { return };
        let op = match &pending {
            Pending::Input { node, port, value } => model::set_input(node, port, value),
            Pending::Param { node, param, value } => model::set_param(node, param, value),
            Pending::Interface(input) => model::set_interface(input),
        };
        self.change(c, "appearance.graph.edit", model::edit(&id, vec![op]));
    }

    /// Apply what a frame asked for. Returns a texture use that needs the file picker.
    pub(crate) fn apply(&mut self, c: &mut dyn Commands, action: Action) -> Option<TextureUse> {
        let id = self.graph().map(|l| l.id.clone());
        match action {
            Action::Choose(graph) => {
                self.chosen = Some(graph);
                self.layer = 0;
                self.stale();
            }
            Action::Create(name) => {
                let name = name.trim().to_string();
                if name.is_empty() || name.len() > limits::NAME_BYTES || name.chars().any(char::is_control) {
                    self.note = Some((tr("appearance.graph.bad_name").into(), true));
                    return None;
                }
                if let Some(reply) = self.change(c, "appearance.graph.create", json!({"name": name})) {
                    self.chosen = reply.get("id").and_then(Value::as_str).map(str::to_string);
                    self.new_name.clear();
                    self.layer = 0;
                }
            }
            Action::Duplicate => {
                if let Some(id) = id
                    && let Some(reply) = self.change(c, "appearance.graph.duplicate", json!({"id": id}))
                {
                    self.chosen = reply.get("id").and_then(Value::as_str).map(str::to_string);
                }
            }
            Action::Delete => {
                if let Some(id) = id
                    && self.change(c, "appearance.graph.delete", json!({"id": id})).is_some()
                {
                    self.chosen = None;
                }
            }
            Action::SelectLayer(i) => self.layer = i,
            Action::Edit(ops) => {
                if let Some(id) = id {
                    self.change(c, "appearance.graph.edit", model::edit(&id, ops));
                }
            }
            Action::Set(graph) => {
                if let Some(id) = id {
                    self.change(c, "appearance.graph.set", model::set(&id, &graph));
                }
            }
            Action::Pending(p) => self.pending = Some(p),
            Action::Texture(sha, use_) => self.use_texture(c, &sha, use_),
            Action::AddTexture(use_) => return Some(use_),
            Action::Json(text) => match serde_json::from_str::<Value>(&text) {
                Ok(graph) => {
                    if let Some(id) = id {
                        self.change(c, "appearance.graph.set", json!({"id": id, "graph": graph}));
                    }
                }
                Err(e) => self.note = Some((format("appearance.graph.bad_json", &[("error", &e.to_string())]), true)),
            },
            Action::Name(name) => self.new_name = name,
            Action::Refused(id) => self.note = Some((tr(&id).into(), true)),
        }
        None
    }

    /// Use texture `sha` as `use_` says.
    pub(crate) fn use_texture(&mut self, c: &mut dyn Commands, sha: &str, use_: TextureUse) {
        let Some(loaded) = self.graph().cloned() else { return };
        let texture = Constant::Texture(solidcraft_engine::render::material_graph::graph::TextureRef { texture: sha.into() });
        match use_ {
            TextureUse::File { node, port } => {
                self.change(c, "appearance.graph.edit", model::edit(&loaded.id, vec![model::set_input(&node, &port, &texture)]));
            }
            TextureUse::Interface(mut input) => {
                input.default = Some(texture);
                self.change(c, "appearance.graph.edit", model::edit(&loaded.id, vec![model::set_interface(&input)]));
            }
            TextureUse::Drive { node, port } => {
                let Some(Ok(catalogue)) = &self.catalogue else { return };
                match model::drive_with(&loaded.graph, catalogue, &node, &port, Driver::Image, Some(sha)) {
                    Ok(graph) => {
                        self.change(c, "appearance.graph.set", model::set(&loaded.id, &graph));
                    }
                    Err(e) => self.note = Some((tr(e).into(), true)),
                }
            }
        }
    }

    /// After the file picker: add the texture at `path`, then use it.
    pub(crate) fn add_texture(&mut self, c: &mut dyn Commands, path: &str, use_: TextureUse) {
        if let Some(reply) = self.change(c, "appearance.texture.add", json!({"path": path}))
            && let Some(sha) = reply.get("texture").and_then(Value::as_str)
        {
            self.sync(c);
            self.use_texture(c, sha, use_);
        }
    }

    /// The surface the swatch shows: the graph's base response once validation is built, else
    /// the look the dialog is editing.
    pub(crate) fn preview(&self, look: Option<OpaqueSurface>) -> Option<OpaqueSurface> {
        self.graph().and(self.base).or(look)
    }
}

/// The Material graph section of the Appearance dialog. `look` is the look being edited, which
/// the swatch shows until a graph's base response can be read.
pub(crate) fn section(app: &mut SolveApp, ui: &mut egui::Ui, ed: &mut Editor, look: Option<OpaqueSurface>) {
    let gpu = app.viewport.gpu.is_some();
    let mut actions = Vec::new();
    ed.sync(app);
    // Open by default when the design has a graph to show.
    let open = ed.graph().is_some();
    ui_kit::section(ui, "ap_material_graph", tr("appearance.graph.section"), open, |ui| actions = body(ui, ed, gpu, look));
    for action in actions {
        if let Some(use_) = ed.apply(app, action) {
            pick_texture(app, use_);
        }
    }
    let settled = ui.input(|i| !i.pointer.any_down()) && ui.ctx().memory(|m| m.focused().is_none());
    if settled && ed.pending.is_some() {
        ed.commit(app);
    }
}

/// Ask for an image file, add it as a texture and use it. The dialog's editor gets the result.
fn pick_texture(app: &mut SolveApp, use_: TextureUse) {
    app.pick_file(tr("appearance.graph.texture_files"), &["png", "jpg", "jpeg"], move |app, path| {
        let Some(path) = path else { return };
        let mut editor = match app.dialog.as_mut().map(|d| &mut d.kind) {
            Some(crate::dialogs::Kind::Appearance(k)) => std::mem::take(&mut k.material),
            _ => return,
        };
        editor.add_texture(app, &path, use_);
        if let Some(crate::dialogs::Kind::Appearance(k)) = app.dialog.as_mut().map(|d| &mut d.kind) {
            k.material = editor;
        }
    });
}

/// The section's contents; returns what the user asked for.
pub(crate) fn body(ui: &mut egui::Ui, ed: &Editor, gpu: bool, look: Option<OpaqueSurface>) -> Vec<Action> {
    let mut out = Vec::new();
    graph_chooser(ui, ed, &mut out);
    match &ed.loaded {
        None => {}
        Some(Err(e)) => {
            ui_kit::status_line(ui, e, ui_kit::StatusTone::Warning);
        }
        Some(Ok(loaded)) => {
            preview(ui, ed, gpu, look);
            budget(ui, ed);
            match (&ed.catalogue, model::stack(&loaded.graph)) {
                (Some(Ok(catalogue)), Ok(stack)) => {
                    let view = View { ed, catalogue, graph: &loaded.graph };
                    view.layers(ui, &stack, &mut out);
                    view.layer(ui, &stack, &mut out);
                    view.interface(ui, &mut out);
                }
                (Some(Err(e)), _) => {
                    ui_kit::status_line(ui, e, ui_kit::StatusTone::Warning);
                }
                (None, _) => {}
                (_, Err(reason)) => summary(ui, loaded, reason),
            }
            json_editor(ui, ed, loaded, &mut out);
        }
    }
    if let Some((note, error)) = &ed.note {
        ui_kit::status_line(ui, note, if *error { ui_kit::StatusTone::Error } else { ui_kit::StatusTone::Dim });
    }
    out
}

fn graph_chooser(ui: &mut egui::Ui, ed: &Editor, out: &mut Vec<Action>) {
    match &ed.graphs {
        Some(Ok(graphs)) if !graphs.is_empty() => {
            let labels: Vec<String> =
                graphs.iter().map(|g| format("appearance.graph.entry", &[("name", &g.name), ("uses", &g.uses.to_string())])).collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let mut index = ed.chosen.as_ref().and_then(|id| graphs.iter().position(|g| &g.id == id)).unwrap_or(0);
            let before = index;
            let response = ui_kit::row(ui, tr("appearance.graph.graph"), |ui| ui_kit::dropdown(ui, "ap_graph", &mut index, &refs));
            crate::scenario::publish_handle("appearance:graph", response.rect.center());
            if index != before
                && let Some(g) = graphs.get(index)
            {
                out.push(Action::Choose(g.id.clone()));
            }
        }
        Some(Ok(_)) => {
            ui_kit::text(ui, tr("appearance.graph.none"), ui_kit::Text::Note("graph_none"));
        }
        Some(Err(e)) => {
            ui_kit::status_line(ui, e, ui_kit::StatusTone::Dim);
        }
        None => {}
    }
    let mut name = ed.new_name.clone();
    ui_kit::row(ui, tr("appearance.graph.new_name"), |ui| ui_kit::text_field(ui, &mut name, tr("appearance.graph.new_hint")));
    ui.horizontal_wrapped(|ui| {
        let has = ed.graph().is_some();
        let create = ui_kit::button(ui, tr("appearance.graph.new_graph"), ui_kit::Button::Enabled(!name.trim().is_empty()));
        crate::scenario::publish_handle("appearance:graph_create", create.rect.center());
        if create.clicked() {
            out.push(Action::Create(name.clone()));
        }
        if ui_kit::button(ui, tr("appearance.graph.duplicate_graph"), ui_kit::Button::Enabled(has)).clicked() {
            out.push(Action::Duplicate);
        }
        if ui_kit::button(ui, tr("appearance.graph.delete_graph"), ui_kit::Button::Enabled(has)).clicked() {
            out.push(Action::Delete);
        }
    });
    if name != ed.new_name {
        out.push(Action::Name(name));
    }
}

/// The live swatch, or why there is none.
fn preview(ui: &mut egui::Ui, ed: &Editor, gpu: bool, look: Option<OpaqueSurface>) {
    let Some(surface) = ed.preview(look) else { return };
    if !gpu {
        ui_kit::status_line(ui, tr("appearance.graph.no_gpu"), ui_kit::StatusTone::Dim);
        return;
    }
    let t = Tokens::get();
    let width = ui.available_width().max(t.control_height);
    let height = (width * 0.45).clamp(t.control_height * 2.0, t.control_height * 5.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    crate::scenario::publish_rect("appearance:graph_swatch", rect);
    swatch::paint(ui, ui.id().with("graph_swatch"), rect, surface);
    ui.painter().rect_stroke(rect, t.control_radius, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
}

/// The cost against the budget, and validation's findings.
fn budget(ui: &mut egui::Ui, ed: &Editor) {
    let Some(loaded) = ed.graph() else { return };
    let (cost, samples, textures) = match &ed.validation {
        Some(Ok(v)) => (v.cost, Some(v.samples), Some(v.textures)),
        _ => (loaded.cost, None, None),
    };
    let text = format("appearance.graph.cost", &[("cost", &cost.to_string()), ("budget", &limits::COST_UNITS.to_string())]);
    ui_kit::row(ui, tr("appearance.graph.budget"), |ui| ui_kit::text(ui, &text, ui_kit::Text::MonoCell));
    ui_kit::progress(ui, "appearance_graph_cost", model::fraction(cost, limits::COST_UNITS));
    if let (Some(samples), Some(textures)) = (samples, textures) {
        let text = format(
            "appearance.graph.samples",
            &[
                ("samples", &samples.to_string()),
                ("samples_budget", &limits::TEXTURE_SAMPLES.to_string()),
                ("textures", &textures.to_string()),
                ("textures_budget", &limits::TEXTURES_PER_GRAPH.to_string()),
            ],
        );
        ui_kit::text(ui, &text, ui_kit::Text::Note("graph_samples"));
    }
    match &ed.validation {
        Some(Ok(v)) => {
            for issue in v.errors.iter().take(8) {
                let at = [issue.node.as_deref(), issue.port.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(".");
                let line = if at.is_empty() { issue.message.clone() } else { format!("{at}: {}", issue.message) };
                ui_kit::status_line(ui, &line, ui_kit::StatusTone::Error);
            }
        }
        Some(Err(e)) => {
            ui_kit::status_line(ui, e, ui_kit::StatusTone::Dim);
        }
        None => {}
    }
}

/// A read-only account of a graph the stack cannot show.
fn summary(ui: &mut egui::Ui, loaded: &Loaded, reason: &str) {
    ui_kit::status_line(ui, tr(reason), ui_kit::StatusTone::Warning);
    let textures = loaded.graph.nodes.iter().filter(|n| n.kind == "image").count();
    let text = format(
        "appearance.graph.summary",
        &[("nodes", &loaded.graph.nodes.len().to_string()), ("cost", &loaded.cost.to_string()), ("textures", &textures.to_string())],
    );
    ui_kit::text(ui, &text, ui_kit::Text::Note("graph_summary"));
}

/// "Edit as JSON": the whole graph, replaced with `appearance.graph.set`.
fn json_editor(ui: &mut egui::Ui, ed: &Editor, loaded: &Loaded, out: &mut Vec<Action>) {
    ui_kit::section(ui, "ap_graph_json", tr("appearance.graph.json"), false, |ui| {
        let id = egui::Id::new(("ap_graph_json", &loaded.id, ed.synced));
        let mut text = ui.data_mut(|d| d.get_temp_mut_or_insert_with(id, || serde_json::to_string_pretty(&loaded.graph).unwrap_or_default()).clone());
        ui_kit::text_area(ui, tr("appearance.graph.json"), id.with("text"), &mut text, 12, JSON_LIMIT);
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
        if ui_kit::button(ui, tr("appearance.graph.replace"), ui_kit::Button::Normal).clicked() {
            out.push(Action::Json(text));
        }
    });
}

/// A frame's read of one stack-shaped graph.
struct View<'a> {
    ed: &'a Editor,
    catalogue: &'a Catalogue,
    graph: &'a MaterialGraph,
}

/// A port's label: the catalogue name, translated where the catalogs have it.
fn port_label(port: &str) -> String {
    let id = format!("appearance.graph.port.{port}");
    let text = tr(&id);
    if text == id { port.replace('_', " ") } else { text.to_string() }
}

fn kind_label(kind: &str) -> String {
    let id = format!("appearance.graph.kind.{kind}");
    let text = tr(&id);
    if text == id { kind.to_string() } else { text.to_string() }
}

fn option_label(option: &str) -> String {
    let id = format!("appearance.graph.option.{option}");
    let text = tr(&id);
    if text == id { option.replace('_', " ") } else { text.to_string() }
}

fn unit_suffix(unit: Unit) -> &'static str {
    match unit {
        Unit::None => "",
        Unit::Mm => " mm",
        Unit::PerMm => " /mm",
        Unit::Degrees => "°",
    }
}

impl View<'_> {
    /// The layer list, top first, with reorder and remove, and Add layer.
    fn layers(&self, ui: &mut egui::Ui, stack: &Stack, out: &mut Vec<Action>) {
        let count = stack.layers.len();
        ui_kit::section_count(ui, "ap_graph_layers", tr("appearance.graph.layers"), true, Some(count), |ui| {
            for (i, layer) in stack.layers.iter().enumerate().rev() {
                let name = if i == 0 {
                    tr("appearance.graph.base").to_string()
                } else {
                    format("appearance.graph.layer", &[("n", &i.to_string()), ("node", &layer.surface)])
                };
                ui.horizontal(|ui| {
                    let t = Tokens::get();
                    let actions = 3.0 * t.form_close_size;
                    let width = (ui.available_width() - actions).max(t.control_height);
                    let picked = ui
                        .allocate_ui(egui::vec2(width, ui_kit::row_height()), |ui| {
                            ui_kit::list_picker(ui, &format!("graph_layer:{i}"), &name, self.ed.layer == i)
                        })
                        .inner;
                    crate::scenario::publish_handle(&format!("appearance:graph_layer:{i}"), picked.rect.center());
                    if picked.clicked() {
                        out.push(Action::SelectLayer(i));
                    }
                    let masked = layer.mix.is_some();
                    let up = ui_kit::compact_button(ui, ui_kit::MOVE_UP, tr("appearance.graph.raise"), masked && i + 1 < count, false);
                    let down = ui_kit::compact_button(ui, ui_kit::MOVE_DOWN, tr("appearance.graph.lower"), masked && i > 1, false);
                    let remove = ui_kit::compact_button(ui, "×", tr("appearance.graph.remove_layer"), masked, false);
                    crate::scenario::publish_handle(&format!("appearance:graph_layer_remove:{i}"), remove.rect.center());
                    let swap = |at: usize, out: &mut Vec<Action>| match model::swap_layers(self.graph, stack, at) {
                        Ok(g) => out.push(Action::Set(g)),
                        Err(e) => out.push(Action::Refused(e.into())),
                    };
                    if up.clicked() {
                        swap(i, out);
                        out.push(Action::SelectLayer(i + 1));
                    }
                    if down.clicked() {
                        swap(i - 1, out);
                        out.push(Action::SelectLayer(i - 1));
                    }
                    if remove.clicked() {
                        match model::remove_layer(self.graph, stack, i) {
                            Ok(g) => {
                                out.push(Action::Set(g));
                                out.push(Action::SelectLayer(i - 1));
                            }
                            Err(e) => out.push(Action::Refused(e.into())),
                        }
                    }
                });
            }
            let add = ui_kit::button(ui, tr("appearance.graph.add_layer"), ui_kit::Button::Normal);
            crate::scenario::publish_handle("appearance:graph_add_layer", add.rect.center());
            if add.clicked() {
                match model::add_layer(self.graph, stack) {
                    Ok(g) => {
                        out.push(Action::Set(g));
                        out.push(Action::SelectLayer(count));
                    }
                    Err(e) => out.push(Action::Refused(e.into())),
                }
            }
        });
    }

    /// The selected layer: its mask, then its surface inputs by group.
    fn layer(&self, ui: &mut egui::Ui, stack: &Stack, out: &mut Vec<Action>) {
        let Some(layer) = stack.layers.get(self.ed.layer).or_else(|| stack.layers.first()) else { return };
        let Some(surface) = model::node(self.graph, &layer.surface) else { return };
        if let Some(mix) = layer.mix.as_deref().and_then(|id| model::node(self.graph, id))
            && let Some(spec) = self.catalogue.port(&mix.kind, "mix")
        {
            ui_kit::divider(ui, "graph_mask");
            self.port(ui, mix, spec, Some(tr("appearance.graph.mask")), true, out);
        }
        let Some(kind) = self.catalogue.kind(&surface.kind) else { return };
        for (group, ports) in model::GROUPS {
            let specs: Vec<&PortSpec> = ports.iter().filter_map(|p| kind.inputs.iter().find(|s| s.name == *p)).collect();
            let used = specs.iter().any(|s| surface.inputs.contains_key(&s.name));
            let open = used || matches!(*group, "appearance.graph.group_base" | "appearance.graph.group_specular");
            ui_kit::section(ui, (*group, &layer.surface), tr(group), open, |ui| {
                for spec in specs {
                    self.port(ui, surface, spec, None, true, out);
                }
            });
        }
    }

    /// The interface inputs a look can override, with their defaults.
    fn interface(&self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        if self.graph.interface.is_empty() {
            return;
        }
        ui_kit::section_count(ui, "ap_graph_interface", tr("appearance.graph.interface"), false, Some(self.graph.interface.len()), |ui| {
            for input in &self.graph.interface {
                let shown = match &self.ed.pending {
                    Some(Pending::Interface(p)) if p.name == input.name => p,
                    _ => input,
                };
                let spec = PortSpec { name: input.name.clone(), ty: input.ty, range: None, default: model::PortDefault::Float(0.0) };
                ui_kit::row(ui, &input.name, |ui| {
                    if input.ty == PortType::Texture {
                        let current = match &shown.default {
                            Some(Constant::Texture(t)) => Some(t.texture.as_str()),
                            _ => None,
                        };
                        self.texture_picker(ui, &input.name, current, TextureUse::Interface(input.clone()), out);
                    } else if let Some(value) = self.value_widget(ui, &spec, shown.default.as_ref()) {
                        out.push(Action::Pending(Pending::Interface(InterfaceInput { default: Some(value), ..input.clone() })));
                    }
                });
            }
        });
    }

    /// One input: its value (or what drives it), the Drive menu, and for a wired input the
    /// nodes upstream with their own values.
    fn port(&self, ui: &mut egui::Ui, n: &Node, spec: &PortSpec, label: Option<&str>, drivable: bool, out: &mut Vec<Action>) {
        let label = label.map_or_else(|| port_label(&spec.name), str::to_string);
        let handle = format!("appearance:graph_port:{}:{}", n.id, spec.name);
        match model::drive(n, &spec.name) {
            Drive::Wired(connection) => {
                let chain = model::upstream(self.graph, connection.source().0);
                let path = chain.iter().rev().map(|c| kind_label(&c.kind)).collect::<Vec<_>>().join(" → ");
                let response = ui_kit::row(ui, &label, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let remove = ui_kit::compact_button(ui, "×", tr("appearance.graph.undrive"), drivable, false);
                        crate::scenario::publish_handle(&format!("{handle}:undrive"), remove.rect.center());
                        if remove.clicked() {
                            match model::undrive(self.graph, &n.id, &spec.name, connection.fallback.clone()) {
                                Ok(g) => out.push(Action::Set(g)),
                                Err(e) => out.push(Action::Refused(e.into())),
                            }
                        }
                        ui_kit::text(ui, &path, ui_kit::Text::Inline);
                    });
                    ui.max_rect()
                });
                crate::scenario::publish_handle(&handle, response.center());
                ui.indent((&n.id, &spec.name, "chain"), |ui| {
                    for node in chain.iter().rev() {
                        self.driver_node(ui, n, spec, node, out);
                    }
                });
            }
            Drive::Interface(name) => {
                ui_kit::row(ui, &label, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui_kit::compact_button(ui, "×", tr("appearance.graph.fix_value"), true, false).clicked() {
                            match model::undrive(self.graph, &n.id, &spec.name, None) {
                                Ok(g) => out.push(Action::Set(g)),
                                Err(e) => out.push(Action::Refused(e.into())),
                            }
                        }
                        ui_kit::text(ui, &format("appearance.graph.from_look", &[("name", name)]), ui_kit::Text::Inline);
                    });
                });
            }
            Drive::Constant(_) | Drive::Default => {
                let response = ui_kit::row(ui, &label, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if drivable && can_drive(spec) {
                            self.drive_menu(ui, n, spec, out);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if spec.ty == PortType::Texture {
                                let current = match model::shown_constant(n, &spec.name) {
                                    Some(Constant::Texture(t)) => Some(t.texture.as_str()),
                                    _ => None,
                                };
                                let use_ = TextureUse::File { node: n.id.clone(), port: spec.name.clone() };
                                self.texture_picker(ui, &format!("{}:{}", n.id, spec.name), current, use_, out);
                                return;
                            }
                            let shown = match &self.ed.pending {
                                Some(Pending::Input { node, port, value }) if *node == n.id && *port == spec.name => Some(value),
                                _ => model::shown_constant(n, &spec.name),
                            };
                            if let Some(value) = self.value_widget(ui, spec, shown) {
                                out.push(Action::Pending(Pending::Input { node: n.id.clone(), port: spec.name.clone(), value }));
                            }
                        });
                    });
                    ui.max_rect()
                });
                crate::scenario::publish_handle(&handle, response.center());
            }
        }
    }

    /// One node of a driver chain: its params and its constant inputs.
    fn driver_node(&self, ui: &mut egui::Ui, owner: &Node, owner_port: &PortSpec, n: &Node, out: &mut Vec<Action>) {
        let Some(kind) = self.catalogue.kind(&n.kind) else {
            ui_kit::status_line(ui, &format("appearance.graph.unknown_kind", &[("kind", &n.kind)]), ui_kit::StatusTone::Warning);
            return;
        };
        let shared = model::readers(self.graph, &n.id) > 1;
        let title = if shared { format("appearance.graph.shared", &[("kind", &kind_label(&n.kind))]) } else { kind_label(&n.kind) };
        let open = n.kind.starts_with("pattern.") || n.kind == "image" || n.kind == "remap";
        ui_kit::section(ui, (&owner.id, &owner_port.name, &n.id), &title, open, |ui| {
            self.params(ui, n, kind, out);
            for spec in &kind.inputs {
                let wired = matches!(n.inputs.get(&spec.name), Some(solidcraft_engine::render::material_graph::InputValue::Connection(_)));
                if wired || matches!(spec.ty, PortType::Texcoord | PortType::Surface | PortType::Normal) {
                    continue;
                }
                // Optional inputs of a pattern (a noise's position) stay unset in the stack.
                if spec.default == model::PortDefault::Optional {
                    continue;
                }
                self.port(ui, n, spec, None, false, out);
            }
        });
    }

    fn params(&self, ui: &mut egui::Ui, n: &Node, kind: &KindSpec, out: &mut Vec<Action>) {
        for spec in &kind.params {
            let current = match &self.ed.pending {
                Some(Pending::Param { node, param, value }) if *node == n.id && *param == spec.name => value.clone(),
                _ => model::param_value(n, spec),
            };
            let label = format!("appearance.graph.param.{}", spec.name);
            let label = if tr(&label) == label { spec.name.replace('_', " ") } else { tr(&label).to_string() };
            match &spec.kind {
                ParamKind::Choice { options, .. } => {
                    let labels: Vec<String> = options.iter().map(|o| option_label(o)).collect();
                    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                    let mut index = match &current {
                        ParamValue::Text(t) => options.iter().position(|o| o == t).unwrap_or(0),
                        ParamValue::Int(_) => 0,
                    };
                    let before = index;
                    ui_kit::row(ui, &label, |ui| ui_kit::dropdown(ui, &format!("ap_graph_param:{}:{}", n.id, spec.name), &mut index, &refs));
                    if index != before
                        && let Some(option) = options.get(index)
                    {
                        out.push(Action::Edit(vec![model::set_param(&n.id, &spec.name, &ParamValue::Text(option.clone()))]));
                    }
                }
                ParamKind::Int { min, max, .. } => {
                    let mut value = match current {
                        ParamValue::Int(i) => i,
                        ParamValue::Text(_) => *min,
                    };
                    let response = ui_kit::row(ui, &label, |ui| ui_kit::number_field(ui, &mut value, *min..=*max, 0.05, ""));
                    if response.changed() {
                        out.push(Action::Pending(Pending::Param { node: n.id.clone(), param: spec.name.clone(), value: ParamValue::Int(value) }));
                    }
                }
            }
        }
    }

    /// The field for a constant of `spec`'s type; the new value when it changed this frame.
    fn value_widget(&self, ui: &mut egui::Ui, spec: &PortSpec, shown: Option<&Constant>) -> Option<Constant> {
        let width = model::width(spec.ty);
        if width == 0 {
            let text = match spec.default {
                model::PortDefault::Geometric => tr("appearance.graph.geometric"),
                _ => tr("appearance.graph.unset"),
            };
            ui_kit::text(ui, text, ui_kit::Text::Inline);
            return None;
        }
        let mut values = model::value_of(shown, spec, width);
        let unit = model::unit(&spec.name);
        let (lo, hi) = spec.range.unwrap_or((-1e6, 1e6));
        let changed = match (spec.ty, values.as_mut_slice()) {
            (PortType::Color3, _) => {
                let mut rgb = [values.first().copied().unwrap_or(0.0), values.get(1).copied().unwrap_or(0.0), values.get(2).copied().unwrap_or(0.0)];
                let changed = ui_kit::linear_color_field(ui, &mut rgb, hi.max(1.0)).changed();
                values = rgb.to_vec();
                changed
            }
            (PortType::Int, [x]) => {
                let mut i = x.round() as i64;
                let changed = ui_kit::number_field(ui, &mut i, lo as i64..=hi as i64, 1.0, "").changed();
                *x = i as f64;
                changed
            }
            (_, [x]) if unit == Unit::Degrees => {
                let mut degrees = x.to_degrees();
                let changed = ui_kit::slider(ui, &mut degrees, lo.to_degrees()..=hi.to_degrees(), Default::default()).changed();
                *x = degrees.to_radians();
                changed
            }
            (_, [x]) if hi - lo <= 10.0 => ui_kit::slider(ui, x, lo..=hi, Default::default()).changed(),
            (_, [x]) => ui_kit::number_field(ui, x, lo..=hi, speed(*x, lo, hi), unit_suffix(unit)).changed(),
            (_, many) => {
                let speed = speed(many.iter().fold(0.0_f64, |a, b| a.max(b.abs())), lo, hi);
                ui_kit::number_fields(ui, many, lo..=hi, speed, unit_suffix(unit)).iter().any(egui::Response::changed)
            }
        };
        changed.then(|| model::constant(&values, spec.range))
    }

    /// The textures in the design, and Add texture.
    fn texture_picker(&self, ui: &mut egui::Ui, key: &str, current: Option<&str>, use_: TextureUse, out: &mut Vec<Action>) {
        let textures: &[TextureEntry] = match &self.ed.textures {
            Some(Ok(t)) => t,
            _ => &[],
        };
        let mut labels = vec![tr("appearance.graph.no_texture").to_string()];
        labels.extend(textures.iter().map(|t| format!("{} · {}×{}", t.name, t.width, t.height)));
        labels.push(tr("appearance.graph.add_texture").to_string());
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut index = current.and_then(|sha| textures.iter().position(|t| t.texture == sha)).map_or(0, |i| i + 1);
        let before = index;
        let response = ui_kit::dropdown(ui, &format!("ap_graph_texture:{key}"), &mut index, &refs);
        crate::scenario::publish_handle(&format!("appearance:graph_texture:{key}"), response.rect.center());
        if index == before {
            return;
        }
        match textures.get(index.wrapping_sub(1)) {
            Some(t) => out.push(Action::Texture(t.texture.clone(), use_)),
            None if index == labels.len() - 1 => out.push(Action::AddTexture(use_)),
            None => {}
        }
    }

    /// The Drive menu: a pattern (or an image) feeding this input.
    fn drive_menu(&self, ui: &mut egui::Ui, n: &Node, spec: &PortSpec, out: &mut Vec<Action>) {
        let menu = ui_kit::action_menu(ui, "+", |ui| {
            ui_kit::text(ui, tr("appearance.graph.drive"), ui_kit::Text::Heading("graph_drive"));
            for driver in Driver::ALL {
                if spec.ty == PortType::Vector3 && driver != Driver::Brushed {
                    continue;
                }
                let cost = self.catalogue.kind(driver.kind()).map_or(0, |k| k.cost);
                let response = ui_kit::button(ui, kind_label(driver.kind()), ui_kit::Button::Normal)
                    .on_hover_text(format("appearance.graph.cost_hint", &[("cost", &cost.to_string())]));
                crate::scenario::publish_handle(&format!("appearance:graph_drive:{}:{}:{}", n.id, spec.name, driver.kind()), response.rect.center());
                if !response.clicked() {
                    continue;
                }
                ui.close();
                if driver == Driver::Image {
                    let use_ = TextureUse::Drive { node: n.id.clone(), port: spec.name.clone() };
                    match self.ed.textures.as_ref().and_then(|t| t.as_ref().ok()).and_then(|t| t.first()) {
                        Some(t) => out.push(Action::Texture(t.texture.clone(), use_)),
                        None => out.push(Action::AddTexture(use_)),
                    }
                    continue;
                }
                match model::drive_with(self.graph, self.catalogue, &n.id, &spec.name, driver, None) {
                    Ok(g) => out.push(Action::Set(g)),
                    Err(e) => out.push(Action::Refused(e.into())),
                }
            }
        });
        crate::scenario::publish_handle(&format!("appearance:graph_drive:{}:{}", n.id, spec.name), menu.response.rect.center());
        menu.response.on_hover_text(tr("appearance.graph.drive"));
    }
}

/// Whether the Drive menu offers a pattern for `spec`.
fn can_drive(spec: &PortSpec) -> bool {
    match spec.ty {
        PortType::Float | PortType::Color3 | PortType::Normal => true,
        PortType::Vector3 => spec.default == model::PortDefault::Geometric,
        _ => false,
    }
}

/// A drag speed for a number of size `x` within `lo..=hi`.
fn speed(x: f64, lo: f64, hi: f64) -> f64 {
    let scale = if x.is_finite() && x.abs() > 1e-9 { x.abs() } else { (hi - lo).abs().min(1.0) };
    (scale * 0.01).clamp(1e-4, 100.0)
}
