//! The studio library (`render.studio.*`): studios as data, one mechanism for the Realistic view,
//! the GPU path tracer's studio and the native studio compositions (`render.preset.*`).
//!
//! - **Built-in studios** ship as data (`assets/render/catalog/studios.json`), read-only: they
//!   can be listed, read, duplicated and exported, never edited or deleted.
//! - **User studios** live in the session ([`StudioLibrary`]); the host keeps them between runs
//!   (the desktop app in its preferences, as it keeps settings profiles) and loads them back with
//!   `render.studio.import`'s document or by setting [`StudioLibrary::user`]. They are
//!   duplicated from another studio, edited, imported and exported as `solidcraft-studios/1`
//!   files, and deleted.
//! - **The Realistic default** is a setting naming a studio ([`StudioLibrary::realistic_default`],
//!   `render.studio.default`); without one it is [`DEFAULT_REALISTIC`].
//!
//! None of these commands changes the design: they are not undone, and a design is never marked
//! changed by them.

use crate::schema::TypedContract;
use crate::schema::render_studios::{
    StudioDefaultParams, StudioDefaultResult, StudioDeleteParams, StudioDeleted, StudioDuplicateParams, StudioEditParams, StudioExportParams,
    StudioExported, StudioGetParams, StudioGetResult, StudioImportParams, StudioImported, StudioListParams, StudioListResult, StudioSummary,
};
use crate::{Result, Session, params::bad};
use serde_json::{Value, json};
use solidcraft_viewport::studio::{Studio, StudioError, valid_id};
use std::sync::OnceLock;

/// The format tag of a studio file and of the built-in catalog.
pub const FORMAT: &str = "solidcraft-studios/1";
/// The Realistic view's studio when no setting names one: Product (`STU-002`).
pub const DEFAULT_REALISTIC: &str = "NATIVE-STUDIO-026";
/// Most user studios a session keeps.
pub const MAX_USER_STUDIOS: usize = 256;
/// Most bytes read from a studio file.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

pub(super) const LIST_CONTRACT: TypedContract<StudioListParams> = TypedContract::new();
pub(super) const GET_CONTRACT: TypedContract<StudioGetParams> = TypedContract::new();
pub(super) const DUPLICATE_CONTRACT: TypedContract<StudioDuplicateParams> = TypedContract::new();
pub(super) const EDIT_CONTRACT: TypedContract<StudioEditParams> = TypedContract::new();
pub(super) const DELETE_CONTRACT: TypedContract<StudioDeleteParams> = TypedContract::new();
pub(super) const IMPORT_CONTRACT: TypedContract<StudioImportParams> = TypedContract::new();
pub(super) const EXPORT_CONTRACT: TypedContract<StudioExportParams> = TypedContract::new();
pub(super) const DEFAULT_CONTRACT: TypedContract<StudioDefaultParams> = TypedContract::new();

/// The session's studios beyond the built-in ones, and the Realistic default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StudioLibrary {
    /// User studios, in the order they were made.
    pub user: Vec<Studio>,
    /// The studio the Realistic view lights with; `None`: [`DEFAULT_REALISTIC`].
    pub realistic_default: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StudioFile {
    schema: String,
    #[serde(default)]
    #[allow(dead_code)]
    license: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    units: Option<Value>,
    items: Vec<Studio>,
}

fn parse_file(text: &str) -> std::result::Result<Vec<Studio>, String> {
    let file: StudioFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if file.schema != FORMAT {
        return Err(format!("a studio file is `{FORMAT}`, not `{}`", file.schema));
    }
    for studio in &file.items {
        studio.validate().map_err(|e| format!("{}: {e}", studio.id))?;
    }
    Ok(file.items)
}

/// The built-in studios, read once from the catalog.
pub fn builtin() -> &'static [Studio] {
    static BUILTIN: OnceLock<Vec<Studio>> = OnceLock::new();
    BUILTIN.get_or_init(|| super::library_data::catalog("studios").and_then(|text| parse_file(text).ok()).unwrap_or_default())
}

