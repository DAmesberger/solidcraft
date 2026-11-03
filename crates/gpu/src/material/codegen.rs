//! The raster back end's code generator (docs/material-graph.md §5.1): a material graph's IR to
//! one straight-line WGSL function, `material_eval(c: GraphCtx) -> Surface`, over the shared
//! shading library (`crate::shading`).
//!
//! Every identifier in the output is generated (`v3`, `s3`, `g3`) and every value is read from
//! the material's uniform record: no user string (an id, a name, a texture hash) ever reaches
//! WGSL, and constants are never literals. The only literals are the catalogue's own (the
//! surface node's ranges and defaults) and the shape's params (octaves, enum kinds), which are
//! part of the shape. A graph the generator cannot express is an error value, never a panic.

use std::fmt::Write as _;

use solidcraft_render::material_graph::catalogue::{InputDefault, PortType, node_spec};
use solidcraft_render::material_graph::ir::{GeometryInput, Instr, Ir, Operand, ValueId};

/// vec4s of the base response at the head of every material's record (`RECORD`, below); the
/// graph's own uniform slots follow.
pub const BASE_VEC4: u32 = 7;

/// Why a shape has no generated code (it draws its base response instead).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodegenError(pub String);

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "material graph code generation: {}", self.0)
    }
}

impl std::error::Error for CodegenError {}

/// What a value is in WGSL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    F,
    V2,
    V3,
    /// `sc_Texcoord`.
    Tc,
    /// `sc_Triplanar`.
    Tri,
    /// A seed: `u32`.
    Seed,
}

impl Kind {
    fn of(ty: PortType) -> Kind {
        match ty {
            PortType::Float | PortType::Numeric => Kind::F,
            PortType::Int => Kind::Seed,
            PortType::Vector2 => Kind::V2,
            PortType::Color3 | PortType::Vector3 | PortType::Normal => Kind::V3,
            PortType::Texcoord => Kind::Tc,
            PortType::Texture | PortType::Surface => Kind::F,
        }
    }
    fn wgsl(self) -> &'static str {
        match self {
            Kind::F => "f32",
            Kind::V2 => "vec2<f32>",
            Kind::V3 => "vec3<f32>",
            Kind::Tc => "sc_Texcoord",
            Kind::Tri => "sc_Triplanar",
            Kind::Seed => "u32",
        }
    }
    /// `x` (a scalar expression) as this kind.
    fn splat(self, x: &str) -> String {
        match self {
            Kind::V2 | Kind::V3 => format!("{}({x})", self.wgsl()),
            _ => x.to_owned(),
        }
    }
}

/// A WGSL float literal of a catalogue constant.
fn lit(x: f64) -> String {
    let x = x as f32;
    if !x.is_finite() {
        return "0.0".to_owned();
    }
    let s = format!("{x:?}");
    if s.contains('.') || s.contains('e') { s } else { format!("{s}.0") }
}

const ZERO3: &str = "vec3<f32>(0.0)";

struct Gen<'a> {
    ir: &'a Ir,
    out: String,
    /// The WGSL kind of each instruction's value `vN`.
    kinds: Vec<Kind>,
    /// Instructions whose value carries a gradient `gN` (per mm, world space).
    grads: Vec<bool>,
}

