//! Validation: the catalogue, the limits, typed connections, cycles and constant ranges
//! (docs/material-graph.md §3). It builds an [`Analysis`] that lowering and the base response
//! read, so the three steps agree on every binding and type. Hostile input: every loop is
//! bounded by the limits, which are checked before anything scales with them.

use std::collections::{BTreeMap, BTreeSet};

use super::catalogue::{InputDefault, InputSpec, NodeSpec, ParamKind, PortType, node_spec};
use super::graph::{Constant, InputValue, MaterialGraph, ParamValue};
use super::ir::{GraphIssue, Validated};
use super::limits;
use crate::surface::srgb8_to_linear;

/// At most this many issues are reported; a hostile graph cannot make the list unbounded.
pub(super) const MAX_ISSUES: usize = 64;
/// More inputs or params than any catalogue node has is refused before they are read one by one.
const MAX_NODE_ENTRIES: usize = 64;

/// A resolved port type. `Tri` is a three-component constant whose colour or vector meaning
/// comes from where it is used; after resolution only constants keep it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Ty {
    Float,
    Int,
    Vec2,
    Tri,
    Vec3,
    Col3,
    Texcoord,
    Normal,
    Texture,
    Surface,
}

impl Ty {
    pub(super) fn from_port(p: PortType) -> Option<Ty> {
        Some(match p {
            PortType::Float => Ty::Float,
            PortType::Int => Ty::Int,
            PortType::Color3 => Ty::Col3,
            PortType::Vector2 => Ty::Vec2,
            PortType::Vector3 => Ty::Vec3,
            PortType::Texcoord => Ty::Texcoord,
            PortType::Normal => Ty::Normal,
            PortType::Texture => Ty::Texture,
            PortType::Surface => Ty::Surface,
            PortType::Numeric => return None,
        })
    }

    pub(super) fn port(self) -> PortType {
        match self {
            Ty::Float => PortType::Float,
            Ty::Int => PortType::Int,
            Ty::Vec2 => PortType::Vector2,
            Ty::Tri | Ty::Vec3 => PortType::Vector3,
            Ty::Col3 => PortType::Color3,
            Ty::Texcoord => PortType::Texcoord,
            Ty::Normal => PortType::Normal,
            Ty::Texture => PortType::Texture,
            Ty::Surface => PortType::Surface,
        }
    }

    /// Floats of uniform data a value of this type takes (0: not a uniform value).
    pub(super) fn width(self) -> u8 {
        match self {
            Ty::Float | Ty::Int => 1,
            Ty::Vec2 => 2,
            Ty::Tri | Ty::Vec3 | Ty::Col3 => 3,
            _ => 0,
        }
    }

    fn numeric(self) -> bool {
        matches!(self, Ty::Float | Ty::Vec2 | Ty::Tri | Ty::Vec3 | Ty::Col3)
    }
}

/// The type a maths node takes from two operands: a float broadcasts, an untyped triple adopts
/// a colour or vector meaning; anything else must match.
fn join(a: Ty, b: Ty) -> Option<Ty> {
    match (a, b) {
        _ if a == b => Some(a),
        (Ty::Float, x) | (x, Ty::Float) if x.numeric() => Some(x),
        (Ty::Tri, x @ (Ty::Vec3 | Ty::Col3)) | (x @ (Ty::Vec3 | Ty::Col3), Ty::Tri) => Some(x),
        _ => None,
    }
}

/// Whether a value of type `got` may drive an input that expects `want`.
pub(super) fn accepts(want: Ty, got: Ty) -> bool {
    got == want
        || (got == Ty::Float && matches!(want, Ty::Vec2 | Ty::Tri | Ty::Vec3 | Ty::Col3))
        || (got == Ty::Tri && matches!(want, Ty::Vec3 | Ty::Col3))
}

/// What drives one catalogue input of one node.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Bound {
    Unset,
    Connection { node: usize, output: usize, fallback: Option<Constant> },
    Interface(usize),
    Constant(Constant),
}

/// One node, resolved against the catalogue.
#[derive(Clone, Debug)]
pub(super) struct NodeInfo {
    pub spec: &'static NodeSpec,
    /// Params in catalogue order: choice index or integer.
    pub params: Vec<i64>,
    /// Bindings in catalogue input order.
    pub inputs: Vec<Bound>,
    /// Resolved output types in catalogue order.
    pub outputs: Vec<Ty>,
    /// The resolved type of `Numeric` ports (`Float` when the node has none).
    pub numeric: Ty,
}

