//! Lowering a resolved graph to the IR both GPU back ends read (docs/material-graph.md §5.1):
//! straight-line SSA in a canonical order, uniform slots, texture bindings, slope and triplanar
//! marks, the expanded cost (§3.6) and the shape key (§5.2). Data preparation only; nothing here
//! evaluates a graph.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::catalogue::{CATALOGUE_VERSION, InputDefault, PortType, node_spec};
use super::ir::{GeometryInput, GraphIssue, Instr, Ir, MixLayer, Operand, ShapeKey, TextureBinding, UniformSlot, ValueId};
use super::limits;
use super::validate::{Analysis, Bound, Issues, Ty, components, default_components};

/// The op of every output selection: `(kind, output, "kind.output")`. A node's first output is
/// the call's own value, so only the later outputs of multi-output kinds are listed.
pub(super) const SELECTIONS: &[(&str, &str, &str)] = &[
    ("image", "value", "image.value"),
    ("image", "alpha", "image.alpha"),
    ("pattern.brushed", "slope", "pattern.brushed.slope"),
    ("pattern.brushed", "tangent", "pattern.brushed.tangent"),
    ("pattern.gradient", "slope", "pattern.gradient.slope"),
    ("pattern.noise", "slope", "pattern.noise.slope"),
    ("pattern.weave", "slope", "pattern.weave.slope"),
    ("separate3", "y", "separate3.y"),
    ("separate3", "z", "separate3.z"),
    ("texcoord.cylindrical", "uv", "texcoord.cylindrical.uv"),
    ("texcoord.cylindrical", "tangent", "texcoord.cylindrical.tangent"),
    ("texcoord.planar", "uv", "texcoord.planar.uv"),
    ("texcoord.planar", "tangent", "texcoord.planar.tangent"),
    ("texcoord.transform", "uv", "texcoord.transform.uv"),
    ("texcoord.triplanar", "tangent", "texcoord.triplanar.tangent"),
];

