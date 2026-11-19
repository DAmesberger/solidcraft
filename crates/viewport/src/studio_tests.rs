use super::*;

fn light() -> StudioLight {
    StudioLight {
        id: "key".into(),
        centre: [-1.2, -1.2, 1.0],
        width_axis: [-std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 0.0],
        height_axis: [0.358979079309, 0.358979079309, 0.861549790341],
        half_width: 0.4,
        half_height: 0.5,
        radiance: [357.289685146; 3],
        grade: 0.5,
    }
}

fn rig() -> StudioRig {
    StudioRig {
        lights: vec![light()],
        rig_lights: 1,
        cyclorama: Cyclorama { floor: [0.9; 3], wall: [1.05; 3], band: [0.8; 3], ceiling: [0.6; 3], spill: 0.4 },
        room: Room::default(),
        white_under_key: WHITE_UNDER_KEY,
        acceptance: Acceptance::default(),
    }
}

fn studio() -> Studio {
    Studio { id: "USER-1".into(), name: "Mine".into(), description: String::new(), source: None, realistic: Some(rig()), path: None }
}

#[test]
fn a_studio_round_trips_and_validates() {
    let s = studio();
    s.validate().unwrap();
    let text = serde_json::to_string(&s).unwrap();
    let back: Studio = serde_json::from_str(&text).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.realistic.as_ref().map(StudioRig::content_key), s.realistic.as_ref().map(StudioRig::content_key));
}

#[test]
fn hostile_studios_are_refused() {
    let bad = |f: &dyn Fn(&mut Studio)| {
        let mut s = studio();
        f(&mut s);
        assert!(s.validate().is_err(), "{s:?}");
    };
    bad(&|s| s.id = "no spaces".into());
    bad(&|s| s.name = String::new());
    bad(&|s| s.realistic = None);
    bad(&|s| s.realistic.as_mut().unwrap().lights.clear());
    bad(&|s| s.realistic.as_mut().unwrap().lights = vec![light(); 5]);
    bad(&|s| s.realistic.as_mut().unwrap().lights[0].radiance = [f64::NAN; 3]);
    bad(&|s| s.realistic.as_mut().unwrap().lights[0].width_axis = [1.0, 1.0, 0.0]);
    bad(&|s| s.realistic.as_mut().unwrap().lights[0].centre = [9.0, 0.0, 0.0]);
    bad(&|s| s.realistic.as_mut().unwrap().white_under_key = 0.0);
    bad(&|s| s.realistic.as_mut().unwrap().cyclorama.spill = 2.0);
    bad(&|s| s.realistic.as_mut().unwrap().room.ceiling = -1.0);
    assert!(serde_json::from_str::<Studio>(r#"{"id":"A","name":"B","realistic":null,"extra":1}"#).is_err(), "unknown keys refused");
}

/// The calibration maps a white card under the key to `white_under_key`, whatever the rig's
/// radiance scale.
#[test]
fn the_calibration_is_independent_of_the_rigs_scale() {
    let a = rig();
    let mut b = rig();
    b.lights[0].radiance = [714.579370292; 3];
    let (ca, cb) = (a.calibration(), b.calibration());
    assert!((ca / cb - 2.0).abs() < 1e-9, "{ca} {cb}");
}

/// Placed lights stand at multiples of B about the centre, half an extent over the ground.
#[test]
fn placed_lights_follow_the_subject() {
    let r = rig();
    let placed = r.placed_lights([-50.0, -50.0, 0.0], [50.0, 50.0, 100.0], 0.0, 2.0).unwrap();
    assert_eq!(placed[0].centre, [-120.0, -120.0, 150.0]);
    assert_eq!(placed[0].radiance, [714.579370292; 3]);
    assert!(r.placed_lights([0.0; 3], [0.0; 3], 0.0, 1.0).is_none());
    assert!(r.placed_lights([f64::NAN; 3], [1.0; 3], 0.0, 1.0).is_none());
}