impl NodeInfo {
    /// The type an input expects, with `Numeric` resolved.
    pub(super) fn expects(&self, input: &InputSpec) -> Ty {
        Ty::from_port(input.ty).unwrap_or(self.numeric)
    }

    pub(super) fn param(&self, name: &str) -> Option<i64> {
        let index = self.spec.params.iter().position(|p| p.name == name)?;
        self.params.get(index).copied()
    }

    pub(super) fn input(&self, name: &str) -> Option<(&'static InputSpec, &Bound)> {
        let index = self.spec.inputs.iter().position(|i| i.name == name)?;
        Some((self.spec.inputs.get(index)?, self.inputs.get(index)?))
    }
}

/// A checked interface input.
#[derive(Clone, Debug)]
pub(super) struct InterfaceInfo {
    pub name: String,
    pub ty: Ty,
    pub default: Option<Constant>,
}

/// A graph resolved against the catalogue: what validation proved, for lowering and the base
/// response to read.
#[derive(Clone, Debug)]
pub(super) struct Analysis {
    pub nodes: Vec<NodeInfo>,
    pub interface: Vec<InterfaceInfo>,
    pub output: usize,
}

/// Collects issues up to [`MAX_ISSUES`].
#[derive(Default)]
pub(super) struct Issues(pub Vec<GraphIssue>);