impl StudioLibrary {
    /// The studio named `id`: built-in, then the user's.
    pub fn get(&self, id: &str) -> Option<&Studio> {
        builtin().iter().find(|s| s.id == id).or_else(|| self.user.iter().find(|s| s.id == id))
    }

    /// The Realistic view's studio: the default setting's, else [`DEFAULT_REALISTIC`]; a setting
    /// naming a studio that is gone or has no rig falls back to it too.
    pub fn realistic(&self) -> Option<&Studio> {
        self.realistic_default.as_deref().and_then(|id| self.get(id)).filter(|s| s.realistic.is_some()).or_else(|| self.get(DEFAULT_REALISTIC))
    }

    fn is_builtin(id: &str) -> bool {
        builtin().iter().any(|s| s.id == id)
    }

    /// A user id that no studio has yet: `USER-STUDIO-<n>`.
    fn fresh_id(&self) -> String {
        (1..).map(|n| format!("USER-STUDIO-{n}")).find(|id| self.get(id).is_none()).unwrap_or_default()
    }

    fn user_mut(&mut self, id: &str, command: &str) -> Result<&mut Studio> {
        if Self::is_builtin(id) {
            return Err(bad(command, format!("{id} is a built-in studio; duplicate it to change a copy")));
        }
        self.user.iter_mut().find(|s| s.id == id).ok_or_else(|| bad(command, format!("unknown studio {id}")))
    }
}

fn studio_error(command: &str, e: StudioError) -> crate::EngineError {
    bad(command, e.to_string())
}

fn reply(command: &str, value: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| bad(command, e.to_string()))
}

fn summary(s: &Studio) -> StudioSummary {
    StudioSummary {
        id: s.id.clone(),
        name: s.name.clone(),
        description: s.description.clone(),
        source: s.source.clone(),
        builtin: StudioLibrary::is_builtin(&s.id),
        realistic: s.realistic.is_some(),
        path: s.path.is_some(),
    }
}

pub(super) fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.list";
    let input: StudioListParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    let studios = builtin()
        .iter()
        .chain(&s.studios.user)
        .filter(|st| match input.kind.as_deref() {
            Some("realistic") => st.realistic.is_some(),
            Some("path") => st.path.is_some(),
            _ => true,
        })
        .map(summary)
        .collect();
    let realistic_default = s.studios.realistic().map(|st| st.id.clone()).unwrap_or_default();
    reply(command, StudioListResult { studios, realistic_default })
}

pub(super) fn get(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.get";
    let input: StudioGetParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    let studio = s.studios.get(&input.id).ok_or_else(|| bad(command, format!("unknown studio {}", input.id)))?;
    reply(command, StudioGetResult { builtin: StudioLibrary::is_builtin(&studio.id), studio: serde_json::to_value(studio).unwrap_or_default() })
}

pub(super) fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.duplicate";
    let input: StudioDuplicateParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    let mut copy = s.studios.get(&input.id).cloned().ok_or_else(|| bad(command, format!("unknown studio {}", input.id)))?;
    if s.studios.user.len() >= MAX_USER_STUDIOS {
        return Err(bad(command, format!("at most {MAX_USER_STUDIOS} user studios; delete one first")));
    }
    let id = match input.new_id {
        Some(id) if !valid_id(&id) => return Err(bad(command, "a studio id is 1 to 128 ASCII letters, digits, hyphens or underscores")),
        Some(id) if s.studios.get(&id).is_some() => return Err(bad(command, format!("a studio {id} already exists"))),
        Some(id) => id,
        None => s.studios.fresh_id(),
    };
    copy.source = Some(copy.id.clone());
    copy.name = input.name.unwrap_or_else(|| format!("{} copy", copy.name));
    copy.id = id;
    copy.validate().map_err(|e| studio_error(command, e))?;
    s.studios.user.push(copy.clone());
    reply(command, StudioGetResult { builtin: false, studio: serde_json::to_value(&copy).unwrap_or_default() })
}

