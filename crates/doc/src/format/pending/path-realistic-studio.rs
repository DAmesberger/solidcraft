//! A saved GPU path setup may be lit by a studio of the studio library
//! (`render.setups.*.job.options.studio`: `{id, rotation_rad}`, docs/appearance-graph.md §5.8), its
//! rig's lights and cyclorama in place of saved lighting and an environment; the design keeps the
//! records of the user studios its setups name (`render.studios`, by id), so it renders the same on
//! another machine. A setup may also ask for Khronos PBR Neutral tone mapping
//! (`tone_map: "pbr_neutral"`), the Realistic view's. An older design has none of these: its
//! setups are unchanged and there is no `step`. A design that uses none is still written in the
//! older format (`USED`). The last release warns about the newer format number, opens what it can
//! read and keeps the rest (`crates/doc/src/newer_file.rs`).

use serde_json::Value;

/// Whether a design's saved JSON uses this change: a setup with a studio or PBR Neutral, or kept
/// studios.
pub const USED: fn(&Value) -> bool = |v| {
    v.pointer("/render/studios").is_some()
        || v.pointer("/render/setups").and_then(Value::as_object).is_some_and(|setups| {
            setups.values().any(|setup| {
                setup.pointer("/job/options").is_some_and(|o| o.get("studio").is_some() || o.get("tone_map").and_then(Value::as_str) == Some("pbr_neutral"))
            })
        })
};
