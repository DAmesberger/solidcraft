//! A studio's rig as a path job's lighting (docs/material-graph.md §5.8): the same lights and
//! cyclorama the Realistic view bakes (`solidcraft_viewport::studio::StudioRig`), not a second
//! description. The rig's lights become the job's area lights, placed in multiples of the
//! subject's extent B about the studio's centre, at their mean radiance and graded as the view
//! grades them; the cyclorama, which the view bakes into its cube around those lights, becomes
//! the job's environment without them, so no light is counted twice. Both are at the rig's calibrated scale, so with PBR
//! Neutral at exposure 0 a path-traced frame and the Realistic view agree in exposure.
//!
//! One approximation, reported here rather than hidden: the cyclorama is an environment at
//! infinity (the view box-projects it against a room sized to the scene, so reflections of the
//! room's walls sit slightly differently near its edges).

use std::sync::Arc;

use solidcraft_viewport::studio::StudioRig;

use super::{AreaLight, AreaLights, ErrorCode, JobError, Result, StudioLighting};
use crate::lighting::environment::EnvironmentSource;

/// The cyclorama's texels: 512 × 256 over the sphere, as the procedural sky's.
pub const CYCLORAMA_SIZE: [u32; 2] = [512, 256];

/// The cyclorama as the view sees it from the studio's centre, at `calibration`, in the studio's
/// own frame (the environment's rotation turns it with the studio).
pub fn cyclorama(rig: &StudioRig, calibration: f64) -> Vec<[f32; 3]> {
    let [width, height] = CYCLORAMA_SIZE;
    let key_white = rig.key_white();
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for row in 0..height {
        for column in 0..width {
            let d = EnvironmentSource::texel_direction(width, height, column, row);
            pixels.push(rig.ambient_in([d.x, d.y, d.z], key_white).map(|c| (c * calibration) as f32));
        }
    }
    pixels
}

/// A job's lighting and environment for `rig`, around a subject of bounds `min`..`max` (mm),
/// turned by `rotation_rad` about +Z.
pub fn lighting(rig: &StudioRig, min: [f64; 3], max: [f64; 3], rotation_rad: f64) -> Result<(StudioLighting, Arc<EnvironmentSource>)> {
    if !rotation_rad.is_finite() || rotation_rad.abs() > 1e6 {
        return Err(JobError::new(ErrorCode::InvalidRequest, "studio rotation must be finite within +/-1000000 radians"));
    }
    let calibration = rig.calibration();
    let placed = rig
        .placed_lights(min, max, rotation_rad, calibration)
        .ok_or_else(|| JobError::new(ErrorCode::InvalidScene, "a studio needs a finite, non-empty subject"))?;
    let lights: Vec<AreaLight> =
        placed.iter().map(|l| AreaLight { center: l.centre, u: l.u, v: l.v, radiance: l.radiance, grade: l.grade }).collect();
    let lighting = StudioLighting { area_lights: AreaLights::new(&lights)?, ..Default::default() };
    lighting.validate()?;
    let [width, height] = CYCLORAMA_SIZE;
    let environment = EnvironmentSource::linear("studio_cyclorama", width, height, cyclorama(rig, calibration), rotation_rad, 1.0)?;
    Ok((lighting, Arc::new(environment)))
}
