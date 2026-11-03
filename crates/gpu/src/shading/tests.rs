/// Every module keeps the library's rules: it parses and validates joined in front of the
/// viewport's PBR shader, every top-level name starts with `sc_`, and the viewport's PBR entry
/// points still translate to WebGL2's GLSL ES 3.00 with the library in front.
#[test]
fn the_library_joins_the_viewport_and_translates_for_webgl2() {
    use naga::back::glsl;
    for (name, source) in super::LIBRARY {
        for line in source.lines() {
            // Module scope is unindented; a function's own `var`s and `let`s are local.
            let top = !line.starts_with(char::is_whitespace);
            let line = line.trim_start();
            for keyword in ["fn ", "const ", "struct ", "var ", "alias "] {
                if let Some(rest) = line.strip_prefix(keyword).filter(|_| top) {
                    assert!(rest.starts_with("sc_"), "{name}: `{line}` needs an sc_ name");
                }
            }
            assert!(!line.starts_with('@'), "{name}: no bindings or entry points in the library");
            for banned in ["dpdx", "dpdy", "fwidth", "var<storage", "var<uniform"] {
                assert!(!line.contains(banned), "{name}: `{banned}` is not allowed in the library");
            }
        }
    }
    // `pbr::SOURCE` starts with the library (`library_source!`).
    assert!(crate::pbr::SOURCE.starts_with(&super::library()));
    let source = format!("{}\n{}", crate::view::SHADER, crate::pbr::SOURCE);
    let module = match naga::front::wgsl::parse_str(&source) {
        Ok(m) => m,
        Err(e) => panic!("{}", e.emit_to_string(&source)),
    };
    let info = match naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module) {
        Ok(i) => i,
        Err(e) => panic!("{e:?}"),
    };
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    for (stage, entry) in [(naga::ShaderStage::Vertex, "vs_pbr"), (naga::ShaderStage::Fragment, "fs_pbr")] {
        let pipeline = glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
        let mut out = String::new();
        let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
            .and_then(|mut w| w.write().map(|_| ()));
        assert!(written.is_ok(), "{entry}: {written:?}");
    }
}

/// The path tracer's shader carries the same library text as [`super::library`].
#[test]
fn the_literal_library_is_the_joined_modules() {
    assert_eq!(super::library_source!(), super::library());
    assert!(crate::pbr::SOURCE.starts_with(super::library_source!()));
}

/// Every library function, not only those the viewport's entry points reach, translates to
/// WebGL2's GLSL ES 3.00 inside a fragment entry.
#[test]
fn every_library_function_translates_for_webgl2() {
    use naga::back::glsl;
    let probe = r#"
@fragment
fn probe_all(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>) -> @location(0) vec4<f32> {
    let s = 7u;
    let unit = sc_unit(p);
    var c = sc_coated_brdf(vec3<f32>(0.5), vec3<f32>(0.0, 0.5, 1.5), vec3<f32>(1.0, 0.1, 1.5), unit, sc_unit(n), sc_unit(p + n));
    c += sc_base_brdf(vec3<f32>(0.5), vec3<f32>(1.0, 0.3, 1.5), unit, sc_unit(n), sc_unit(p - n));
    c += sc_fresnel(vec3<f32>(0.04), p.x) + sc_base_fresnel(sc_base_f0(vec3<f32>(0.5), 0.5, 1.5), p.y, 0.5, 1.5) + sc_coat_bound(sc_coat_f0(1.5), p.z);
    var x = sc_ggx_d(p.x, 0.1) + sc_smith_g1(p.y, 0.1);
    x += sc_hash2(vec2<i32>(p.xy), s) + sc_hash3(vec3<i32>(p), s);
    x += sc_value_noise2(p.xy, s) + sc_value_noise3(p, s) + sc_gradient_noise2(p.xy, s) + sc_gradient_noise3(p, s);
    x += sc_cellular_noise2(p.xy, s) + sc_cellular_noise3(p, s) + sc_noise(sc_noise_kind_cellular, 3u, p, s);
    x += sc_fbm(sc_noise_kind_value, 2u, p, s, 4u, 2.0, 0.5) + dot(sc_gradient2(vec2<i32>(p.xy), s), p.xy) + dot(sc_gradient3(vec3<i32>(p), s), p);
    let uv = sc_planar_uv(p, vec3<f32>(0.0), sc_repeat_axis(vec3<f32>(2.0, 0.0, 0.0)), vec3<f32>(0.0, 0.5, 0.0))
        + sc_cylindrical_uv(p, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), 2.0, true);
    c += sc_triplanar_weights(sc_unit(n), 4.0) + sc_cylindrical_tangent(p, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0));
    return vec4<f32>(c, x + uv.x + uv.y + sc_pi);
}
"#;
    let source = format!("{}\n{probe}", super::library());
    let module = match naga::front::wgsl::parse_str(&source) {
        Ok(m) => m,
        Err(e) => panic!("{}", e.emit_to_string(&source)),
    };
    let info = match naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default()).validate(&module) {
        Ok(i) => i,
        Err(e) => panic!("{e:?}"),
    };
    let options = glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
    let pipeline = glsl::PipelineOptions { shader_stage: naga::ShaderStage::Fragment, entry_point: "probe_all".into(), multiview: None };
    let mut out = String::new();
    let written = glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
        .and_then(|mut w| w.write().map(|_| ()));
    assert!(written.is_ok(), "{written:?}");
    for name in ["sc_coated_brdf", "sc_fbm", "sc_cellular_noise3", "sc_cylindrical_tangent"] {
        assert!(out.contains(name), "{name} is in the GLSL");
    }
}