pub(super) fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.edit";
    let input: StudioEditParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    let studio = s.studios.user_mut(&input.id, command)?;
    let mut edited = studio.clone();
    if let Some(name) = input.name {
        edited.name = name;
    }
    if let Some(description) = input.description {
        edited.description = description;
    }
    if let Some(realistic) = input.realistic {
        edited.realistic = serde_json::from_value(realistic).map_err(|e| bad(command, format!("realistic: {e}")))?;
    }
    if let Some(path) = input.path {
        edited.path = serde_json::from_value(path).map_err(|e| bad(command, format!("path: {e}")))?;
    }
    // Checked whole before it replaces the studio: a refused edit changes nothing.
    edited.validate().map_err(|e| studio_error(command, e))?;
    *studio = edited.clone();
    reply(command, StudioGetResult { builtin: false, studio: serde_json::to_value(&edited).unwrap_or_default() })
}

pub(super) fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.delete";
    let input: StudioDeleteParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    s.studios.user_mut(&input.id, command)?;
    s.studios.user.retain(|st| st.id != input.id);
    if s.studios.realistic_default.as_deref() == Some(input.id.as_str()) {
        s.studios.realistic_default = None;
    }
    reply(command, StudioDeleted { deleted: input.id, realistic_default: s.studios.realistic().map(|st| st.id.clone()).unwrap_or_default() })
}

fn path_check(command: &str, path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(bad(command, "invalid studio file path"));
    }
    solidcraft_io::vfs::check(path).map_err(|e| bad(command, e.to_string()))
}

pub(super) fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.import";
    let input: StudioImportParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    let items = match (&input.path, input.document) {
        (Some(path), None) => {
            path_check(command, path)?;
            let bytes = solidcraft_io::vfs::read(path, MAX_FILE_BYTES).map_err(|e| bad(command, format!("could not read studio file: {e}")))?;
            let text = String::from_utf8(bytes).map_err(|_| bad(command, "a studio file is UTF-8 JSON"))?;
            parse_file(&text).map_err(|e| bad(command, e))?
        }
        (None, Some(document)) => parse_file(&document.to_string()).map_err(|e| bad(command, e))?,
        _ => return Err(bad(command, "give either path or document")),
    };
    // Checked whole first: an import that would fail adds nothing.
    let mut next = s.studios.user.clone();
    for studio in &items {
        if StudioLibrary::is_builtin(&studio.id) {
            return Err(bad(command, format!("{} is a built-in studio's id; give the imported studio another", studio.id)));
        }
        match next.iter_mut().find(|st| st.id == studio.id) {
            Some(existing) if input.replace => *existing = studio.clone(),
            Some(_) => return Err(bad(command, format!("a studio {} already exists; import with replace: true to replace it", studio.id))),
            None => next.push(studio.clone()),
        }
    }
    if next.len() > MAX_USER_STUDIOS {
        return Err(bad(command, format!("at most {MAX_USER_STUDIOS} user studios")));
    }
    s.studios.user = next;
    reply(command, StudioImported { imported: items.iter().map(|st| st.id.clone()).collect() })
}

/// The studio a GPU job names: the library's (built-in, then the user's), else the one the design
/// keeps (`RenderSettings::studios`).
pub fn resolve<'a>(s: &'a Session, id: &str) -> Option<&'a Studio> {
    s.studios.get(id).or_else(|| s.doc.render.studios.get(id))
}

/// A GPU job's lighting and environment for its studio `choice`, about the subject `bounds`.
pub(super) fn job_lighting(
    s: &Session,
    choice: &solidcraft_doc::render_job::StudioChoice,
    bounds: solidcraft_geom::Aabb3,
    command: &str,
) -> Result<(solidcraft_render::job::StudioLighting, std::sync::Arc<solidcraft_render::lighting::environment::EnvironmentSource>)> {
    choice.validate().map_err(|e| bad(command, e.to_string()))?;
    let studio = resolve(s, &choice.id).ok_or_else(|| bad(command, format!("unknown studio {}", choice.id)))?;
    let rig = studio.realistic.as_ref().ok_or_else(|| bad(command, format!("{} has no rig to light a job with", choice.id)))?;
    if bounds.is_empty() {
        return Err(bad(command, "a studio needs a nonempty visible subject"));
    }
    let (lo, hi) = (bounds.min, bounds.max);
    solidcraft_render::job::studio::lighting(rig, [lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z], choice.rotation_rad)
        .map_err(|e| bad(command, e.to_string()))
}

