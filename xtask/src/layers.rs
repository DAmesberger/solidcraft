//! Dependency layering rules (plan/architecture.md §3).
//!
//! Adapted from CADCraft's xtask (MIT OR Apache-2.0), see ATTRIBUTION.md.
//!
//! The rule engine works on a small, metadata-independent model so it can be
//! unit-tested; `from_metadata` builds that model from `cargo metadata`.

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0, and additionally may depend on no workspace crate at all.
    Standalone,
    /// Test tooling: may depend on anything up to L5 (it sits at L6 for rule
    /// purposes); other crates may use it only as a dev-dependency.
    Testkit,
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
    /// Crates other front ends use too, kept extractable while they live here: they may depend
    /// on other shared crates and on external crates (UI toolkits included) only. solidcore and
    /// solidcore-record are git dependencies, not workspace members, so they count as external.
    /// Any crate may depend on a shared crate; `cargo xtask shared` builds each on its own.
    Shared,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Standalone => Some(0),
            Class::Testkit => Some(6),
            // Shared crates sit outside the layer order; `check` handles them before it asks.
            Class::Exempt | Class::Shared => None,
        }
    }
}

/// The layering table. Names are package names without the `solidcraft-`
/// prefix.
pub const TABLE: &[(&str, Class)] = &[
    ("geom", Class::Layer(0)),
    // 3D mouse descriptors and reports (no workspace dependencies; the apps read the device)
    ("space-mouse", Class::Layer(0)),
    // the derive macros (proc-macro, no workspace dependencies)
    ("derive", Class::Standalone),
    // the 3Dconnexion driver's client API on macOS (no workspace dependencies)
    ("3dx-client", Class::Standalone),
    // the profiler: timed spans for every layer, the kernel's calls to commands (no workspace dependencies)
    ("profile", Class::Standalone),
    ("sketch", Class::Layer(1)),
    // the B-rep kernel, solidcore (the Rust port of NVIDIA's SMLib, ADR 0002); only the kernel uses it
    ("smlib", Class::Layer(1)),
    // STEP (ISO 10303) exchange structure and geometry, no modelling backend
    ("step", Class::Layer(1)),
    ("kernel", Class::Layer(1)),
    ("render", Class::Layer(1)),
    // multibody dynamics (rigid bodies, joints, forces, time integration; geom only)
    ("dynamics", Class::Layer(1)),
    ("doc", Class::Layer(2)),
    ("io", Class::Layer(3)),
    ("engine", Class::Layer(4)),
    ("ui-egui", Class::Layer(5)),
    ("gpu", Class::Layer(5)),
    ("mcp", Class::Layer(5)),
    ("testkit", Class::Testkit),
    // the sRGB transfer rule and the surface-analysis palette, read by the renderer and the theme (#13291)
    ("color", Class::Shared),
    // the widget handle and rect registry UI components publish into and scenarios/the control channel read (#12936)
    ("ui-registry", Class::Shared),
    // message catalogs' machinery and bidirectional text: lookup, formatting, language choice (#13290)
    ("ui-text", Class::Shared),
    // the MCP server machinery: JSON-RPC over stdio, lifecycle, tool schemas and argument checks; apps register their tools (#12947)
    ("mcp-server", Class::Shared),
    // apps and tooling
    ("solidcraft", Class::Exempt),
    ("cli", Class::Exempt),
    ("web", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later).
/// The viewport draws display plans the GPU crate evaluates and draws with its pipelines; the MCP
/// server's tests draw screenshots with them. The kernel reads and writes STEP through `step`.
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[&["gpu", "ui-egui"], &["gpu", "mcp"], &["smlib", "kernel"], &["step", "kernel"]];

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    let (from, to) = (short_name(from), short_name(to));
    INTRA_LAYER_ORDER.iter().any(|chain| match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
        (Some(f), Some(t)) => t < f,
        _ => false,
    })
}

/// External crates that constitute a UI toolkit / windowing dependency.
/// Entries ending in `*` are prefixes.
pub const UI_CRATES: &[&str] = &["egui", "eframe", "egui-wgpu", "wgpu", "winit", "egui_kittest", "rfd", "bevy*"];