/// Runs `body` (WGSL statements that read `i: vec4<f32>` and assign `o: vec4<f32>`) once per input
/// on the GPU, with the library in front. `None` without a GPU.
fn evaluate(body: &str, inputs: &[[f32; 4]]) -> Option<Vec<[f32; 4]>> {
    evaluate_cases(body, inputs, 1)
}

/// [`evaluate`] with `stride` inputs per case: invocation `id.x` reads `inputs[id.x * stride + k]`
/// (`i` is its first) and writes one output.
pub(super) fn evaluate_cases(body: &str, inputs: &[[f32; 4]], stride: usize) -> Option<Vec<[f32; 4]>> {
    use wgpu::util::DeviceExt;
    let (_, device, queue) = crate::open_headless().ok()?;
    let source = format!(
        "{}\n@group(0) @binding(0) var<storage, read> inputs: array<vec4<f32>>;\n@group(0) @binding(1) var<storage, read_write> outputs: array<vec4<f32>>;\nconst stride: u32 = {stride}u;\n@compute @workgroup_size(1)\nfn probe(@builtin(global_invocation_id) id: vec3<u32>) {{\n    let i = inputs[id.x * stride];\n    var o = vec4<f32>(0.0);\n{body}\n    outputs[id.x] = o;\n}}\n",
        super::library()
    );
    let cases = inputs.len() / stride.max(1);
    let module =
        device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shading probe"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &inputs.iter().flatten().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let size = (cases.max(1) * 16) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(u32::try_from(cases).ok()?, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &read, 0, size);
    queue.submit([encoder.finish()]);
    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let bytes = read.slice(..).get_mapped_range().map(|b| b.to_vec()).unwrap_or_default();
    Some(
        bytes
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| std::array::from_fn(|k| f32::from_le_bytes([c[4 * k], c[4 * k + 1], c[4 * k + 2], c[4 * k + 3]])))
            .collect(),
    )
}

// Test-only CPU ports of the hashes, in exact integer arithmetic.
fn hash2(x: i32, y: i32, seed: u32) -> f64 {
    let mut h = (x as u32).wrapping_mul(1597334677) ^ (y as u32).wrapping_mul(3812015801) ^ seed.wrapping_mul(0x9e3779b9);
    h = h.wrapping_mul(747796405).wrapping_add(2891336453);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
    h ^= h >> 22;
    f64::from(h) / 4294967295.0
}

fn hash3(c: [i32; 3], seed: u32) -> f64 {
    let c = c.map(|v| (v as u32) & 4095);
    let mut h = c[0].wrapping_mul(0x8da6b343) ^ c[1].wrapping_mul(0xd8163841) ^ c[2].wrapping_mul(0xcb1ab31f) ^ seed.wrapping_mul(0x9e3779b9);
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846ca68b);
    h ^= h >> 16;
    f64::from(h >> 8) / 16777216.0
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn value2(x: f64, y: f64, seed: u32) -> f64 {
    let (i, j) = (x.floor(), y.floor());
    let (wx, wy) = (smooth(x - i), smooth(y - j));
    let (i, j) = (i as i32, j as i32);
    lerp(lerp(hash2(i, j, seed), hash2(i + 1, j, seed), wx), lerp(hash2(i, j + 1, seed), hash2(i + 1, j + 1, seed), wx), wy)
}