/// Keep in `settings` the records of the user studios its GPU setups name, and only those: a
/// design that names a user studio renders the same on another machine, and one that no longer
/// does carries none. Built-in studios are never copied. A setup naming a studio neither the
/// library nor the design has is refused.
pub fn keep_used(s: &Session, settings: &mut solidcraft_doc::render_settings::RenderSettings, command: &str) -> Result<()> {
    let mut kept = std::collections::BTreeMap::new();
    for setup in settings.setups.values() {
        let Some(solidcraft_doc::render_settings::RenderJob::GpuPath { options, .. }) = &setup.job else { continue };
        let Some(choice) = &options.studio else { continue };
        if StudioLibrary::is_builtin(&choice.id) {
            continue;
        }
        let studio = s
            .studios
            .get(&choice.id)
            .or_else(|| settings.studios.get(&choice.id))
            .cloned()
            .ok_or_else(|| bad(command, format!("unknown studio {}", choice.id)))?;
        kept.insert(choice.id.clone(), studio);
    }
    settings.studios = kept;
    Ok(())
}

/// A `solidcraft-studios/1` document of `studios`.
pub fn document(studios: &[&Studio]) -> Value {
    json!({"schema": FORMAT, "items": studios})
}

pub(super) fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.export";
    let input: StudioExportParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    path_check(command, &input.path)?;
    if !input.path.to_ascii_lowercase().ends_with(".json") {
        return Err(bad(command, "a studio file ends in .json"));
    }
    let studios: Vec<&Studio> =
        input.ids.iter().map(|id| s.studios.get(id).ok_or_else(|| bad(command, format!("unknown studio {id}")))).collect::<Result<_>>()?;
    if studios.is_empty() {
        return Err(bad(command, "name at least one studio to export"));
    }
    let bytes = serde_json::to_vec_pretty(&document(&studios)).map_err(|e| bad(command, e.to_string()))?;
    #[cfg(not(target_arch = "wasm32"))]
    let result = if input.overwrite {
        solidcraft_io::write_atomic(std::path::Path::new(&input.path), &bytes, false)
    } else {
        solidcraft_io::safe_write::create_atomic(std::path::Path::new(&input.path), &bytes)
    };
    #[cfg(target_arch = "wasm32")]
    let result = if !input.overwrite && solidcraft_io::vfs::exists(&input.path) {
        Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "studio file already exists"))
    } else {
        solidcraft_io::vfs::write(&input.path, &bytes)
    };
    result.map_err(|e| bad(command, format!("could not export studios: {e}")))?;
    reply(command, StudioExported { path: input.path, count: studios.len(), bytes: bytes.len() })
}

pub(super) fn default(s: &mut Session, p: &Value) -> Result<Value> {
    let command = "render.studio.default";
    let input: StudioDefaultParams = serde_json::from_value(p.clone()).map_err(|e| bad(command, e.to_string()))?;
    if let Some(id) = input.id {
        let studio = s.studios.get(&id).ok_or_else(|| bad(command, format!("unknown studio {id}")))?;
        if studio.realistic.is_none() {
            return Err(bad(command, format!("{id} has no rig for the Realistic view")));
        }
        s.studios.realistic_default = (id != DEFAULT_REALISTIC).then_some(id);
    }
    reply(command, StudioDefaultResult { id: s.studios.realistic().map(|st| st.id.clone()).unwrap_or_default() })
}

#[cfg(test)]
#[path = "studios_tests.rs"]
mod tests;