/// Features that exist only for tests: `(crate, feature)`. Only a dev-dependency may enable one;
/// no normal or build dependency, no feature of any crate and not the crate's own `default`.
/// The material graph's f64 reference interpreter is a test oracle and never ships
/// (docs/material-graph.md, decision O12).
pub const TEST_ONLY_FEATURES: &[(&str, &str)] = &[("render", "test-reference")];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 5;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("solidcraft-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// `true` if the dependency is a workspace member.
    pub workspace: bool,
    /// Features the dependency enables.
    pub features: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
    /// The crate's own `[features]` table: each feature and what it enables.
    pub features: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    StandaloneHasWorkspaceDep { krate: String, dep: String },
    TestkitAsNormalDep { krate: String },
    UiBelowL5 { krate: String, dep: String, layer: u8 },
    TestOnlyFeature { krate: String, feature: String, via: String },
    SharedDependsOnApp { krate: String, dep: String },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (see plan/architecture.md §3)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on strictly lower layers")
            }
            Violation::StandaloneHasWorkspaceDep { krate, dep } => {
                write!(f, "{krate}: standalone crate must not depend on workspace crate {dep}")
            }
            Violation::TestkitAsNormalDep { krate } => {
                write!(f, "{krate}: solidcraft-testkit may only be a dev-dependency")
            }
            Violation::UiBelowL5 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L5+")
            }
            Violation::TestOnlyFeature { krate, feature, via } => {
                write!(f, "{krate} enables the test-only feature `{feature}` {via}; only a dev-dependency may")
            }
            Violation::SharedDependsOnApp { krate, dep } => {
                write!(f, "{krate}: shared crate must depend only on other shared crates, solidcore and external crates, not workspace crate {dep}")
            }
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    check_with(crates, classify)
}