impl Issues {
    pub(super) fn push(&mut self, node: Option<&str>, port: Option<&str>, code: &'static str, message: String) {
        if self.0.len() < MAX_ISSUES {
            self.0.push(GraphIssue { node: node.map(shown), port: port.map(shown), code, message });
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A user string as it may appear in a message: at most `ID_BYTES` bytes, on a char boundary.
pub(super) fn shown(s: &str) -> String {
    if s.len() <= limits::ID_BYTES {
        return s.to_owned();
    }
    let mut end = limits::ID_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", s.get(..end).unwrap_or(""))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= limits::ID_BYTES
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
}

fn valid_name(name: &str, max: usize) -> bool {
    !name.is_empty() && name.len() <= max && !name.chars().any(char::is_control)
}

fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// GPUs read values as `f32`: a constant must stay finite there.
fn fits_f32(x: f64) -> bool {
    x.is_finite() && x.abs() <= f64::from(f32::MAX)
}

/// The type of a constant as written, or why it has none.
pub(super) fn constant_ty(c: &Constant) -> Result<Ty, &'static str> {
    match c {
        Constant::Number(x) if fits_f32(*x) => Ok(Ty::Float),
        Constant::Number(_) => Err("not_finite"),
        Constant::Vector(v) if v.iter().any(|x| !fits_f32(*x)) => Err("not_finite"),
        Constant::Vector(v) if v.len() == 2 => Ok(Ty::Vec2),
        Constant::Vector(v) if v.len() == 3 => Ok(Ty::Tri),
        Constant::Vector(_) => Err("constant_form"),
        Constant::Srgb8(_) => Ok(Ty::Col3),
        Constant::Texture(t) if valid_sha256(&t.texture) => Ok(Ty::Texture),
        Constant::Texture(_) => Err("texture_ref"),
    }
}

/// A numeric constant's components, sRGB8 decoded, a number broadcast to `width`.
pub(super) fn components(c: &Constant, width: usize) -> Vec<f64> {
    match c {
        Constant::Number(x) => vec![*x; width.max(1)],
        Constant::Vector(v) => v.clone(),
        Constant::Srgb8(s) => s.srgb8.iter().map(|b| srgb8_to_linear(*b)).collect(),
        Constant::Texture(_) => Vec::new(),
    }
}

/// An input's catalogue default as components (empty: it has none).
pub(super) fn default_components(input: &InputSpec) -> Vec<f64> {
    match input.default {
        InputDefault::Float(x) => vec![x],
        InputDefault::Vec2(v) => v.to_vec(),
        InputDefault::Vec3(v) => v.to_vec(),
        InputDefault::Required | InputDefault::Optional | InputDefault::Geometric => Vec::new(),
    }
}

/// Check a constant for an input expecting `want`: its form, finiteness and the input's range.
pub(super) fn check_constant(input: &InputSpec, want: Ty, c: &Constant) -> Result<(), (&'static str, String)> {
    let got = constant_ty(c).map_err(|code| (code, format!("{} is not a valid constant", input.name)))?;
    if want == Ty::Int {
        let ok = matches!(c, Constant::Number(x) if x.fract() == 0.0);
        if !ok {
            return Err(("constant_type", format!("{} takes an integer", input.name)));
        }
    } else if !accepts(want, got) || matches!(want, Ty::Texcoord | Ty::Normal | Ty::Surface) {
        return Err(("constant_type", format!("{} takes a {} value, not a {} constant", input.name, label(want), label(got))));
    }
    if let Some((lo, hi)) = input.range {
        let width = usize::from(want.width());
        if components(c, width).iter().any(|x| !(lo..=hi).contains(x)) {
            return Err(("out_of_range", format!("{} must be in [{lo}, {hi}]", input.name)));
        }
    }
    Ok(())
}

/// A type as people read it.
pub(super) fn label(t: Ty) -> &'static str {
    match t {
        Ty::Float => "float",
        Ty::Int => "integer",
        Ty::Vec2 => "vector2",
        Ty::Tri => "3-component",
        Ty::Vec3 => "vector3",
        Ty::Col3 => "color3",
        Ty::Texcoord => "texcoord",
        Ty::Normal => "normal",
        Ty::Texture => "texture",
        Ty::Surface => "surface",
    }
}

fn param_index(node: &str, name: &str, kind: ParamKind, value: &ParamValue, issues: &mut Issues) -> Option<i64> {
    match (kind, value) {
        (ParamKind::Choice { options, .. }, ParamValue::Text(t)) => {
            let found = options.iter().position(|o| *o == t.as_str()).and_then(|i| i64::try_from(i).ok());
            if found.is_none() {
                issues.push(Some(node), Some(name), "param_value", format!("{name} must be one of {}", options.join(", ")));
            }
            found
        }
        (ParamKind::Int { min, max, .. }, ParamValue::Int(i)) if (min..=max).contains(i) => Some(*i),
        (ParamKind::Int { min, max, .. }, _) => {
            issues.push(Some(node), Some(name), "param_value", format!("{name} must be an integer in [{min}, {max}]"));
            None
        }
        (ParamKind::Choice { .. }, ParamValue::Int(_)) => {
            issues.push(Some(node), Some(name), "param_value", format!("{name} takes an option name"));
            None
        }
    }
}

fn default_param(kind: ParamKind) -> i64 {
    match kind {
        ParamKind::Choice { options, default } => options.iter().position(|o| *o == default).and_then(|i| i64::try_from(i).ok()).unwrap_or(0),
        ParamKind::Int { default, .. } => default,
    }
}

/// Where a connection reads: a node index and output index. A node id may contain '.', so
/// `a.b` is the output `b` of node `a` when that exists, else the node `a.b`; both existing is
/// ambiguous and refused.
fn resolve_source(from: &str, ids: &BTreeMap<&str, usize>, specs: &[Option<&'static NodeSpec>]) -> Result<(usize, usize), (&'static str, String)> {
    let split = from.split_once('.').and_then(|(node, output)| {
        let index = *ids.get(node)?;
        let spec = (*specs.get(index)?)?;
        let out = spec.outputs.iter().position(|o| o.name == output)?;
        Some((index, out))
    });
    let whole = ids.get(from).map(|i| (*i, 0));
    match (split, whole) {
        (Some(_), Some(_)) => Err(("ambiguous_connection", format!("{} names both a node and a node's output", shown(from)))),
        (Some(s), None) | (None, Some(s)) => Ok(s),
        (None, None) => match from.split_once('.') {
            Some((node, output)) if ids.contains_key(node) => Err(("unknown_output", format!("{} has no output {}", shown(node), shown(output)))),
            _ => Err(("unknown_node", format!("no node {}", shown(from)))),
        },
    }
}

/// Check `graph` and resolve it. Every issue found, up to [`MAX_ISSUES`].
pub(super) fn analyse(graph: &MaterialGraph) -> Result<Analysis, Vec<GraphIssue>> {
    let mut issues = Issues::default();
    if !valid_name(&graph.name, limits::NAME_BYTES) {
        issues.push(None, None, "name", format!("a graph name is 1–{} bytes without control characters", limits::NAME_BYTES));
    }
    if graph.nodes.is_empty() || graph.nodes.len() > limits::NODES {
        issues.push(None, None, "node_count", format!("a graph has 1–{} nodes", limits::NODES));
        return Err(issues.0);
    }
    if graph.interface.len() > limits::INTERFACE_INPUTS {
        issues.push(None, None, "interface_count", format!("a graph exposes at most {} interface inputs", limits::INTERFACE_INPUTS));
        return Err(issues.0);
    }
    if graph.layout.len() > limits::NODES {
        issues.push(None, None, "layout", format!("the layout holds at most {} positions", limits::NODES));
    } else if graph.layout.values().any(|p| p.iter().any(|c| !c.is_finite() || c.abs() > limits::LAYOUT_COORD)) {
        issues.push(None, None, "layout", format!("layout positions are finite and within ±{}", limits::LAYOUT_COORD));
    }
    if let Some(source) = &graph.source
        && (!valid_name(&source.library, limits::NAME_BYTES) || !valid_name(&source.entry, limits::NAME_BYTES))
    {
        issues.push(None, None, "source", "a graph's source names a library and an entry".to_owned());
    }

    // Interface.
    let mut interface = Vec::with_capacity(graph.interface.len());
    let mut interface_ids = BTreeMap::new();
    for (index, input) in graph.interface.iter().enumerate() {
        if !valid_id(&input.name) {
            issues.push(None, Some(&input.name), "interface_name", "an interface name is 1–64 characters of a–z, 0–9, _ . -".to_owned());
        } else if interface_ids.insert(input.name.as_str(), index).is_some() {
            issues.push(None, Some(&input.name), "duplicate_interface", format!("interface input {} twice", shown(&input.name)));
        }
        let ty = match Ty::from_port(input.ty) {
            Some(t @ (Ty::Float | Ty::Int | Ty::Col3 | Ty::Vec2 | Ty::Vec3 | Ty::Texture)) => t,
            _ => {
                issues.push(
                    None,
                    Some(&input.name),
                    "interface_type",
                    "an interface input is a float, integer, color3, vector2, vector3 or texture".to_owned(),
                );
                Ty::Float
            }
        };
        if let Some(default) = &input.default {
            let ok = match constant_ty(default) {
                Ok(got) if ty == Ty::Int => got == Ty::Float && matches!(default, Constant::Number(x) if x.fract() == 0.0),
                Ok(got) => got == ty || (got == Ty::Tri && matches!(ty, Ty::Vec3 | Ty::Col3)),
                Err(_) => false,
            };
            if !ok {
                issues.push(
                    None,
                    Some(&input.name),
                    "interface_default",
                    format!("the default of {} is not a {} value", shown(&input.name), label(ty)),
                );
            }
        }
        interface.push(InterfaceInfo { name: input.name.clone(), ty, default: input.default.clone() });
    }

    // Ids and kinds.
    let mut ids = BTreeMap::new();
    let mut specs = Vec::with_capacity(graph.nodes.len());
    for (index, node) in graph.nodes.iter().enumerate() {
        if !valid_id(&node.id) {
            issues.push(Some(&node.id), None, "id", "a node id is 1–64 characters of a–z, 0–9, _ . -".to_owned());
        } else if ids.insert(node.id.as_str(), index).is_some() {
            issues.push(Some(&node.id), None, "duplicate_id", format!("node id {} twice", shown(&node.id)));
        }
        let spec = node_spec(&node.kind);
        if spec.is_none() {
            issues.push(Some(&node.id), None, "unknown_kind", format!("unknown node kind {}", shown(&node.kind)));
        }
        specs.push(spec);
    }

    // Params and bindings.
    let mut nodes = Vec::with_capacity(graph.nodes.len());
    let mut connections = 0usize;
    for (node, spec) in graph.nodes.iter().zip(&specs) {
        let Some(spec) = *spec else { continue };
        let id = node.id.as_str();
        if node.params.len() > MAX_NODE_ENTRIES || node.inputs.len() > MAX_NODE_ENTRIES {
            issues.push(Some(id), None, "unknown_input", format!("{} has inputs or params the catalogue does not list", spec.kind));
            continue;
        }
        let mut params: Vec<i64> = spec.params.iter().map(|p| default_param(p.kind)).collect();
        for (name, value) in &node.params {
            match spec.params.iter().position(|p| p.name == name.as_str()) {
                Some(i) => {
                    let kind = spec.params.get(i).map(|p| p.kind);
                    if let (Some(kind), Some(slot)) = (kind, params.get_mut(i))
                        && let Some(v) = param_index(id, name, kind, value, &mut issues)
                    {
                        *slot = v;
                    }
                }
                None => issues.push(Some(id), Some(name), "unknown_param", format!("{} has no param {}", spec.kind, shown(name))),
            }
        }
        let mut inputs = vec![Bound::Unset; spec.inputs.len()];
        for (name, value) in &node.inputs {
            let Some(i) = spec.inputs.iter().position(|p| p.name == name.as_str()) else {
                // Refused rather than dropped: subsurface, sheen and the like are later versions.
                issues.push(Some(id), Some(name), "unknown_input", format!("{} has no input {}", spec.kind, shown(name)));
                continue;
            };
            let bound = match value {
                InputValue::Connection(c) => {
                    connections += 1;
                    match resolve_source(&c.from, &ids, &specs) {
                        Ok((node, output)) => Bound::Connection { node, output, fallback: c.fallback.clone() },
                        Err((code, message)) => {
                            issues.push(Some(id), Some(name), code, message);
                            continue;
                        }
                    }
                }
                InputValue::Interface(r) => match interface_ids.get(r.interface.as_str()) {
                    Some(k) => Bound::Interface(*k),
                    None => {
                        issues.push(Some(id), Some(name), "unknown_interface", format!("no interface input {}", shown(&r.interface)));
                        continue;
                    }
                },
                InputValue::Constant(c) => Bound::Constant(c.clone()),
            };
            if let Some(slot) = inputs.get_mut(i) {
                *slot = bound;
            }
        }
        nodes.push(NodeInfo { spec, params, inputs, outputs: Vec::new(), numeric: Ty::Float });
    }
    if connections > limits::CONNECTIONS {
        issues.push(None, None, "connection_count", format!("a graph has at most {} connections", limits::CONNECTIONS));
    }
    let output = match ids.get(graph.output.as_str()) {
        Some(i) => Some(*i),
        None => {
            issues.push(None, None, "output", format!("the output names no node ({})", shown(&graph.output)));
            None
        }
    };
    if !issues.is_empty() {
        return Err(issues.0);
    }
    let Some(output) = output else { return Err(issues.0) };
    if let Some(kind) = nodes.get(output).map(|n| n.spec.kind)
        && kind != "standard_surface"
        && kind != "surface.mix"
    {
        issues.push(Some(&graph.output), None, "output", "the output is a standard_surface or surface.mix node".to_owned());
    }

    let order = match topological_order(&nodes) {
        Ok(order) => order,
        Err(cycle) => {
            let names: Vec<String> = cycle.iter().take(8).filter_map(|i| graph.nodes.get(*i)).map(|n| shown(&n.id)).collect();
            issues.push(
                graph.nodes.get(cycle.first().copied().unwrap_or(0)).map(|n| n.id.as_str()),
                None,
                "cycle",
                format!("connections form a cycle through {}", names.join(", ")),
            );
            return Err(issues.0);
        }
    };
    let names: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    resolve_types(&mut nodes, &interface, &order, &names, &mut issues);
    check_bindings(&nodes, &interface, &names, &mut issues);
    check_semantics(&nodes, &names, &mut issues);
    if issues.is_empty() { Ok(Analysis { nodes, interface, output }) } else { Err(issues.0) }
}

/// Kahn's order (ties by node index), or the nodes left on a cycle.
fn topological_order(nodes: &[NodeInfo]) -> Result<Vec<usize>, Vec<usize>> {
    let mut indegree = vec![0usize; nodes.len()];
    let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        for bound in &node.inputs {
            if let Bound::Connection { node: from, .. } = bound {
                if let Some(d) = indegree.get_mut(i) {
                    *d += 1;
                }
                if let Some(c) = consumers.get_mut(*from) {
                    c.push(i);
                }
            }
        }
    }
    let mut ready: BTreeSet<usize> = indegree.iter().enumerate().filter(|(_, d)| **d == 0).map(|(i, _)| i).collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(i) = ready.pop_first() {
        order.push(i);
        for c in consumers.get(i).map(Vec::as_slice).unwrap_or(&[]) {
            if let Some(d) = indegree.get_mut(*c) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    ready.insert(*c);
                }
            }
        }
    }
    if order.len() == nodes.len() { Ok(order) } else { Err(indegree.iter().enumerate().filter(|(_, d)| **d > 0).map(|(i, _)| i).collect()) }
}