impl Gen<'_> {
    fn instr(&self, v: ValueId) -> Result<&Instr, CodegenError> {
        usize::try_from(v.0).ok().and_then(|i| self.ir.instrs.get(i)).ok_or_else(|| CodegenError(format!("value {} out of range", v.0)))
    }

    fn kind_of(&self, op: &Operand) -> Kind {
        match op {
            Operand::Value(v) => usize::try_from(v.0).ok().and_then(|i| self.kinds.get(i)).copied().unwrap_or(Kind::F),
            Operand::Uniform { width, .. } => match width {
                1 => Kind::F,
                2 => Kind::V2,
                _ => Kind::V3,
            },
            Operand::Geometry(GeometryInput::Facing) => Kind::F,
            Operand::Geometry(_) => Kind::V3,
            Operand::Texture(_) | Operand::Absent => Kind::F,
        }
    }

    /// The raw expression of an operand, in its own kind.
    fn raw(&self, op: &Operand, seed: bool) -> Result<String, CodegenError> {
        Ok(match op {
            Operand::Value(v) => {
                self.instr(*v)?;
                format!("v{}", v.0)
            }
            Operand::Uniform { offset, width } => {
                let index = offset / 4;
                let lane = offset % 4;
                let swizzle = match (width, lane) {
                    (1, 0) => "x",
                    (1, 1) => "y",
                    (1, 2) => "z",
                    (1, 3) => "w",
                    (2, 0) => "xy",
                    (2, 2) => "zw",
                    (3, 0) => "xyz",
                    _ => return Err(CodegenError(format!("a uniform of width {width} at float {offset}"))),
                };
                let read = format!("mat_page.m[k + {index}u].{swizzle}");
                if seed && *width == 1 { format!("bitcast<u32>({read})") } else { read }
            }
            Operand::Geometry(g) => match g {
                GeometryInput::ObjectPosition => "c.p_object".to_owned(),
                GeometryInput::WorldPosition => "c.p_world".to_owned(),
                GeometryInput::ObjectNormal => "c.n_object".to_owned(),
                GeometryInput::WorldNormal => "c.n".to_owned(),
                GeometryInput::Facing => "abs(dot(c.n, c.frag.v))".to_owned(),
                GeometryInput::Tangent => "sc_default_tangent(c.n)".to_owned(),
            },
            Operand::Texture(_) => "0.0".to_owned(),
            Operand::Absent => "0.0".to_owned(),
        })
    }

    /// An operand as `want`: a float broadcasts, a vector2 widens with a zero z.
    fn arg(&self, op: &Operand, want: Kind) -> Result<String, CodegenError> {
        if want == Kind::Seed {
            return match op {
                Operand::Uniform { width: 1, .. } => self.raw(op, true),
                Operand::Absent => Ok("0u".to_owned()),
                _ => Err(CodegenError("a seed that is not a constant".to_owned())),
            };
        }
        let have = self.kind_of(op);
        if matches!(op, Operand::Absent | Operand::Texture(_)) {
            return Ok(match want {
                Kind::Tc => "mat_texcoord_none()".to_owned(),
                k => k.splat("0.0"),
            });
        }
        let raw = self.raw(op, false)?;
        Ok(match (have, want) {
            (a, b) if a == b => raw,
            (Kind::F, Kind::V2 | Kind::V3) => want.splat(&raw),
            (Kind::V2, Kind::V3) => format!("vec3<f32>({raw}, 0.0)"),
            (Kind::V3, Kind::V2) => format!("({raw}).xy"),
            (Kind::V2 | Kind::V3, Kind::F) => format!("({raw}).x"),
            (Kind::Tri, Kind::Tc) => format!("{raw}.z"),
            (a, b) => return Err(CodegenError(format!("a {a:?} where a {b:?} is expected"))),
        })
    }

    /// The gradient of a float operand (per mm, world space), zero when it carries none.
    fn grad(&self, op: &Operand) -> String {
        match op {
            Operand::Value(v) if usize::try_from(v.0).ok().and_then(|i| self.grads.get(i)).copied().unwrap_or(false) => format!("g{}", v.0),
            _ => ZERO3.to_owned(),
        }
    }

    fn args<const N: usize>(&self, instr: &Instr, kinds: [Kind; N]) -> Result<[String; N], CodegenError> {
        self.args_at(instr, 0, kinds)
    }

    /// Operands `start..start + N` as `kinds`.
    fn args_at<const N: usize>(&self, instr: &Instr, start: usize, kinds: [Kind; N]) -> Result<[String; N], CodegenError> {
        let mut out: [String; N] = std::array::from_fn(|_| String::new());
        for (k, (slot, kind)) in out.iter_mut().zip(kinds).enumerate() {
            let op = instr.args.get(start + k).copied().unwrap_or(Operand::Absent);
            *slot = self.arg(&op, kind)?;
        }
        Ok(out)
    }

    fn operand(instr: &Instr, k: usize) -> Operand {
        instr.args.get(k).copied().unwrap_or(Operand::Absent)
    }

    fn param(instr: &Instr, k: usize) -> i64 {
        instr.params.get(k).copied().unwrap_or(0)
    }

    fn line(&mut self, text: &str) {
        self.out.push_str("    ");
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// Position, normal and footprint of a projection in `space` (0 object, 1 world).
    fn space(space: i64) -> (&'static str, &'static str, &'static str) {
        if space == 0 { ("c.p_object", "c.n_object", "c.fp_object") } else { ("c.p_world", "c.n", "c.fp_world") }
    }

    /// A pattern call over a texcoord operand: once, or three times and blended when the
    /// texcoord is triplanar. `call` receives the texcoord expression.
    fn pattern(&mut self, n: u32, texcoord: &Operand, call: impl Fn(&str) -> String) -> Result<(), CodegenError> {
        match self.kind_of(texcoord) {
            Kind::Tri => {
                let t = self.raw(texcoord, false)?;
                let (x, y, z) = (call(&format!("{t}.x")), call(&format!("{t}.y")), call(&format!("{t}.z")));
                self.line(&format!("let s{n} = sc_pattern_blend({x}, {y}, {z}, {t}.w);"));
            }
            _ => {
                let t = self.arg(texcoord, Kind::Tc)?;
                let c = call(&t);
                self.line(&format!("let s{n} = {c};"));
            }
        }
        Ok(())
    }

    fn emit(&mut self, index: usize) -> Result<(), CodegenError> {
        let Some(instr) = self.ir.instrs.get(index) else { return Err(CodegenError("instruction out of range".to_owned())) };
        let instr = instr.clone();
        let n = instr.result.0;
        if usize::try_from(n).ok() != Some(index) {
            return Err(CodegenError("instructions out of order".to_owned()));
        }
        let ty = Kind::of(instr.ty);
        let mut kind = ty;
        let mut grad = false;
        if let Some(output) = instr.selects() {
            let call = match instr.args.first() {
                Some(Operand::Value(v)) => *v,
                _ => return Err(CodegenError("a selection without its call".to_owned())),
            };
            let source = self.instr(call)?.kind();
            let call_kind = self.kind_of(&Operand::Value(call));
            let c = call.0;
            let expr = match (source, output) {
                (_, "slope") => format!("s{c}.slope"),
                ("pattern.brushed", "tangent") => format!("s{c}.tangent"),
                ("image", "value") => format!("s{c}.value"),
                ("image", "alpha") => format!("s{c}.alpha"),
                ("separate3", "y") => format!("s{c}.y"),
                ("separate3", "z") => format!("s{c}.z"),
                (k, "uv") if k.starts_with("texcoord.") => {
                    if call_kind == Kind::Tri {
                        format!("sc_tri_uv(v{c})")
                    } else {
                        format!("v{c}.uv")
                    }
                }
                (k, "tangent") if k.starts_with("texcoord.") => {
                    if call_kind == Kind::Tri {
                        format!("sc_tri_tangent(v{c})")
                    } else {
                        format!("sc_tc_tangent(v{c})")
                    }
                }
                _ => return Err(CodegenError("an output selection the raster back end does not know".to_owned())),
            };
            self.line(&format!("let v{n}: {} = {expr};", ty.wgsl()));
        } else {
            match instr.op {
                "texcoord.planar" => {
                    let (p, _, fp) = Self::space(Self::param(&instr, 0));
                    let [o, u, v] = self.args(&instr, [Kind::V3, Kind::V3, Kind::V3])?;
                    let call = format!("sc_tc_planar({p}, {fp}, {o}, {u}, {v})");
                    let call = if Self::param(&instr, 0) == 0 { format!("sc_tc_to_world({call}, c.frame)") } else { call };
                    self.line(&format!("let v{n} = {call};"));
                    kind = Kind::Tc;
                }
                "texcoord.triplanar" => {
                    let (p, nn, fp) = Self::space(Self::param(&instr, 0));
                    let [o, s, sharp] = self.args(&instr, [Kind::V3, Kind::V3, Kind::F])?;
                    let call = format!("sc_tc_triplanar({p}, {nn}, {fp}, {o}, {s}, {sharp})");
                    let call = if Self::param(&instr, 0) == 0 { format!("sc_tri_to_world({call}, c.frame)") } else { call };
                    self.line(&format!("let v{n} = {call};"));
                    kind = Kind::Tri;
                }
                "texcoord.cylindrical" => {
                    let (p, _, fp) = Self::space(Self::param(&instr, 0));
                    let angle = if Self::param(&instr, 1) == 1 { "true" } else { "false" };
                    let [o, a, r, radius] = self.args(&instr, [Kind::V3, Kind::V3, Kind::V3, Kind::F])?;
                    let call = format!("sc_tc_cylindrical({p}, {fp}, {o}, {a}, {r}, {radius}, {angle})");
                    let call = if Self::param(&instr, 0) == 0 { format!("sc_tc_to_world({call}, c.frame)") } else { call };
                    self.line(&format!("let v{n} = {call};"));
                    kind = Kind::Tc;
                }
                "texcoord.transform" => {
                    let input = Self::operand(&instr, 0);
                    let [offset, scale, rotation] = self.args_at(&instr, 1, [Kind::V2, Kind::V2, Kind::F])?;
                    if self.kind_of(&input) == Kind::Tri {
                        let t = self.raw(&input, false)?;
                        self.line(&format!("let v{n} = sc_tri_transform({t}, {offset}, {scale}, {rotation});"));
                        kind = Kind::Tri;
                    } else {
                        let t = self.arg(&input, Kind::Tc)?;
                        self.line(&format!("let v{n} = sc_tc_transform({t}, {offset}, {scale}, {rotation});"));
                        kind = Kind::Tc;
                    }
                }
                "pattern.noise" => {
                    let noise_kind = Self::param(&instr, 0).clamp(0, 2);
                    let dims = Self::param(&instr, 1).clamp(2, 3);
                    let octaves = Self::param(&instr, 2).clamp(1, 8);
                    let slope = if instr.slope { "true" } else { "false" };
                    let [scale, lacunarity, gain, seed] = self.args_at(&instr, 2, [Kind::F, Kind::F, Kind::F, Kind::Seed])?;
                    let texcoord = Self::operand(&instr, 0);
                    if texcoord != Operand::Absent {
                        self.pattern(n, &texcoord, |t| {
                            format!("sc_pattern_noise({t}, {noise_kind}u, {dims}u, {octaves}u, {scale}, {lacunarity}, {gain}, {seed}, {slope})")
                        })?;
                    } else {
                        let position = Self::operand(&instr, 1);
                        let object = position == Operand::Geometry(GeometryInput::ObjectPosition);
                        let p = if position == Operand::Absent { "c.p_world".to_owned() } else { self.arg(&position, Kind::V3)? };
                        let fp = if object { "c.fp_object" } else { "c.fp_world" };
                        self.line(&format!(
                            "let s{n} = sc_pattern_noise3({p}, {fp}, {noise_kind}u, {dims}u, {octaves}u, {scale}, {lacunarity}, {gain}, {seed}, {slope});"
                        ));
                        if object && instr.slope {
                            // An object-space gradient is a covector: into world space by the transpose.
                            self.line(&format!("let g{n} = transpose(c.frame.to_object) * s{n}.grad;"));
                            grad = true;
                        }
                    }
                    self.line(&format!("let v{n} = s{n}.value;"));
                    kind = Kind::F;
                    if instr.slope && !grad {
                        self.line(&format!("let g{n} = s{n}.grad;"));
                        grad = true;
                    }
                }
                "pattern.brushed" => {
                    let slope = if instr.slope { "true" } else { "false" };
                    let [density, streak, contrast, jitter, seed] = self.args_at(&instr, 1, [Kind::F, Kind::F, Kind::F, Kind::F, Kind::Seed])?;
                    let texcoord = Self::operand(&instr, 0);
                    self.pattern(n, &texcoord, |t| format!("sc_pattern_brushed({t}, {density}, {streak}, {contrast}, {jitter}, {seed}, {slope})"))?;
                    self.pattern_value(n, instr.slope, &mut grad);
                    kind = Kind::F;
                }
                "pattern.weave" => {
                    let slope = if instr.slope { "true" } else { "false" };
                    let twill = if Self::param(&instr, 0) == 1 { "true" } else { "false" };
                    let [pitch, crown, fibres, contrast] = self.args_at(&instr, 1, [Kind::V2, Kind::F, Kind::F, Kind::F])?;
                    let texcoord = Self::operand(&instr, 0);
                    self.pattern(n, &texcoord, |t| format!("sc_pattern_weave({t}, {pitch}, {crown}, {fibres}, {contrast}, {twill}, {slope})"))?;
                    self.pattern_value(n, instr.slope, &mut grad);
                    kind = Kind::F;
                }
                "pattern.checker" => {
                    let [size] = self.args_at(&instr, 1, [Kind::V2])?;
                    self.pattern(n, &Self::operand(&instr, 0), |t| format!("sc_pattern_checker({t}, {size})"))?;
                    self.pattern_value(n, instr.slope, &mut grad);
                    kind = Kind::F;
                }
                "pattern.grid" => {
                    let [size, line] = self.args_at(&instr, 1, [Kind::V2, Kind::F])?;
                    self.pattern(n, &Self::operand(&instr, 0), |t| format!("sc_pattern_grid({t}, {size}, {line})"))?;
                    self.pattern_value(n, instr.slope, &mut grad);
                    kind = Kind::F;
                }
                "pattern.gradient" => {
                    let radial = if Self::param(&instr, 0) == 1 { "true" } else { "false" };
                    let [start, end] = self.args_at(&instr, 1, [Kind::F, Kind::F])?;
                    self.pattern(n, &Self::operand(&instr, 0), |t| format!("sc_pattern_gradient({t}, {start}, {end}, {radial})"))?;
                    self.pattern_value(n, instr.slope, &mut grad);
                    kind = Kind::F;
                }
                "image" => {
                    // Sampling lands with the GPU textures (track C3): until then the node's
                    // `default` colour, as for a texture that is not decoded yet (§5.6).
                    let channel = Self::param(&instr, 2).clamp(0, 4);
                    let [default] = self.args_at(&instr, 2, [Kind::V3])?;
                    self.line(&format!("let s{n} = mat_image_default({default}, {channel}u);"));
                    self.line(&format!("let v{n} = s{n}.color;"));
                    kind = Kind::V3;
                }
                "normal.from_height" => {
                    let height = Self::operand(&instr, 0);
                    let g = self.grad(&height);
                    let [relief] = self.args_at(&instr, 1, [Kind::F])?;
                    self.line(&format!("let v{n} = sc_normal_from_height(c.n, {g}, {relief});"));
                    kind = Kind::V3;
                }
                "normal.map" => {
                    let directx = if Self::param(&instr, 0) == 1 { "true" } else { "false" };
                    let [color, strength] = self.args(&instr, [Kind::V3, Kind::F])?;
                    let tangent = self.image_tangent(&Self::operand(&instr, 0));
                    self.line(&format!("let v{n} = sc_normal_map(c.n, {tangent}, {color}, {strength}, {directx});"));
                    kind = Kind::V3;
                }
                "normal.blend" => {
                    let [a, b, t] = self.args(&instr, [Kind::V3, Kind::V3, Kind::F])?;
                    self.line(&format!("let v{n} = sc_normal_blend({a}, {b}, {t});"));
                    kind = Kind::V3;
                }
                _ => {
                    grad = self.maths(&instr, ty)?;
                }
            }
        }
        if let Some(k) = self.kinds.get_mut(index) {
            *k = kind;
        }
        if let Some(g) = self.grads.get_mut(index) {
            *g = grad;
        }
        Ok(())
    }

    fn pattern_value(&mut self, n: u32, slope: bool, grad: &mut bool) {
        self.line(&format!("let v{n} = s{n}.value;"));
        if slope {
            self.line(&format!("let g{n} = s{n}.grad;"));
            *grad = true;
        }
    }

    /// The tangent of the texcoord an `image` colour was sampled with, for a normal map.
    fn image_tangent(&self, color: &Operand) -> String {
        if let Operand::Value(v) = color
            && let Ok(instr) = self.instr(*v)
            && instr.kind() == "image"
        {
            let texcoord = Self::operand(instr, 0);
            if let Ok(t) = self.raw(&texcoord, false) {
                return match self.kind_of(&texcoord) {
                    Kind::Tri => format!("sc_tri_tangent({t})"),
                    Kind::Tc => format!("sc_tc_tangent({t})"),
                    _ => "sc_default_tangent(c.n)".to_owned(),
                };
            }
        }
        "sc_default_tangent(c.n)".to_owned()
    }

    /// A maths node: its value `vN` of kind `ty` and, when it carries a slope, its gradient `gN`.
    /// Returns whether it emitted a gradient.
    fn maths(&mut self, instr: &Instr, ty: Kind) -> Result<bool, CodegenError> {
        let n = instr.result.0;
        let op = instr.kind();
        let spec = node_spec(op).ok_or_else(|| CodegenError("an unknown node kind".to_owned()))?;
        let a0 = Self::operand(instr, 0);
        let a1 = Self::operand(instr, 1);
        let a2 = Self::operand(instr, 2);
        let wide = |g: &Self, ops: &[Operand]| {
            ops.iter().map(|o| g.kind_of(o)).fold(Kind::F, |acc, k| match (acc, k) {
                (Kind::V3, _) | (_, Kind::V3) => Kind::V3,
                (Kind::V2, _) | (_, Kind::V2) => Kind::V2,
                _ => Kind::F,
            })
        };
        let r = ty;
        let one = r.splat("1.0");
        let zero = r.splat("0.0");
        let tiny = r.splat("1e-6");
        let den = |b: &str| format!("select(min({b}, -{tiny}), max({b}, {tiny}), {b} >= {zero})");
        let slope = instr.slope && r == Kind::F;
        let mut g: Option<String> = None;
        let value = match op {
            "constant" => {
                let [a] = self.args(instr, [r])?;
                a
            }
            "add" | "subtract" | "multiply" | "min" | "max" | "power" => {
                let [a, b] = self.args(instr, [r, r])?;
                self.line(&format!("let a{n} = {a};"));
                self.line(&format!("let b{n} = {b};"));
                let (ga, gb) = (self.grad(&a0), self.grad(&a1));
                let (a, b) = (format!("a{n}"), format!("b{n}"));
                match op {
                    "add" => {
                        g = Some(format!("{ga} + {gb}"));
                        format!("{a} + {b}")
                    }
                    "subtract" => {
                        g = Some(format!("{ga} - {gb}"));
                        format!("{a} - {b}")
                    }
                    "multiply" => {
                        g = Some(format!("{a} * {gb} + {b} * {ga}"));
                        format!("{a} * {b}")
                    }
                    "min" => {
                        g = Some(format!("select({gb}, {ga}, {a} <= {b})"));
                        format!("min({a}, {b})")
                    }
                    "max" => {
                        g = Some(format!("select({gb}, {ga}, {a} >= {b})"));
                        format!("max({a}, {b})")
                    }
                    _ => {
                        let base = format!("max({a}, {zero})");
                        g = Some(format!("{b} * pow({base}, {b} - 1.0) * {ga} + log(max({base}, 1e-20)) * pow({base}, {b}) * {gb}"));
                        format!("pow({base}, {b})")
                    }
                }
            }
            "divide" => {
                let [a, b] = self.args(instr, [r, r])?;
                self.line(&format!("let a{n} = {a};"));
                self.line(&format!("let b{n} = {};", den(&b)));
                let (ga, gb) = (self.grad(&a0), self.grad(&a1));
                g = Some(format!("({ga} - (a{n} / b{n}) * {gb}) / b{n}"));
                format!("a{n} / b{n}")
            }
            "clamp" => {
                let [x, lo, hi] = self.args(instr, [r, r, r])?;
                self.line(&format!("let a{n} = {x};"));
                g = Some(format!("select({ZERO3}, {}, a{n} > {lo} && a{n} < {hi})", self.grad(&a0)));
                format!("min(max(a{n}, {lo}), {hi})")
            }
            "abs" | "floor" | "fract" | "sin" | "cos" | "sqrt" | "invert" => {
                let [x] = self.args(instr, [r])?;
                self.line(&format!("let a{n} = {x};"));
                let gx = self.grad(&a0);
                let x = format!("a{n}");
                match op {
                    "abs" => {
                        g = Some(format!("sign({x}) * {gx}"));
                        format!("abs({x})")
                    }
                    "floor" => {
                        g = Some(ZERO3.to_owned());
                        format!("floor({x})")
                    }
                    "fract" => {
                        g = Some(gx);
                        format!("fract({x})")
                    }
                    "sin" => {
                        g = Some(format!("cos({x}) * {gx}"));
                        format!("sin({x})")
                    }
                    "cos" => {
                        g = Some(format!("-sin({x}) * {gx}"));
                        format!("cos({x})")
                    }
                    "sqrt" => {
                        g = Some(format!("{gx} * (0.5 / max(sqrt(max({x}, 0.0)), 1e-6))"));
                        format!("sqrt(max({x}, {zero}))")
                    }
                    _ => {
                        g = Some(format!("-{gx}"));
                        format!("{one} - {x}")
                    }
                }
            }
            "mix" => {
                let [a, b, t] = self.args(instr, [r, r, Kind::F])?;
                self.line(&format!("let a{n} = {a};"));
                self.line(&format!("let b{n} = {b};"));
                self.line(&format!("let t{n} = clamp({t}, 0.0, 1.0);"));
                g = Some(format!("{} * (1.0 - t{n}) + {} * t{n} + (b{n} - a{n}) * {}", self.grad(&a0), self.grad(&a1), self.grad(&a2)));
                format!("mix(a{n}, b{n}, {})", r.splat(&format!("t{n}")))
            }
            "step" => {
                let [edge, x] = self.args(instr, [r, r])?;
                g = Some(ZERO3.to_owned());
                format!("step({edge}, {x})")
            }
            "smoothstep" => {
                let [e0, e1, x] = self.args(instr, [r, r, r])?;
                self.line(&format!("let a{n} = {e0};"));
                self.line(&format!("let b{n} = {};", den(&format!("({e1}) - a{n}"))));
                self.line(&format!("let t{n} = clamp(({x} - a{n}) / b{n}, {zero}, {one});"));
                g = Some(format!("(6.0 * t{n} * (1.0 - t{n}) / b{n}) * {}", self.grad(&a2)));
                format!("t{n} * t{n} * ({} - 2.0 * t{n})", r.splat("3.0"))
            }
            "remap" => {
                let [x, in_lo, in_hi, out_lo, out_hi] = self.args(instr, [r, r, r, r, r])?;
                self.line(&format!("let a{n} = {in_lo};"));
                self.line(&format!("let b{n} = {};", den(&format!("({in_hi}) - a{n}"))));
                self.line(&format!("let t{n} = clamp(({x} - a{n}) / b{n}, {zero}, {one});"));
                self.line(&format!("let o{n} = {out_lo};"));
                self.line(&format!("let d{n} = ({out_hi}) - o{n};"));
                g = Some(format!("select({ZERO3}, (d{n} / b{n}) * {}, t{n} > 0.0 && t{n} < 1.0)", self.grad(&a0)));
                format!("o{n} + d{n} * t{n}")
            }
            "dot" => {
                let w = wide(self, &[a0, a1]);
                let [a, b] = self.args(instr, [w, w])?;
                if w == Kind::F {
                    self.line(&format!("let a{n} = {a};"));
                    self.line(&format!("let b{n} = {b};"));
                    g = Some(format!("a{n} * {} + b{n} * {}", self.grad(&a1), self.grad(&a0)));
                    format!("a{n} * b{n}")
                } else {
                    format!("dot({a}, {b})")
                }
            }
            "length" => {
                let w = wide(self, &[a0]);
                let [x] = self.args(instr, [w])?;
                if w == Kind::F {
                    self.line(&format!("let a{n} = {x};"));
                    g = Some(format!("sign(a{n}) * {}", self.grad(&a0)));
                    format!("abs(a{n})")
                } else {
                    format!("length({x})")
                }
            }
            "normalize" => {
                let [x] = self.args(instr, [r])?;
                g = Some(ZERO3.to_owned());
                match r {
                    Kind::V3 => format!("sc_normalize3({x})"),
                    Kind::V2 => format!("sc_normalize2({x})"),
                    _ => format!("select(1.0, sign({x}), {x} != 0.0)"),
                }
            }
            "combine3" => {
                let [x, y, z] = self.args(instr, [Kind::F, Kind::F, Kind::F])?;
                format!("vec3<f32>({x}, {y}, {z})")
            }
            "separate3" => {
                let [x] = self.args(instr, [Kind::V3])?;
                self.line(&format!("let s{n} = {x};"));
                format!("s{n}.x")
            }
            "luminance" => {
                let [x] = self.args(instr, [Kind::V3])?;
                format!("sc_luminance({x})")
            }
            "hsv_adjust" => {
                let [x, h, s, v] = self.args(instr, [Kind::V3, Kind::F, Kind::F, Kind::F])?;
                format!("sc_hsv_adjust({x}, {h}, {s}, {v})")
            }
            "convert.float_to_color3" | "convert.float_to_vector3" | "convert.color3_to_vector3" | "convert.vector3_to_color3" => {
                let [x] = self.args(instr, [Kind::V3])?;
                x
            }
            _ => return Err(CodegenError(format!("node kind {:?} has no raster code", spec.kind))),
        };
        self.line(&format!("let v{n}: {} = {value};", r.wgsl()));
        if slope {
            let g = g.unwrap_or_else(|| ZERO3.to_owned());
            self.line(&format!("let g{n} = {g};"));
        }
        Ok(slope)
    }

    /// A `Surface` named `name` from a surface node's operands: each input clamped to its range
    /// with NaN at its default (the catalogue's), as the surface node defines.
    fn surface(&mut self, name: &str, inputs: &[(&'static str, Operand)]) -> Result<(), CodegenError> {
        let spec = node_spec("standard_surface").ok_or_else(|| CodegenError("no standard_surface".to_owned()))?;
        let field = |input: &str| -> Result<String, CodegenError> {
            let spec_input = spec.input(input).ok_or_else(|| CodegenError("a surface input the catalogue lacks".to_owned()))?;
            let op = inputs.iter().find(|(n, _)| *n == input).map(|(_, o)| *o).unwrap_or(Operand::Absent);
            let (lo, hi) = spec_input.range.unwrap_or((0.0, 1.0));
            Ok(match spec_input.ty {
                PortType::Color3 => {
                    let d = match spec_input.default {
                        InputDefault::Vec3(d) => format!("vec3<f32>({}, {}, {})", lit(d[0]), lit(d[1]), lit(d[2])),
                        _ => "vec3<f32>(1.0)".to_owned(),
                    };
                    if op == Operand::Absent { d } else { format!("sc_clean3({}, {}, {}, {d})", self.arg(&op, Kind::V3)?, lit(lo), lit(hi)) }
                }
                PortType::Normal => {
                    if op == Operand::Absent {
                        "c.n".to_owned()
                    } else {
                        format!("sc_clean_unit({}, c.n)", self.arg(&op, Kind::V3)?)
                    }
                }
                PortType::Vector3 => {
                    if op == Operand::Absent {
                        "sc_default_tangent(c.n)".to_owned()
                    } else {
                        self.arg(&op, Kind::V3)?
                    }
                }
                _ => {
                    let d = match spec_input.default {
                        InputDefault::Float(d) => lit(d),
                        _ => "0.0".to_owned(),
                    };
                    if op == Operand::Absent { d } else { format!("sc_clean({}, {}, {}, {d})", self.arg(&op, Kind::F)?, lit(lo), lit(hi)) }
                }
            })
        };
        let base_weight = field("base_weight")?;
        let base_color = field("base_color")?;
        let metalness = field("base_metalness")?;
        let specular_weight = field("specular_weight")?;
        let specular_color = field("specular_color")?;
        let roughness = field("specular_roughness")?;
        let ior = field("specular_ior")?;
        let anisotropy = field("specular_roughness_anisotropy")?;
        let rotation = field("specular_rotation")?;
        let tangent = field("geometry_tangent")?;
        let coat_weight = field("coat_weight")?;
        let coat_color = field("coat_color")?;
        let coat_roughness = field("coat_roughness")?;
        let coat_ior = field("coat_ior")?;
        let coat_normal = field("geometry_coat_normal")?;
        let normal = field("geometry_normal")?;
        let transmission_weight = field("transmission_weight")?;
        let transmission_color = field("transmission_color")?;
        let transmission_depth = field("transmission_depth_mm")?;
        let emission_luminance = field("emission_luminance")?;
        let emission_color = field("emission_color")?;
        let opacity = field("geometry_opacity")?;
        self.line(&format!("let {name}_n = {normal};"));
        self.line(&format!("let {name}_t = {tangent};"));
        self.line(&format!("let {name}_tangent = sc_clean_unit({name}_t - {name}_n * dot({name}_t, {name}_n), sc_default_tangent({name}_n));"));
        let mut text = String::new();
        let _ = write!(
            text,
            "let {name} = Surface({base_color} * {base_weight}, {metalness}, {roughness}, {ior}, {coat_weight}, {coat_roughness}, {coat_ior}, \
             {name}_n, {coat_normal}, {specular_weight}, {specular_color}, {anisotropy}, {rotation}, {name}_tangent, {coat_color}, \
             {emission_luminance}, {emission_color}, {transmission_weight}, {transmission_color}, {transmission_depth}, {opacity});"
        );
        self.line(&text);
        Ok(())
    }
}

/// `material_eval` for a lowered graph: straight-line WGSL over the shared library, reading every
/// value from the material's record at `c.record` (its slots after the base response).
pub fn material_eval(ir: &Ir) -> Result<String, CodegenError> {
    let mut g = Gen { ir, out: String::new(), kinds: vec![Kind::F; ir.instrs.len()], grads: vec![false; ir.instrs.len()] };
    g.out.push_str("fn material_eval(c: GraphCtx) -> Surface {\n");
    g.line(&format!("let k = c.record + {BASE_VEC4}u;"));
    for i in 0..ir.instrs.len() {
        g.emit(i)?;
    }
    g.surface("s", &ir.surface)?;
    let mut last = "s".to_owned();
    for (i, layer) in ir.mix_layers.iter().enumerate() {
        let fg = format!("f{i}");
        g.surface(&fg, &layer.fg)?;
        let t = g.arg(&layer.mix, Kind::F)?;
        let out = format!("m{i}");
        g.line(&format!("let {out} = mat_surface_mix({last}, {fg}, sc_clean({t}, 0.0, 1.0, 0.5));"));
        last = out;
    }
    g.line(&format!("return {last};"));
    g.out.push_str("}\n");
    Ok(g.out)
}

/// The constant-graph `material_eval`: the base response at the head of the record. It draws
/// every material of a shape that is not compiled yet (§5.2), and any constant graph.
pub const FALLBACK_EVAL: &str = "fn material_eval(c: GraphCtx) -> Surface {\n    return mat_base_response(c.record, c.n);\n}\n";