/// `check` against any classification (the tests use one with crates TABLE does not have).
fn check_with(crates: &[Crate], classify: impl Fn(&str) -> Option<Class>) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        test_only_features(c, &mut out);
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Exempt {
            continue;
        }
        let layer = class.layer().unwrap_or(0);
        for d in &c.deps {
            // Self dev-dependencies (e.g. to enable features in tests) are fine.
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                if class == Class::Standalone {
                    out.push(Violation::StandaloneHasWorkspaceDep { krate: c.name.clone(), dep: d.name.clone() });
                    continue;
                }
                if class == Class::Shared {
                    // Every kind counts, dev included: its tests must build after extraction,
                    // so not even testkit.
                    match classify(&d.name) {
                        // Unregistered deps are reported on their own entry.
                        None | Some(Class::Shared) => {}
                        Some(_) => out.push(Violation::SharedDependsOnApp { krate: c.name.clone(), dep: d.name.clone() }),
                    }
                    continue;
                }
                match classify(&d.name) {
                    // Unregistered deps are reported on their own entry; any crate may use a shared one.
                    None | Some(Class::Shared) => {}
                    Some(Class::Testkit) if d.kind != DepKind::Dev => {
                        out.push(Violation::TestkitAsNormalDep { krate: c.name.clone() });
                    }
                    Some(Class::Testkit) => {}
                    Some(dc) => {
                        let to = dc.layer().unwrap_or(u8::MAX);
                        if to >= layer && !(to == layer && intra_layer_allowed(&c.name, &d.name)) {
                            out.push(Violation::Upward {
                                krate: c.name.clone(),
                                dep: d.name.clone(),
                                from: layer,
                                to: if to == u8::MAX { 7 } else { to },
                                kind: d.kind,
                            });
                        }
                    }
                }
            } else if class != Class::Shared && layer < UI_MIN_LAYER && is_ui_crate(&d.name) {
                out.push(Violation::UiBelowL5 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

/// Test-only features reach product builds through a normal or build dependency, a feature
/// that forwards them, or the owner's `default`. Apps are checked too: they are what ships.
fn test_only_features(c: &Crate, out: &mut Vec<Violation>) {
    for (owner, feature) in TEST_ONLY_FEATURES {
        let package = format!("solidcraft-{owner}");
        let full = format!("{package}/{feature}");
        for d in &c.deps {
            if d.name == package && d.kind != DepKind::Dev && d.features.iter().any(|f| f == feature) {
                out.push(Violation::TestOnlyFeature { krate: c.name.clone(), feature: full.clone(), via: format!("in a {:?} dependency", d.kind) });
            }
        }
        for (name, enables) in &c.features {
            let forwards = enables.iter().any(|e| e == &full || *e == format!("{package}?/{feature}"));
            let own_default = c.name == package && name == "default" && enables.iter().any(|e| e == feature);
            if forwards || own_default {
                out.push(Violation::TestOnlyFeature { krate: c.name.clone(), feature: full.clone(), via: format!("through its feature `{name}`") });
            }
        }
    }
}

/// Build the model from `cargo metadata --format-version 1 --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str()) || d["path"].is_string();
            let features = d["features"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_owned)).collect();
            deps.push(Dep { name: dname, kind, workspace, features });
        }
        let features = p["features"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_owned)).collect()))
            .collect();
        out.push(Crate { name, deps, features });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Standalone) => "L0 standalone".into(),
        Some(Class::Testkit) => "testkit".into(),
        Some(Class::Exempt) => "exempt".into(),
        Some(Class::Shared) => "shared".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
        Crate {
            name: name.into(),
            deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w, features: Vec::new() }).collect(),
            features: Vec::new(),
        }
    }
    use DepKind::*;

    #[test]
    fn clean_downward_graph_passes() {
        let g = [
            c("solidcraft-geom", &[("kurbo", Normal, false)]),
            c("solidcraft-doc", &[("solidcraft-geom", Normal, true)]),
            c("solidcraft-engine", &[("solidcraft-doc", Normal, true), ("solidcraft-testkit", Dev, true)]),
            c("solidcraft-ui-egui", &[("solidcraft-engine", Normal, true), ("egui", Normal, false)]),
            c("solidcraft-cli", &[("solidcraft-ui-egui", Normal, true)]),
        ];
        assert!(check(&g).is_empty(), "{:?}", check(&g));
    }

    #[test]
    fn upward_dependency_flagged() {
        let v = check(&[c("solidcraft-doc", &[("solidcraft-engine", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 2, to: 4, .. }]));
    }

    #[test]
    fn sideways_dependency_flagged() {
        let v = check(&[c("solidcraft-kernel", &[("solidcraft-sketch", Normal, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 1, to: 1, .. }]));
    }

    #[test]
    fn self_dev_dependency_ignored() {
        assert!(check(&[c("solidcraft-doc", &[("solidcraft-doc", Dev, true)])]).is_empty());
    }

    #[test]
    fn upward_dev_dependency_flagged() {
        let v = check(&[c("solidcraft-geom", &[("solidcraft-doc", Dev, true)])]);
        assert!(matches!(v[..], [Violation::Upward { kind: Dev, .. }]));
    }

    #[test]
    fn ui_crates_below_l6_flagged() {
        for dep in ["egui", "eframe", "winit", "egui_kittest", "rfd", "bevy_ecs", "bevy"] {
            let v = check(&[c("solidcraft-engine", &[(dep, Normal, false)])]);
            assert!(matches!(v[..], [Violation::UiBelowL5 { layer: 4, .. }]), "{dep}");
        }
        assert!(check(&[c("solidcraft-engine", &[("egui_extras_not", Normal, false)])]).is_empty());
        assert!(check(&[c("solidcraft-ui-egui", &[("winit", Normal, false)])]).is_empty());
    }

    #[test]
    fn unregistered_crate_is_error() {
        let v = check(&[c("solidcraft-mystery", &[])]);
        assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "solidcraft-mystery"));
        assert!(v[0].to_string().contains("register"));
    }

    #[test]
    fn testkit_only_as_dev_dependency() {
        let v = check(&[c("solidcraft-doc", &[("solidcraft-testkit", Normal, true)])]);
        assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]));
        assert!(check(&[c("solidcraft-doc", &[("solidcraft-testkit", Dev, true)])]).is_empty());
        assert!(check(&[c("solidcraft-testkit", &[("solidcraft-engine", Normal, true)])]).is_empty());
    }

    #[test]
    fn apps_and_xtask_exempt() {
        for app in ["solidcraft", "solidcraft-cli", "xtask"] {
            assert!(check(&[c(app, &[("egui", Normal, false), ("solidcraft-ui-egui", Normal, true)])]).is_empty());
        }
    }

    #[test]
    fn test_only_features_stay_in_dev_dependencies() {
        let with = |kind: DepKind| {
            let mut krate = c("solidcraft-gpu", &[("solidcraft-render", kind, true)]);
            krate.deps[0].features = vec!["test-reference".into()];
            krate
        };
        assert!(check(&[with(Dev)]).is_empty());
        for kind in [Normal, Build] {
            let v = check(&[with(kind)]);
            assert!(matches!(&v[..], [Violation::TestOnlyFeature { .. }]), "{kind:?}: {v:?}");
        }
        // Apps are exempt from layering, not from this.
        let mut app = with(Normal);
        app.name = "solidcraft".into();
        assert!(matches!(&check(&[app])[..], [Violation::TestOnlyFeature { .. }]));
        let mut forwarding = c("solidcraft-engine", &[]);
        forwarding.features = vec![("oracle".into(), vec!["solidcraft-render/test-reference".into()])];
        assert!(matches!(&check(&[forwarding])[..], [Violation::TestOnlyFeature { .. }]));
        let mut weak = c("solidcraft-engine", &[]);
        weak.features = vec![("oracle".into(), vec!["solidcraft-render?/test-reference".into()])];
        assert!(matches!(&check(&[weak])[..], [Violation::TestOnlyFeature { .. }]));
        let mut default_on = c("solidcraft-render", &[]);
        default_on.features = vec![("default".into(), vec!["test-reference".into()]), ("test-reference".into(), vec!["dep:serde_json".into()])];
        assert!(matches!(&check(&[default_on.clone()])[..], [Violation::TestOnlyFeature { .. }]));
        default_on.features.remove(0);
        assert!(check(&[default_on]).is_empty());
    }

    #[test]
    fn shared_crate_must_not_depend_on_layered_crates() {
        let v = check(&[c("solidcraft-ui-registry", &[("solidcraft-engine", Normal, true)])]);
        assert!(matches!(&v[..], [Violation::SharedDependsOnApp { krate, dep }] if krate == "solidcraft-ui-registry" && dep == "solidcraft-engine"));
        assert!(v[0].to_string().contains("shared crate must depend only on"));
        let v = check(&[c("solidcraft-ui-registry", &[("solidcraft-geom", Build, true)])]);
        assert!(matches!(v[..], [Violation::SharedDependsOnApp { .. }]));
    }

    #[test]
    fn any_crate_may_use_a_shared_crate() {
        for user in ["solidcraft-geom", "solidcraft-engine", "solidcraft-ui-egui", "solidcraft-testkit"] {
            assert!(check(&[c(user, &[("solidcraft-ui-registry", Normal, true)])]).is_empty(), "{user}");
        }
    }

    #[test]
    fn shared_crate_may_use_shared_and_external_crates() {
        let classes = |pkg: &str| if short_name(pkg) == "ui-text" { Some(Class::Shared) } else { classify(pkg) };
        for kind in [Normal, Dev, Build] {
            let g = [c("solidcraft-ui-registry", &[("solidcraft-ui-text", kind, true), ("serde", Normal, false), ("solidcore", Normal, false)])];
            assert!(check_with(&g, classes).is_empty(), "{kind:?}: {:?}", check_with(&g, classes));
        }
    }

    #[test]
    fn shared_crate_may_use_ui_crates() {
        for dep in ["egui", "eframe", "winit", "wgpu"] {
            assert!(check(&[c("solidcraft-ui-registry", &[(dep, Normal, false)])]).is_empty(), "{dep}");
        }
    }

    #[test]
    fn shared_crate_must_not_use_testkit_even_in_tests() {
        let v = check(&[c("solidcraft-ui-registry", &[("solidcraft-testkit", Dev, true)])]);
        assert!(matches!(v[..], [Violation::SharedDependsOnApp { .. }]));
    }

    #[test]
    fn metadata_parsing() {
        let meta: Value = serde_json::from_str(
            r#"{"packages":[
                {"name":"solidcraft-doc","dependencies":[
                    {"name":"solidcraft-geom","kind":null,"path":"/x/crates/geom"},
                    {"name":"serde","kind":null},
                    {"name":"proptest","kind":"dev"}]},
                {"name":"solidcraft-geom","dependencies":[]}
            ]}"#,
        )
        .unwrap();
        let g = from_metadata(&meta).unwrap();
        assert_eq!(g.len(), 2);
        let doc = g.iter().find(|c| c.name == "solidcraft-doc").unwrap();
        assert!(doc.deps[0].workspace && !doc.deps[1].workspace);
        assert_eq!(doc.deps[2].kind, Dev);
        assert!(check(&g).is_empty());
    }
}