/// The type a binding supplies, before checking it against the input.
fn bound_ty(bound: &Bound, input: &InputSpec, nodes: &[NodeInfo], interface: &[InterfaceInfo]) -> Option<Ty> {
    match bound {
        Bound::Unset => match input.default {
            InputDefault::Float(_) => Some(Ty::Float),
            InputDefault::Vec2(_) => Some(Ty::Vec2),
            InputDefault::Vec3(_) => Ty::from_port(input.ty).or(Some(Ty::Tri)),
            _ => None,
        },
        Bound::Connection { node, output, .. } => nodes.get(*node).and_then(|n| n.outputs.get(*output)).copied(),
        Bound::Interface(k) => interface.get(*k).map(|i| i.ty),
        Bound::Constant(c) => constant_ty(c).ok(),
    }
}

/// Resolve `Numeric` ports: forward (a node takes the widest of its operands), then backward (an
/// untyped triple takes the colour or vector meaning of what it feeds; vector3 when nothing says).
fn resolve_types(nodes: &mut [NodeInfo], interface: &[InterfaceInfo], order: &[usize], id: &[&str], issues: &mut Issues) {
    for &i in order {
        let Some(node) = nodes.get(i) else { continue };
        let mut resolved = Ty::Float;
        for (input, bound) in node.spec.inputs.iter().zip(&node.inputs) {
            if input.ty != PortType::Numeric {
                continue;
            }
            let Some(got) = bound_ty(bound, input, nodes, interface) else { continue };
            match join(resolved, got) {
                Some(t) if got.numeric() => resolved = t,
                _ => issues.push(
                    id.get(i).copied(),
                    Some(input.name),
                    "type_mismatch",
                    format!("{} mixes {} and {} values; use a convert node", node.spec.kind, label(resolved), label(got)),
                ),
            }
        }
        let outputs = node.spec.outputs.iter().map(|o| Ty::from_port(o.ty).unwrap_or(resolved)).collect();
        if let Some(node) = nodes.get_mut(i) {
            node.numeric = resolved;
            node.outputs = outputs;
        }
    }
    // Consumers of each node's numeric outputs.
    let mut consumers: Vec<Vec<(usize, usize, usize)>> = vec![Vec::new(); nodes.len()];
    for (c, node) in nodes.iter().enumerate() {
        for (k, bound) in node.inputs.iter().enumerate() {
            if let Bound::Connection { node: from, output, .. } = bound
                && let Some(list) = consumers.get_mut(*from)
            {
                list.push((c, k, *output));
            }
        }
    }
    for &i in order.iter().rev() {
        let Some(node) = nodes.get(i) else { continue };
        if node.numeric != Ty::Tri {
            continue;
        }
        let mut demand: Option<Ty> = None;
        for &(c, k, output) in consumers.get(i).map(Vec::as_slice).unwrap_or(&[]) {
            let numeric_out = node.spec.outputs.get(output).is_some_and(|o| o.ty == PortType::Numeric);
            let (Some(consumer), true) = (nodes.get(c), numeric_out) else { continue };
            let Some(input) = consumer.spec.inputs.get(k) else { continue };
            let want = consumer.expects(input);
            if !matches!(want, Ty::Vec3 | Ty::Col3) {
                continue;
            }
            match demand {
                Some(d) if d != want => {
                    issues.push(
                        id.get(i).copied(),
                        None,
                        "type_mismatch",
                        "this value feeds both a color3 and a vector3 input; use a convert node".to_owned(),
                    );
                }
                _ => demand = Some(want),
            }
        }
        let resolved = demand.unwrap_or(Ty::Vec3);
        if let Some(node) = nodes.get_mut(i) {
            node.numeric = resolved;
            for (out, spec) in node.outputs.iter_mut().zip(node.spec.outputs) {
                if spec.ty == PortType::Numeric {
                    *out = resolved;
                }
            }
        }
    }
}

