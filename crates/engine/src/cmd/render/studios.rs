//! Built-in studios shared by native compositions and the viewport.
use solidcraft_viewport::studio::{Studio, StudioRig};

/// The original Product rig, derived from STU-002.
pub const DEFAULT_REALISTIC: &str = "NATIVE-STUDIO-026";

pub(super) fn try_builtin() -> Result<&'static [Studio], String> {
    #[derive(serde::Deserialize)]
    struct Catalog {
        schema: String,
        items: Vec<Studio>,
    }
    static CATALOG: std::sync::OnceLock<Result<Vec<Studio>, String>> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let source = super::library_data::catalog("studios").ok_or("missing native studio catalog")?;
            let catalog: Catalog = serde_json::from_str(source).map_err(|e| format!("invalid native studio catalog: {e}"))?;
            if catalog.schema != "solidcraft-studios/1" {
                return Err(format!("unsupported studio schema: {}", catalog.schema));
            }
            for studio in &catalog.items {
                studio.validate().map_err(|e| format!("{}: {e}", studio.id))?;
            }
            Ok(catalog.items)
        })
        .as_ref()
        .map(Vec::as_slice)
        .map_err(Clone::clone)
}

/// The validated catalog supplied by the application.
pub fn builtin() -> &'static [Studio] {
    try_builtin().unwrap_or_default()
}

/// The selected rig when no other studio was requested.
pub fn default_rig() -> Option<&'static StudioRig> {
    builtin().iter().find(|s| s.id == DEFAULT_REALISTIC)?.realistic.as_ref()
}

pub(super) fn require_rig(id: &str, command: &str) -> crate::Result<&'static StudioRig> {
    let entry = builtin().iter().find(|s| s.id == id).ok_or_else(|| crate::params::bad(command, format!("unknown studio {id}")))?;
    entry.realistic.as_ref().ok_or_else(|| crate::params::bad(command, format!("{id} has no lighting rig")))
}

/// A GPU job's lighting and environment for its studio `choice`, about the subject `bounds`.
pub(super) fn job_lighting(
    choice: &solidcraft_doc::render_job::StudioChoice,
    bounds: solidcraft_geom::Aabb3,
    command: &str,
) -> crate::Result<(solidcraft_render::job::StudioLighting, std::sync::Arc<solidcraft_render::lighting::environment::EnvironmentSource>)> {
    choice.validate().map_err(|e| crate::params::bad(command, e.to_string()))?;
    let rig = require_rig(&choice.id, command)?;
    if bounds.is_empty() {
        return Err(crate::params::bad(command, "a studio needs a nonempty visible subject"));
    }
    let (lo, hi) = (bounds.min, bounds.max);
    solidcraft_render::job::studio::lighting(rig, [lo.x, lo.y, lo.z], [hi.x, hi.y, hi.z], choice.rotation_rad)
        .map_err(|e| crate::params::bad(command, e.to_string()))
}