fn value3(p: [f64; 3], seed: u32) -> f64 {
    let c = p.map(|v| v.floor() as i32);
    let w: Vec<f64> = p.iter().map(|v| smooth(v - v.floor())).collect();
    let h = |dx: i32, dy: i32, dz: i32| hash3([c[0] + dx, c[1] + dy, c[2] + dz], seed);
    let low = lerp(lerp(h(0, 0, 0), h(1, 0, 0), w[0]), lerp(h(0, 1, 0), h(1, 1, 0), w[0]), w[1]);
    let high = lerp(lerp(h(0, 0, 1), h(1, 0, 1), w[0]), lerp(h(0, 1, 1), h(1, 1, 1), w[0]), w[1]);
    lerp(low, high, w[2])
}

/// The noise matches its CPU ports (value noise and both hashes, including the former viewport
/// `pat_hash` at seed 0), every kind stays in [0, 1] and is continuous, gradient noise is ½ on the
/// lattice, cellular noise is 0 on a feature point, and one octave of fBm is the noise itself.
#[test]
fn gpu_noise_matches_its_reference_and_bounds() {
    let mut points = Vec::new();
    let mut state = 0x2545_f491_u32;
    let mut next = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        f64::from(state >> 8) / 16777216.0
    };
    for _ in 0..256 {
        points.push([(next() * 40.0 - 20.0) as f32, (next() * 40.0 - 20.0) as f32, (next() * 40.0 - 20.0) as f32, (next() * 1000.0).floor() as f32]);
    }
    let body = r#"
    let s = u32(i.w);
    let p = i.xyz;
    if (id.x % 4u == 0u) {
        o = vec4<f32>(sc_value_noise2(p.xy, s), sc_value_noise3(p, s), sc_hash2(vec2<i32>(floor(p.xy)), s), sc_hash3(vec3<i32>(floor(p)), s));
    } else if (id.x % 4u == 1u) {
        o = vec4<f32>(sc_gradient_noise2(p.xy, s), sc_gradient_noise3(p, s), sc_gradient_noise2(floor(p.xy), s), sc_gradient_noise3(floor(p), s));
    } else if (id.x % 4u == 2u) {
        o = vec4<f32>(sc_cellular_noise2(p.xy, s), sc_cellular_noise3(p, s), abs(sc_cellular_noise3(p + vec3<f32>(1e-3), s) - sc_cellular_noise3(p, s)), abs(sc_gradient_noise3(p + vec3<f32>(1e-3), s) - sc_gradient_noise3(p, s)));
    } else {
        o = vec4<f32>(sc_fbm(sc_noise_kind_gradient, 3u, p, s, 1u, 2.0, 0.5) - sc_gradient_noise3(p, s), sc_fbm(sc_noise_kind_value, 2u, p, s, 8u, 2.0, 0.5), sc_noise(sc_noise_kind_value, 3u, p, s) - sc_value_noise3(p, s), abs(sc_value_noise2(p.xy + vec2<f32>(1e-3), s) - sc_value_noise2(p.xy, s)));
    }
