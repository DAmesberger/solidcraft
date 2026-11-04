//! Path-tracer frames of a saved design's graph looks (docs/material-graph.md §5.9, track I): every
//! saved view of `$GOLDEN_DESIGN` (e.g. the realistic showcase) rendered by the GPU path tracer
//! from a `gpu_path` setup lit by starter rig `RIG-002` (STU-002, the studio the Realistic view's
//! default is built on), with the job's graph summary and approximations. Evidence, not a
//! check (`--ignored`):
//!
//! ```sh
//! GOLDEN_DESIGN=<file> GOLDEN_OUT=<dir> [GOLDEN_SIZE=1600x1000] [GOLDEN_SAMPLES=256] [GOLDEN_RIG=RIG-002|none] [GOLDEN_EV=0] [GOLDEN_AMBIENT=30] \
//!     cargo test --release -p solidcraft-gpu --lib path_golden_frames -- --ignored --nocapture
//! ```
//!
//! It writes `path-<view>.png` and `path-frames.jsonl` (the preflight's `graphs` reply and the
//! job's summary per view).

use std::sync::Arc;

use serde_json::json;

use super::super::{Executor, open_headless};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn slug(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

#[test]
#[ignore = "renders a saved design through the path tracer; run explicitly"]
fn path_golden_frames() {
    use solidcraft_render::job::{JobId, Phase};
    let (Some(design), Some(out)) = (env("GOLDEN_DESIGN"), env("GOLDEN_OUT")) else { return };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    let [width, height]: [u32; 2] =
        env("GOLDEN_SIZE").and_then(|s| s.split_once('x').and_then(|(a, b)| Some([a.parse().ok()?, b.parse().ok()?]))).unwrap_or([1600, 1000]);
    let samples: u32 = env("GOLDEN_SAMPLES").and_then(|s| s.parse().ok()).unwrap_or(256);
    let rig = env("GOLDEN_RIG").unwrap_or_else(|| "RIG-002".into());
    let ev: f64 = env("GOLDEN_EV").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let ambient: Option<f64> = env("GOLDEN_AMBIENT").and_then(|s| s.parse().ok());
    let (_, device, queue) = open_headless().unwrap_or_else(|e| panic!("no GPU: {e}"));
    let executor = Executor::new(device, queue).unwrap_or_else(|e| panic!("{e}"));
    let host = solidcraft_render::job::ImageHost::with_executor(Arc::new(executor)).unwrap_or_else(|e| panic!("{e}"));
    let mut session = solidcraft_engine::Session::default();
    session.register_render_job_host(host).unwrap_or_else(|e| panic!("{e}"));
    session.execute("doc.open", &json!({"path": design})).unwrap_or_else(|e| panic!("{e}"));
    let views: Vec<String> = session.doc.named_views.iter().map(|v| v.name.clone()).collect();
    let mut lines = String::new();
    for view in &views {
        session.execute("view.apply", &json!({"view": view})).unwrap_or_else(|e| panic!("{view}: {e}"));
        let camera = session.execute("render.camera.get", &json!({})).unwrap_or_else(|e| panic!("{e}"))["camera"].clone();
        session
            .execute(
                "render.settings.set",
                &json!({"name": "Golden", "set": {"width": width, "height": height, "camera": camera, "exposure_ev": ev,
                    "job": {"method": "gpu_path", "options": {"samples": samples, "max_depth": 8, "seed": 20261104}}}}),
            )
            .unwrap_or_else(|e| panic!("{e}"));
        // The studio the Realistic view's default is built on (STU-002), as authored lights.
        if rig != "none" {
            let scene = solidcraft_engine::product_scene::extract(&session).unwrap_or_else(|e| panic!("{e}"));
            let (c, size) = (scene.bounds.center(), scene.bounds.size());
            let rig = json!({"id": rig, "variant": "complete", "center_mm": [c.x, c.y, c.z], "subject_extent_mm": size.x.max(size.y).max(size.z)});
            session
                .execute(
                    "render.library.studio.apply",
                    &json!({"rig": rig, "name": "Golden", "replace_lighting_and_environment": true, "replace_stage": true}),
                )
                .unwrap_or_else(|e| panic!("{e}"));
        }
        // A uniform environment under the rig (`GOLDEN_AMBIENT`, radiance in the rig's units): the
        // path tracer has no counterpart of the Realistic view's cyclorama.
        if let Some(ambient) = ambient {
            let setup = serde_json::to_value(session.doc.render.setups.get("Golden")).unwrap_or_default();
            let mut job = setup["job"].clone();
            job["options"]["lighting"]["environment"] = json!([ambient, ambient, ambient]);
            session.execute("render.settings.set", &json!({"name": "Golden", "set": {"job": job}})).unwrap_or_else(|e| panic!("{e}"));
        }
        let check = session.execute("render.preflight", &json!({"setup": "Golden"})).unwrap_or_else(|e| panic!("{e}"));
        let started = std::time::Instant::now();
        let start = session.execute("render.start", &json!({"setup": "Golden"})).unwrap_or_else(|e| panic!("{e}"));
        let id: JobId = serde_json::from_value(start["job"].clone()).unwrap_or_else(|e| panic!("{e}"));
        let handle = session.render_job_handle(id).unwrap_or_else(|e| panic!("{e}"));
        let status = handle.wait_timeout(std::time::Duration::from_secs(1800)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(status.progress.phase, Phase::Completed, "{view}: {:?}", status.error);
        let frame = handle.frame().unwrap_or_else(|e| panic!("{e}")).unwrap_or_else(|| panic!("{view}: no frame"));
        let path = out.join(format!("path-{}.png", slug(view.trim_start_matches("Showcase:"))));
        std::fs::write(&path, frame.png()).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let row = json!({"view": view, "file": path.file_name().map(|f| f.to_string_lossy().to_string()), "seconds": started.elapsed().as_secs_f64(),
            "samples": samples, "rig": rig, "exposure_ev": ev, "ambient": ambient, "executor": check["resolved"]["executor"], "graphs": check["graphs"],
            "job_graphs": serde_json::to_value(&status.graphs).unwrap_or_default()});
        eprintln!("{row}");
        lines.push_str(&row.to_string());
        lines.push('\n');
    }
    let path = out.join("path-frames.jsonl");
    std::fs::write(&path, lines).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
