/// Every module keeps the library's rules: it parses and validates joined in front of the
/// viewport's PBR shader, every top-level name starts with `sc_`, and the viewport's PBR entry
/// points still translate to WebGL2's GLSL ES 3.00 with the library in front.
#[test]
fn the_library_joins_the_viewport_and_translates_for_webgl2() {
    use naga::back::glsl;
    for (name, source) in super::LIBRARY {
        for line in source.lines() {
            let line = line.trim_start();
            for keyword in ["fn ", "const ", "struct ", "var ", "alias "] {
                if let Some(rest) = line.strip_prefix(keyword) {
                    assert!(rest.starts_with("sc_"), "{name}: `{line}` needs an sc_ name");
                }
            }
            assert!(!line.starts_with('@'), "{name}: no bindings or entry points in the library");
            for banned in ["dpdx", "dpdy", "fwidth", "var<storage", "var<uniform"] {
                assert!(!line.contains(banned), "{name}: `{banned}` is not allowed in the library");
            }
        }
    }
    let source = format!("{}\n{}\n{}", super::library(), crate::view::SHADER, crate::pbr::SOURCE);
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