"#;
    let Some(got) = evaluate(body, &points) else {
        assert!(std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none(), "a GPU is required here");
        return;
    };
    assert_eq!(got.len(), points.len());
    for (k, (p, o)) in points.iter().zip(&got).enumerate() {
        let (x, y, z, seed) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]), p[3] as u32);
        let o = o.map(f64::from);
        assert!(o.iter().all(|v| v.is_finite()), "{p:?}: {o:?}");
        match k % 4 {
            0 => {
                assert!((o[0] - value2(x, y, seed)).abs() < 1e-5, "value 2D at {p:?}: {} vs {}", o[0], value2(x, y, seed));
                assert!((o[1] - value3([x, y, z], seed)).abs() < 1e-5, "value 3D at {p:?}");
                assert!((o[2] - hash2(x.floor() as i32, y.floor() as i32, seed)).abs() < 1e-6, "hash 2D at {p:?}");
                assert!((o[3] - hash3([x, y, z].map(|v| v.floor() as i32), seed)).abs() < 1e-6, "hash 3D at {p:?}");
            }
            1 => {
                assert!((0.0..=1.0).contains(&o[0]) && (0.0..=1.0).contains(&o[1]), "gradient range at {p:?}: {o:?}");
                assert!((o[2] - 0.5).abs() < 1e-6 && (o[3] - 0.5).abs() < 1e-6, "gradient on the lattice at {p:?}: {o:?}");
            }
            2 => {
                assert!((0.0..=1.0).contains(&o[0]) && (0.0..=1.0).contains(&o[1]), "cellular range at {p:?}: {o:?}");
                assert!(o[2] < 3e-3 && o[3] < 6e-3, "continuity at {p:?}: {o:?}");
            }
            _ => {
                assert!(o[0].abs() < 1e-6 && o[2].abs() < 1e-6, "one octave is the noise at {p:?}: {o:?}");
                assert!((0.0..=1.0).contains(&o[1]) && o[3] < 3e-3, "fBm range and continuity at {p:?}: {o:?}");
            }
        }
    }
    // The former viewport `pat_hash` (no seed) is `sc_hash2` at seed 0.
    let old = |x: i32, y: i32| {
        let mut h = (x as u32).wrapping_mul(1597334677) ^ (y as u32).wrapping_mul(3812015801);
        h = h.wrapping_mul(747796405).wrapping_add(2891336453);
        h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
        h ^= h >> 22;
        f64::from(h) / 4294967295.0
    };
    assert_eq!(old(-3, 17), hash2(-3, 17, 0));
    // A feature point of cell (2, 5) is a zero of cellular noise.
    let seed = 11;
    let feature = [2.0 + hash2(2, 5, seed), 5.0 + hash2(2, 5, seed ^ 0x68e3_1da4)].map(|v| v as f32);
    let Some(at) = evaluate("    o = vec4<f32>(sc_cellular_noise2(i.xy, 11u));", &[[feature[0], feature[1], 0.0, 0.0]]) else { return };
    assert!(at.first().is_some_and(|o| o[0].abs() < 1e-3), "{at:?}");
}

/// The projections at known points: planar repeats, triplanar weights, cylindrical arc length,
/// angle and tangent.
#[test]
fn gpu_projections_match_analytic_values() {
    let body = r#"
    let p = i.xyz;
    if (id.x == 0u) {
        o = vec4<f32>(sc_planar_uv(p, vec3<f32>(1.0, 0.0, 0.0), sc_repeat_axis(vec3<f32>(4.0, 0.0, 0.0)), sc_repeat_axis(vec3<f32>(0.0, 0.0, 0.5))), sc_repeat_axis(vec3<f32>(0.0)).xy);
    } else if (id.x == 1u) {
        o = vec4<f32>(sc_triplanar_weights(sc_unit(p), 4.0), 0.0);
    } else if (id.x == 2u) {
        o = vec4<f32>(sc_cylindrical_uv(p, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), 2.0, true), sc_cylindrical_uv(p, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), 2.0, false));
    } else {
        o = vec4<f32>(sc_cylindrical_tangent(p, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0)), 0.0);
    }
"#;
    let inputs = [[9.0, 3.0, 1.0, 0.0], [0.6, 0.0, 0.8, 0.0], [0.0, 5.0, 3.0, 0.0], [0.0, 5.0, 3.0, 0.0]];
    let Some(got) = evaluate(body, &inputs) else {
        assert!(std::env::var_os("SOLIDCRAFT_REQUIRE_GPU").is_none(), "a GPU is required here");
        return;
    };
    let near = |a: &[f32; 4], b: [f64; 4]| a.iter().zip(b).all(|(x, y)| (f64::from(*x) - y).abs() < 1e-5);
    let quarter = std::f64::consts::FRAC_PI_2;
    let (wx, wz) = (0.6_f64.powi(4), 0.8_f64.powi(4));
    let expected = [[2.0, 2.0, 0.0, 0.0], [wx / (wx + wz), 0.0, wz / (wx + wz), 0.0], [2.0 * quarter, 3.0, quarter, 3.0], [-1.0, 0.0, 0.0, 0.0]];
    for (k, (g, e)) in got.iter().zip(expected).enumerate() {
        assert!(near(g, e), "projection {k}: {g:?} vs {e:?}");
    }
}