/// Check every binding against its input: connection types, constant forms and ranges,
/// interface types and defaults, fallbacks and required inputs.
fn check_bindings(nodes: &[NodeInfo], interface: &[InterfaceInfo], id: &[&str], issues: &mut Issues) {
    for (i, node) in nodes.iter().enumerate() {
        for (input, bound) in node.spec.inputs.iter().zip(&node.inputs) {
            let want = node.expects(input);
            let name = Some(input.name);
            match bound {
                Bound::Unset => {
                    if input.default == InputDefault::Required {
                        issues.push(id.get(i).copied(), name, "required_input", format!("{} needs its {} input", node.spec.kind, input.name));
                    }
                }
                Bound::Connection { fallback, .. } => {
                    let got = bound_ty(bound, input, nodes, interface);
                    if matches!(want, Ty::Int | Ty::Texture) {
                        issues.push(
                            id.get(i).copied(),
                            name,
                            "type_mismatch",
                            format!("{} takes a constant or an interface input, not a connection", input.name),
                        );
                    } else if let Some(got) = got
                        && !accepts(want, got)
                    {
                        issues.push(id.get(i).copied(), name, "type_mismatch", format!("{} takes {}, not {}", input.name, label(want), label(got)));
                    }
                    if let Some(fallback) = fallback {
                        if matches!(want, Ty::Texcoord | Ty::Normal | Ty::Surface | Ty::Texture | Ty::Int) {
                            issues.push(id.get(i).copied(), name, "fallback", format!("{} takes no fallback", input.name));
                        } else if let Err((code, message)) = check_constant(input, want, fallback) {
                            issues.push(id.get(i).copied(), name, if code == "out_of_range" { code } else { "fallback" }, message);
                        }
                    }
                }
                Bound::Interface(k) => {
                    let Some(iface) = interface.get(*k) else { continue };
                    let ok = if want == Ty::Int {
                        iface.ty == Ty::Int
                    } else {
                        accepts(want, iface.ty) && !matches!(want, Ty::Texcoord | Ty::Normal | Ty::Surface)
                    };
                    if !ok {
                        issues.push(
                            id.get(i).copied(),
                            name,
                            "type_mismatch",
                            format!("{} takes {}, not the {} interface input {}", input.name, label(want), label(iface.ty), shown(&iface.name)),
                        );
                    } else if let Some(default) = &iface.default
                        && want != Ty::Texture
                        && let Err((code, message)) = check_constant(input, want, default)
                    {
                        issues.push(
                            id.get(i).copied(),
                            name,
                            if code == "out_of_range" { code } else { "interface_default" },
                            format!("interface {}: {message}", shown(&iface.name)),
                        );
                    }
                }
                Bound::Constant(c) => {
                    if let Err((code, message)) = check_constant(input, want, c) {
                        issues.push(id.get(i).copied(), name, code, message);
                    }
                }
            }
        }
    }
}