/// The node kind of an op (`separate3.y` → `separate3`).
pub(super) fn op_kind(op: &'static str) -> &'static str {
    SELECTIONS.iter().find(|s| s.2 == op).map_or(op, |s| s.0)
}

/// Maths kinds pass slopes through by the chain rule; everything else ends a slope chain.
pub(super) fn is_maths(kind: &str) -> bool {
    !(kind == "standard_surface"
        || kind == "image"
        || ["surface.", "geometry.", "texcoord.", "pattern.", "normal."].iter().any(|p| kind.starts_with(p)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TexKey {
    /// A constant texture on one `image` node.
    Node(usize),
    /// An interface texture read in one colour space.
    Interface(usize, bool),
}

struct Lowerer<'a> {
    an: &'a Analysis,
    instrs: Vec<Instr>,
    memo: BTreeMap<(usize, usize), Operand>,
    slots: Vec<UniformSlot>,
    interface_slots: BTreeMap<usize, usize>,
    next: u32,
    textures: Vec<TextureBinding>,
    texture_keys: BTreeMap<TexKey, u8>,
    issues: Issues,
}

impl Lowerer<'_> {
    fn internal(&mut self, message: &str) {
        self.issues.push(None, None, "internal", format!("material graph lowering: {message}"));
    }

    fn alloc(&mut self, width: u8) -> u32 {
        let align = match width {
            1 => 1,
            2 => 2,
            _ => 4,
        };
        let offset = self.next.div_ceil(align).saturating_mul(align);
        self.next = offset.saturating_add(u32::from(width));
        offset
    }

    fn constant_slot(&mut self, ty: Ty, mut value: Vec<f64>) -> Operand {
        let width = ty.width();
        value.resize(usize::from(width), value.first().copied().unwrap_or(0.0));
        let offset = self.alloc(width);
        self.slots.push(UniformSlot { offset, width, ty: ty.port(), value, interface: None, range: None });
        Operand::Uniform { offset, width }
    }

    fn texture(&mut self, key: TexKey, source: String, srgb: bool) -> Operand {
        if let Some(i) = self.texture_keys.get(&key) {
            return Operand::Texture(*i);
        }
        let index = u8::try_from(self.textures.len()).unwrap_or(u8::MAX);
        self.textures.push(TextureBinding { source, srgb });
        self.texture_keys.insert(key, index);
        Operand::Texture(index)
    }

    /// The operand driving input `k` of node `n`.
    fn operand(&mut self, n: usize, k: usize, depth: usize) -> Operand {
        let an = self.an;
        let (Some(node), Some(input)) = (an.nodes.get(n), an.nodes.get(n).and_then(|node| node.spec.inputs.get(k))) else {
            self.internal("input out of range");
            return Operand::Absent;
        };
        let want = node.expects(input);
        let srgb = node.param("space") == Some(0);
        match node.inputs.get(k).unwrap_or(&Bound::Unset) {
            Bound::Connection { node: from, output, .. } => self.value(*from, *output, depth + 1),
            Bound::Interface(j) => {
                let Some(iface) = an.interface.get(*j) else {
                    self.internal("interface out of range");
                    return Operand::Absent;
                };
                if iface.ty == Ty::Texture {
                    return self.texture(TexKey::Interface(*j, srgb), format!("{}{}", TextureBinding::INTERFACE_PREFIX, iface.name), srgb);
                }
                let width = iface.ty.width();
                if let Some(s) = self.interface_slots.get(j).and_then(|s| self.slots.get_mut(*s)) {
                    s.range = intersect(s.range, input.range);
                    return Operand::Uniform { offset: s.offset, width: s.width };
                }
                let mut value = match &iface.default {
                    Some(c) => components(c, usize::from(width)),
                    None => default_components(input),
                };
                value.resize(usize::from(width), value.first().copied().unwrap_or(0.0));
                let offset = self.alloc(width);
                self.interface_slots.insert(*j, self.slots.len());
                self.slots.push(UniformSlot { offset, width, ty: iface.ty.port(), value, interface: Some(iface.name.clone()), range: input.range });
                Operand::Uniform { offset, width }
            }
            Bound::Constant(c) => {
                if want == Ty::Texture {
                    let source = match c {
                        super::graph::Constant::Texture(t) => t.texture.clone(),
                        _ => String::new(),
                    };
                    return self.texture(TexKey::Node(n), source, srgb);
                }
                self.constant_slot(want, components(c, usize::from(want.width())))
            }
            Bound::Unset => match input.default {
                InputDefault::Optional | InputDefault::Required => Operand::Absent,
                InputDefault::Geometric if input.ty == PortType::Normal => Operand::Geometry(GeometryInput::WorldNormal),
                InputDefault::Geometric => Operand::Geometry(GeometryInput::Tangent),
                _ => self.constant_slot(want, default_components(input)),
            },
        }
    }

    /// The operand for output `output` of node `n`, emitting its instructions once.
    fn value(&mut self, n: usize, output: usize, depth: usize) -> Operand {
        if let Some(op) = self.memo.get(&(n, output)) {
            return *op;
        }
        let an = self.an;
        let Some(node) = an.nodes.get(n) else {
            self.internal("node out of range");
            return Operand::Absent;
        };
        if depth > an.nodes.len() + 2 {
            self.internal("connection depth");
            return Operand::Absent;
        }
        let kind = node.spec.kind;
        let object = node.param("space") == Some(0);
        let geometry = match kind {
            "geometry.position" => Some(if object { GeometryInput::ObjectPosition } else { GeometryInput::WorldPosition }),
            "geometry.normal" => Some(if object { GeometryInput::ObjectNormal } else { GeometryInput::WorldNormal }),
            "geometry.facing" => Some(GeometryInput::Facing),
            _ => None,
        };
        if let Some(g) = geometry {
            let op = Operand::Geometry(g);
            self.memo.insert((n, output), op);
            return op;
        }
        if kind == "standard_surface" || kind == "surface.mix" {
            self.internal("a surface used as a value");
            return Operand::Absent;
        }
        let ty = node.outputs.get(output).copied().unwrap_or(Ty::Float).port();
        let op = if output == 0 {
            let args = (0..node.spec.inputs.len()).map(|k| self.operand(n, k, depth)).collect();
            self.push(kind, ty, node.params.clone(), args)
        } else {
            let call = self.value(n, 0, depth);
            let name = node.spec.outputs.get(output).map_or("", |o| o.name);
            let Some(select) = SELECTIONS.iter().find(|s| s.0 == kind && s.1 == name).map(|s| s.2) else {
                self.internal("an output without a selection op");
                return Operand::Absent;
            };
            self.push(select, ty, Vec::new(), vec![call])
        };
        self.memo.insert((n, output), op);
        op
    }

    fn push(&mut self, op: &'static str, ty: PortType, params: Vec<i64>, args: Vec<Operand>) -> Operand {
        let result = ValueId(u32::try_from(self.instrs.len()).unwrap_or(u32::MAX));
        self.instrs.push(Instr { result, op, ty, params, args, slope: false, triplanar: false });
        Operand::Value(result)
    }

    fn surface(&mut self, n: usize) -> Vec<(&'static str, Operand)> {
        let Some(node) = self.an.nodes.get(n) else { return Vec::new() };
        node.spec.inputs.iter().enumerate().map(|(k, input)| (input.name, self.operand(n, k, 0))).collect()
    }
}

fn intersect(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> Option<(f64, f64)> {
    match (a, b) {
        (Some((a0, a1)), Some((b0, b1))) => Some((a0.max(b0), a1.min(b1))),
        (x, None) | (None, x) => x,
    }
}

/// The connection source of input `name` of node `n`.
fn source(an: &Analysis, n: usize, name: &str) -> Option<usize> {
    match an.nodes.get(n)?.input(name)?.1 {
        Bound::Connection { node, .. } => Some(*node),
        _ => None,
    }
}

/// The `surface.mix` chain from the output down `bg` links, and the standard surface under it.
pub(super) fn surface_chain(an: &Analysis) -> (Vec<usize>, Option<usize>) {
    let mut chain = Vec::new();
    let mut at = an.output;
    for _ in 0..=an.nodes.len() {
        match an.nodes.get(at).map(|n| n.spec.kind) {
            Some("surface.mix") => {
                chain.push(at);
                match source(an, at, "bg") {
                    Some(bg) => at = bg,
                    None => return (chain, None),
                }
            }
            Some("standard_surface") => return (chain, Some(at)),
            _ => return (chain, None),
        }
    }
    (chain, None)
}

fn index_of(instrs: &[Instr], op: Operand) -> Option<usize> {
    match op {
        Operand::Value(ValueId(v)) => usize::try_from(v).ok().filter(|i| *i < instrs.len()),
        _ => None,
    }
}

/// Mark slope-carrying values (they reach a `normal.from_height` height through maths) and
/// pattern calls expanded three times under a triplanar texcoord.
fn mark(instrs: &mut [Instr]) {
    let mut slope = vec![false; instrs.len()];
    for instr in instrs.iter() {
        if instr.op == "normal.from_height"
            && let Some(i) = instr.args.first().and_then(|a| index_of(instrs, *a))
            && let Some(s) = slope.get_mut(i)
        {
            *s = true;
        }
    }
    for i in (0..instrs.len()).rev() {
        if !slope.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some(instr) = instrs.get(i) else { continue };
        if is_maths(op_kind(instr.op)) {
            let sources: Vec<usize> = instr.args.iter().filter_map(|a| index_of(instrs, *a)).collect();
            for j in sources {
                if instrs.get(j).is_some_and(|s| s.ty != PortType::Texcoord)
                    && let Some(s) = slope.get_mut(j)
                {
                    *s = true;
                }
            }
        }
    }
    let mut triplanar = vec![false; instrs.len()];
    for i in 0..instrs.len() {
        let Some(instr) = instrs.get(i) else { continue };
        let reads_triplanar = node_spec(instr.op)
            .and_then(|spec| spec.inputs.iter().position(|p| p.name == "texcoord"))
            .and_then(|k| instr.args.get(k))
            .and_then(|a| index_of(instrs, *a))
            .is_some_and(|j| triplanar.get(j).copied().unwrap_or(false));
        let expand = match instr.op {
            "texcoord.triplanar" => true,
            "texcoord.transform" => reads_triplanar,
            op => (op.starts_with("pattern.") || op == "image") && reads_triplanar,
        };
        if let Some(t) = triplanar.get_mut(i) {
            *t = expand;
        }
    }
    for (i, instr) in instrs.iter_mut().enumerate() {
        instr.slope = slope.get(i).copied().unwrap_or(false);
        // The projection itself is evaluated once; what reads it is expanded.
        instr.triplanar = instr.op != "texcoord.triplanar" && triplanar.get(i).copied().unwrap_or(false);
    }
}

/// Cost units of one instruction after expansion (§3.6), and its texture samples.
fn instr_cost(instr: &Instr) -> (u64, u64) {
    let Some(spec) = node_spec(instr.op) else { return (0, 0) };
    let param = |name: &str| spec.params.iter().position(|p| p.name == name).and_then(|i| instr.params.get(i)).copied();
    let mut cost = u64::from(spec.cost);
    if instr.op == "pattern.noise" {
        let octaves = u64::try_from(param("octaves").unwrap_or(1)).unwrap_or(1);
        let dims = if param("dimensions") == Some(3) { 2 } else { 1 };
        let (num, den) = match param("kind") {
            Some(1) => (3, 2),
            Some(2) => (3, 1),
            _ => (1, 1),
        };
        cost = cost.saturating_mul(octaves).saturating_mul(dims).saturating_mul(num) / den;
    }
    let factor = if instr.slope { 3 } else { 1 } * if instr.triplanar { 3 } else { 1 };
    let samples = match (instr.op, instr.triplanar) {
        ("image", true) => 3,
        ("image", false) => 1,
        _ => 0,
    };
    (cost.saturating_mul(factor), samples)
}

/// Build the IR of a resolved graph, refusing one over a budget.
pub(super) fn build(an: &Analysis) -> Result<Ir, Vec<GraphIssue>> {
    let mut l = Lowerer {
        an,
        instrs: Vec::new(),
        memo: BTreeMap::new(),
        slots: Vec::new(),
        interface_slots: BTreeMap::new(),
        next: 0,
        textures: Vec::new(),
        texture_keys: BTreeMap::new(),
        issues: Issues::default(),
    };
    let (chain, base) = surface_chain(an);
    let Some(base) = base else {
        l.internal("the output is not a surface");
        return Err(l.issues.0);
    };
    let surface = l.surface(base);
    let mut mix_layers = Vec::new();
    let mut cost: u64 = 0;
    for &m in chain.iter().rev() {
        let mix_input = an.nodes.get(m).and_then(|n| n.spec.inputs.iter().position(|p| p.name == "mix")).unwrap_or(0);
        let mix = l.operand(m, mix_input, 0);
        let fg = match source(an, m, "fg") {
            Some(fg) => l.surface(fg),
            None => Vec::new(),
        };
        cost = cost.saturating_add(an.nodes.get(m).map_or(0, |n| u64::from(n.spec.cost)));
        mix_layers.push(MixLayer { mix, fg });
    }
    let mut instrs = std::mem::take(&mut l.instrs);
    mark(&mut instrs);
    let mut samples: u64 = 0;
    for instr in &instrs {
        let (c, s) = instr_cost(instr);
        cost = cost.saturating_add(c);
        samples = samples.saturating_add(s);
    }
    let uniform_floats = l.next.div_ceil(4).saturating_mul(4);
    let object_space = instrs
        .iter()
        .any(|i| i.op.starts_with("texcoord.") && node_spec(i.op).is_some() && i.op != "texcoord.transform" && i.params.first() == Some(&0))
        || l.memo.values().any(|o| *o == Operand::Geometry(GeometryInput::ObjectPosition));
    let mut issues = l.issues;
    if cost > u64::from(limits::COST_UNITS) {
        issues.push(None, None, "cost", format!("the graph costs {cost} units after expansion; the budget is {}", limits::COST_UNITS));
    }
    if samples > u64::from(limits::TEXTURE_SAMPLES) {
        issues.push(None, None, "samples", format!("the graph takes {samples} texture samples after expansion; at most {}", limits::TEXTURE_SAMPLES));
    }
    if l.textures.len() > limits::TEXTURES_PER_GRAPH {
        issues.push(None, None, "textures", format!("the graph reads {} textures; at most {}", l.textures.len(), limits::TEXTURES_PER_GRAPH));
    }
    if usize::try_from(uniform_floats).map_or(true, |u| u > limits::UNIFORM_FLOATS) {
        issues.push(None, None, "uniforms", format!("the graph needs {uniform_floats} uniform floats; at most {}", limits::UNIFORM_FLOATS));
    }
    if !issues.is_empty() {
        return Err(issues.0);
    }
    let mut ir = Ir {
        instrs,
        surface,
        mix_layers,
        uniform_floats,
        textures: l.textures,
        cost: u32::try_from(cost).unwrap_or(u32::MAX),
        samples: u32::try_from(samples).unwrap_or(u32::MAX),
        object_space,
        shape: ShapeKey([0; 16]),
        slots: l.slots,
    };
    ir.shape = shape_key(&ir);
    Ok(ir)
}

/// Bytes of an operand for the shape hash.
fn hash_operand(h: &mut Sha256, op: &Operand) {
    match op {
        Operand::Value(ValueId(v)) => {
            h.update([0]);
            h.update(v.to_le_bytes());
        }
        Operand::Uniform { offset, width } => {
            h.update([1]);
            h.update(offset.to_le_bytes());
            h.update([*width]);
        }
        Operand::Texture(t) => h.update([2, *t]),
        Operand::Geometry(g) => h.update([3, *g as u8]),
        Operand::Absent => h.update([4]),
    }
}

fn hash_str(h: &mut Sha256, s: &str) {
    h.update(u32::try_from(s.len()).unwrap_or(u32::MAX).to_le_bytes());
    h.update(s.as_bytes());
}

fn hash_surface(h: &mut Sha256, surface: &[(&'static str, Operand)]) {
    h.update(u32::try_from(surface.len()).unwrap_or(u32::MAX).to_le_bytes());
    for (name, op) in surface {
        hash_str(h, name);
        hash_operand(h, op);
    }
}

/// The 128-bit shape key: the canonical IR without values (§5.2). Constants, interface defaults,
/// texture hashes, names, ids, provenance and the editor layout never enter it.
pub(super) fn shape_key(ir: &Ir) -> ShapeKey {
    let mut h = Sha256::new();
    h.update(b"solidcraft material graph shape\0");
    h.update(CATALOGUE_VERSION.to_le_bytes());
    h.update(u32::try_from(ir.instrs.len()).unwrap_or(u32::MAX).to_le_bytes());
    for instr in &ir.instrs {
        hash_str(&mut h, instr.op);
        h.update([instr.ty as u8, u8::from(instr.slope), u8::from(instr.triplanar)]);
        h.update(u32::try_from(instr.params.len()).unwrap_or(u32::MAX).to_le_bytes());
        for p in &instr.params {
            h.update(p.to_le_bytes());
        }
        h.update(u32::try_from(instr.args.len()).unwrap_or(u32::MAX).to_le_bytes());
        for a in &instr.args {
            hash_operand(&mut h, a);
        }
    }
    hash_surface(&mut h, &ir.surface);
    h.update(u32::try_from(ir.mix_layers.len()).unwrap_or(u32::MAX).to_le_bytes());
    for layer in &ir.mix_layers {
        hash_operand(&mut h, &layer.mix);
        hash_surface(&mut h, &layer.fg);
    }
    h.update(ir.uniform_floats.to_le_bytes());
    h.update(u32::try_from(ir.textures.len()).unwrap_or(u32::MAX).to_le_bytes());
    for t in &ir.textures {
        h.update([u8::from(t.srgb)]);
    }
    // Which slots are integers (bit-cast seeds) changes generated code.
    for s in &ir.slots {
        h.update([s.ty as u8]);
    }
    h.update([u8::from(ir.object_space)]);
    let digest = h.finalize();
    let mut key = [0u8; 16];
    for (k, d) in key.iter_mut().zip(digest.iter()) {
        *k = *d;
    }
    ShapeKey(key)
}