fn constant_of<'a>(node: &'a NodeInfo, name: &str) -> Option<&'a Constant> {
    match node.input(name) {
        Some((_, Bound::Constant(c))) => Some(c),
        _ => None,
    }
}

fn vec3_of(node: &NodeInfo, name: &str) -> Option<[f64; 3]> {
    match constant_of(node, name).map(|c| components(c, 3)) {
        Some(v) => Some([*v.first()?, *v.get(1)?, *v.get(2)?]),
        None => match node.input(name).map(|(spec, bound)| (spec.default, bound)) {
            Some((InputDefault::Vec3(d), Bound::Unset)) => Some(d),
            _ => None,
        },
    }
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn cosine(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let l = length(a) * length(b);
    if l > 0.0 { d / l } else { 1.0 }
}

/// The projection node a texcoord input finally reads, through `texcoord.transform`s.
pub(super) fn texcoord_root(nodes: &[NodeInfo], node: usize) -> Option<usize> {
    let mut at = node;
    for _ in 0..=nodes.len() {
        let n = nodes.get(at)?;
        match n.input("texcoord").map(|(_, b)| b) {
            Some(Bound::Connection { node: from, .. })
                if n.spec.kind == "texcoord.transform" || n.spec.kind.starts_with("pattern.") || n.spec.kind == "image" =>
            {
                at = *from
            }
            _ => return n.spec.kind.starts_with("texcoord.").then_some(at),
        }
    }
    None
}

/// Rules beyond types: noise reads one source, projections are not degenerate, a v1
/// `surface.mix` blends a standard surface over a surface, triplanar has no flat uv, and
/// transmission excludes metal.
fn check_semantics(nodes: &[NodeInfo], id: &[&str], issues: &mut Issues) {
    for (i, node) in nodes.iter().enumerate() {
        let connected = |name: &str| matches!(node.input(name), Some((_, Bound::Connection { .. })));
        match node.spec.kind {
            "pattern.noise" => {
                let dims = node.param("dimensions").unwrap_or(2);
                let ok = match (connected("texcoord"), connected("position")) {
                    (true, false) => dims == 2,
                    (false, true) => dims == 3,
                    _ => false,
                };
                if !ok {
                    issues.push(
                        id.get(i).copied(),
                        None,
                        "noise_source",
                        "a 2D noise reads a texcoord and a 3D noise a position, exactly one of them".to_owned(),
                    );
                }
            }
            "texcoord.planar" => {
                if let (Some(u), Some(v)) = (vec3_of(node, "u"), vec3_of(node, "v"))
                    && (length(u) < 0.001 || length(v) < 0.001 || cosine(u, v).abs() > 1e-4)
                {
                    issues.push(
                        id.get(i).copied(),
                        None,
                        "degenerate_projection",
                        "a planar projection needs orthogonal u and v at least 0.001 mm long".to_owned(),
                    );
                }
            }
            "texcoord.cylindrical" => {
                if let (Some(a), Some(r)) = (vec3_of(node, "axis"), vec3_of(node, "reference"))
                    && (length(a) < 1e-6 || length(r) < 1e-6 || cosine(a, r).abs() > 0.999)
                {
                    issues.push(
                        id.get(i).copied(),
                        None,
                        "degenerate_projection",
                        "a cylindrical projection needs a nonzero axis and a reference not along it".to_owned(),
                    );
                }
            }
            "surface.mix" => {
                if let Some((_, Bound::Connection { node: fg, .. })) = node.input("fg")
                    && nodes.get(*fg).map(|n| n.spec.kind) != Some("standard_surface")
                {
                    issues.push(
                        id.get(i).copied(),
                        Some("fg"),
                        "mix_layer",
                        "in v1 a surface.mix's fg is a standard_surface; nest mixes through bg".to_owned(),
                    );
                }
            }
            "standard_surface" => {
                let metal = constant_of(node, "base_metalness").map(|c| components(c, 1));
                let transmission = constant_of(node, "transmission_weight").map(|c| components(c, 1));
                let positive = |v: Option<Vec<f64>>| v.and_then(|v| v.first().copied()).is_some_and(|x| x > 0.0);
                if positive(transmission) && positive(metal) {
                    issues.push(
                        id.get(i).copied(),
                        Some("transmission_weight"),
                        "transmission_conflict",
                        "transmission requires base_metalness 0".to_owned(),
                    );
                }
            }
            _ => {}
        }
        // A triplanar texcoord has no single flat uv.
        for (input, bound) in node.spec.inputs.iter().zip(&node.inputs) {
            if let Bound::Connection { node: from, output, .. } = bound
                && let Some(src) = nodes.get(*from)
                && src.spec.kind == "texcoord.transform"
                && src.spec.outputs.get(*output).is_some_and(|o| o.name == "uv")
                && texcoord_root(nodes, *from).and_then(|r| nodes.get(r)).is_some_and(|r| r.spec.kind == "texcoord.triplanar")
            {
                issues.push(id.get(i).copied(), Some(input.name), "triplanar_uv", "a triplanar texcoord has no single uv".to_owned());
            }
        }
    }
}

/// Check `graph` and return it with the editor layout cleaned (entries for missing nodes dropped).
pub(super) fn validate(graph: &MaterialGraph) -> Result<Validated, Vec<GraphIssue>> {
    let analysis = analyse(graph)?;
    super::lower::build(&analysis)?;
    let mut clean = graph.clone();
    let ids: BTreeSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    clean.layout.retain(|k, _| ids.contains(k.as_str()));
    Ok(Validated::new(clean))
}
