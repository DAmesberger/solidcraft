# Realistic view and material graphs (design note)

Status: proposal for the `DAmesberger/solidcraft` fork, to be offered upstream as **one
package** once it is proven. The package has four parts:

- the realistic renderer (today's opt-in product look, branch `upstream-photoreal-look`:
  [opaque PBR](render-opaque-pbr.md#opt-in-product-look), [clear coat](render-clear-coat.md));
- the material node graph in the file format;
- its editor and commands;
- a realistic **view mode** beside the existing shading;
- a default lighting environment that flatters typical CAD materials, a demo scene, and
  golden GPU frames with an acceptance bar the founder reviews (§5.8, §5.9).

There is no renderer-only PR (§9). Of the new parts, nothing is implemented yet.

The package is **real-time, GPU-only realistic rendering in the interactive viewport**, not image
rendering. A GPU is a hard requirement for it, as it already is for Solidcraft's viewport,
and the feature has no CPU fallback. Upstream's GPU path tracer is a second target of the
same material system, so materials are implemented once (§5.7). The CPU ray tracer and CPU
product host are out of scope.

- CPU work is limited to data preparation: parsing, validation, the IR, code generation and
  texture decoding.
- No CPU shading or rasterising runs in product code.
- A CPU reference exists only inside tests, as an oracle for GPU frames.
Performance figures are **targets** until they are measured. Work estimates are in
agent-hours.

Three founder requirements bind the whole note:

- the standard view stays the default;
- files without material graphs open and save exactly as today;
- material graphs are optional, needed only for realistic rendering.

## 1. Why and what

Today a saved look is `Look { name, color, opacity, pbr: Option<{metallic, roughness,
dielectric_ior}> }` ([`appearance.rs`](../crates/doc/src/appearance.rs),
[`pbr_appearance.rs`](../crates/doc/src/pbr_appearance.rs)). The product look can already
draw a clear coat and four procedural patterns (`SurfacePattern::{Weave, OrangePeel,
PeelOverWeave, InkGrain}` in [`pbr.rs`](../crates/gpu/src/pbr.rs)), but only from an
embedding program's vertex data: no saved design can carry them. Adding `coat`, `pattern`,
`anisotropy`, `texture` … one field at a time would grow `Look` without end, and every
field would need its own format change, command flags and UI.

This note replaces that path with **one** generic, data-only material description:

- A material is a small **node graph**: pattern, projection and maths nodes feed one
  **standard surface** node. The model follows open specifications: the node vocabulary
  follows MaterialX's standard library, and the surface node's inputs follow OpenPBR
  Surface (itself derived from Autodesk Standard Surface).
- Clear coat, anisotropy, patterns and textures are **inputs and nodes**, not format fields.
- The graph is validated on load (bounded size, typed ports, no cycles, value ranges, cost
  budget). Solidcraft compiles it to WGSL itself; a file never carries shader code.
- It renders the same in the native and the WebGL2/wasm builds.
- Everything is generic CAD material data. No product-specific fields, nodes or panels.
- It is **optional**. A design without graphs is the same file as today (§4.4). Graphs
  only show in the realistic view mode (§6). The standard view, which stays the default
  for modelling, shows each look's colour as today.

What it is not: a general shading language, a MaterialX runtime, or a node editor in the
first version (see §7.1). A sandboxed custom-WGSL node is a possible later extension (§3.9).

### Clean room

The sources are public specifications, read as documentation:

- the MaterialX specification (Academy Software Foundation, Apache-2.0);
- the OpenPBR Surface specification (ASWF, Apache-2.0);
- the Autodesk Standard Surface specification (published under Apache-2.0).

We take input **names** and the layering concept from them, so that a later `.mtlx`
exchange is a mapping, not a translation. No MaterialX, OpenPBR or shader-library code is
read, copied or linked. The pattern mathematics is the fork's own (`pbr.rs`) or textbook
material, cited as mathematics only: value noise; gradient noise (Perlin 2002, "Improving
Noise"); cellular noise (Worley 1996); fBm. GPL/LGPL/AGPL code is not read. If a node-editor
widget crate is ever considered (§7.1), its licence must be MIT/Apache/BSD, checked by
`cargo xtask licences --check`.

## 2. Concepts

| Term | Meaning |
|---|---|
| **Graph** | A design-owned, named material definition: nodes, typed connections, one output surface node, and an optional *interface*. Stored once in the design, referenced by looks. |
| **Node** | One operation from the fixed v1 catalogue (§3). It has typed **inputs** (a constant, a connection, or an interface reference) and compile-time **params** (enums and small integers that change the generated code). |
| **Interface** | Up to 32 named, typed inputs a graph exposes (MaterialX's nodegraph interface). A look can override them, so one "Anodised aluminium" graph serves every colour. |
| **Look** | Today's record, plus an optional `graph` (an id) and `values` (interface overrides). The look's `color` remains the colour the standard view, exports and old consumers show. |
| **View look** | A viewport's `standard` (default, today's renderer) or `realistic` drawing, independent of its visual style (§6). Not saved in the design. |
| **Shape** | Everything about a graph that changes generated code: node kinds, params, topology, which inputs are connected and which are constants, texture colour spaces. Values never belong to the shape. |
| **Base response** | A static read of the graph's output node: constant inputs as they are, driven inputs at their declared `fallback`. It is an `OpaqueSurface` (§5.7), computed without evaluating anything. It gives the display `color` and the fallback while a shape compiles. |

Lengths are millimetres and angles are radians, as the render-materials catalogue's scale
policy already requires ([materials.json](render-materials/materials.json)). Colours in
files are linear Rec.709 RGB; a constant may also be written `{"srgb8":[r,g,b]}`, which is
decoded exactly as `srgb8_to_linear` does today.

## 3. Node set v1

**Types:** `float`, `color3` (linear), `vector2`, `vector3`, `texcoord` (a projected
coordinate: uv in mm, the tangent and bitangent it defines, and the pixel footprint in mm),
`normal` (a unit shading normal in world space), `texture` (a texture asset reference;
connectable only to an `image` node or an interface input), and `surface` (only the
surface nodes produce it). The only implicit conversion is the broadcast `float` →
`color3`/`vector2`/`vector3`. All other conversions use `convert.*` nodes.

**Slopes:** a `float` wire that reaches a normal input carries its value and its slope
along the texcoord's u and v (per mm). Pattern nodes compute slopes analytically or by a
fixed finite difference in pattern space, as `pat_bumps` does today. Maths nodes
propagate them by the chain rule (forward-mode differentiation in the code generator). A
slope is computed only for wires that need one, so plain colour patterns pay nothing.

**Anti-aliasing is built in:** every pattern node fades to its mean as its period
approaches the pixel footprint (today's `pat_fade` and `pat_weave_fade`). An author cannot
turn this off, so a graph cannot alias into moiré.

**Ranges:** a constant outside its input's range **refuses** on load, as `PbrParameters`
does today. Wires computed at run time can leave a range. The surface node clamps its
inputs, and maps NaN to the input's default, identically in both GPU back ends (§5.7) and in
the test-only reference.

**Fallbacks:** a connection may carry a constant `fallback` of its input's type, for example
`{"from": "rough", "fallback": 0.45}`; it defaults to the input's default. Fallbacks make up
the base response (§2): data, not shading.

### 3.1 Surface nodes

`standard_surface` has OpenPBR input names (a subset); its output is `surface`.

| Input | Type | Range | Default | Notes |
|---|---|---|---|---|
| `base_weight` | float | [0, 1] | 1 | |
| `base_color` | color3 | [0, 1] | 0.8 | Linear reflectance. |
| `base_metalness` | float | [0, 1] | 0 | |
| `specular_weight` | float | [0, 1] | 1 | |
| `specular_color` | color3 | [0, 1] | 1 | Edge tint of metals, scale of dielectric F0. |
| `specular_roughness` | float | [0.045, 1] | 0.3 | Perceptual; Solidcraft's lower bound. |
| `specular_ior` | float | [1, 3] | 1.5 | As `dielectric_ior` today; 1 means no interface. |
| `specular_roughness_anisotropy` | float | [0, 1] | 0 | 0 is isotropic. |
| `specular_rotation` | float | [−π, π] | 0 | Turns the tangent about the normal. |
| `geometry_tangent` | vector3 | unit | the texcoord's tangent | Usually wired from a projection. |
| `coat_weight` | float | [0, 1] | 0 | The same lobe as the path tracer's runtime `clear_coats` override. |
| `coat_color` | color3 | [0, 1] | 1 | Tint of light through the coat (new in the renderer, §5.5). |
| `coat_roughness` | float | [0.045, 1] | 0.1 | |
| `coat_ior` | float | [1, 3] | 1.5 | |
| `geometry_coat_normal` | normal | unit | the geometric normal | For orange peel on the coat only. |
| `geometry_normal` | normal | unit | the geometric normal | Base relief. |
| `transmission_weight` | float | [0, 1] | 0 | Requires `base_metalness` = 0 and opacity 1. |
| `transmission_color` | color3 | [0, 1] | 1 | Transmittance at `transmission_depth_mm`. |
| `transmission_depth_mm` | float | [0.001, 1000] | 1 | |
| `emission_luminance` | float | [0, 1000] | 0 | Relative, like the product lights; not calibrated. |
| `emission_color` | color3 | [0, 1] | 1 | |
| `geometry_opacity` | float | [0, 1] | 1 | Cut-out coverage, not glass. |

Not in v1, and **refused** rather than dropped: subsurface, sheen/fuzz, thin film,
thin-walled, displacement and specular IOR spectra. Each is a later node-set version with
its own pending format change.

`surface.mix` takes `fg` and `bg` (surface) and `mix` (float [0, 1]). In v1 it blends the
two surfaces' **parameters** (colours and scalars linearly, normals by normalised lerp), so
one BRDF is evaluated per pixel. This is enough for masks with sharp or moderately soft
edges: an etched mark, printed ink, a stripe. A true weighted sum of two BRDF evaluations
(MaterialX's `mix` of BSDFs) is a later option (decision O3).

### 3.2 Geometry and projection nodes

Bodies are B-rep tessellations without UVs, so every texcoord is **projected**. Each
projection has a `space` param, `object` (default: the pattern stays on the part as it
moves or is instanced) or `world`.

| Node | Inputs (type, range) | Params | Output |
|---|---|---|---|
| `geometry.position` | – | `space` | vector3, mm |
| `geometry.normal` | – | `space` | vector3, unit |
| `geometry.facing` | – | – | float, \|n·v\| in [0, 1] |
| `texcoord.planar` | `origin` vector3 (±1e7 mm), `u`, `v` vector3 (each 0.001–1e6 mm long, orthogonal) | `space` | texcoord. u and v span one repeat, as in the runtime planar maps ([runtime textures](render-runtime-textures.md)). |
| `texcoord.triplanar` | `origin`, `scale_mm` vector3 (0.001–1e6), `sharpness` float [1, 64] | `space` | texcoord. Its consumers up to the first non-pattern node are evaluated three times and blended. That cost counts three times against the budget. |
| `texcoord.cylindrical` | `origin`, `axis` (unit), `reference` (unit, ⟂ axis) vector3; `radius_mm` float (0.001–1e6) | `space`, `unwrap`: `arc_length` \| `angle` | texcoord: u around (mm of arc at `radius_mm`, or radians), v along the axis (mm). |
| `texcoord.transform` | `texcoord`, `offset` vector2 (±1e6 mm), `scale` vector2 (1e-3–1e3), `rotation` float [−π, π] | – | texcoord |

### 3.3 Pattern nodes

Every pattern takes a `texcoord`, or a `vector3` position for 3D noise, and a `seed`
(integer 0–2³¹−1). Each has outputs `value` (float, in [0, 1], mean 0.5 unless stated) and,
where meaningful, `slope`.

| Node | Inputs | Params | Notes |
|---|---|---|---|
| `pattern.noise` | `scale_mm` (0.001–1e4), `lacunarity` [1, 4] = 2, `gain` [0, 1] = 0.5 | `kind`: `value` \| `gradient` \| `cellular`; `dimensions`: 2 \| 3; `octaves`: 1–8 | fBm with an unrolled octave count. `value` 2D is today's `pat_noise`. |
| `pattern.brushed` | `density_per_mm` (0.1–1000), `streak_mm` (0.01–1000), `contrast` [0, 1], `jitter` [0, 1] | – | Fine lines along u: noise stretched by `streak_mm / (1/density)`. Its tangent output is the texcoord's u, which feeds `geometry_tangent`. |
| `pattern.weave` | `pitch_mm` vector2 (0.01–100), `crown` [0, 1] = 1, `fibres` (0–32) = 7, `fibre_contrast` [0, 1] = 0.3 | `kind`: `plain` \| `twill_2x2` | Today's glass-cloth weave, generalised (carbon-fibre twill). |
| `pattern.checker` | `size_mm` vector2 (0.001–1e6) | – | Values 0 or 1, smoothed by the footprint. |
| `pattern.grid` | `size_mm` vector2, `line_mm` (0.0001–1e6) | – | 1 on lines. |
| `pattern.gradient` | `start`, `end` float (±1e6 mm along u) | `kind`: `linear` \| `radial` | Ramp clamped to [0, 1]; `radial` measures the uv distance from the origin. |
| `image` | `file` texture; `default` color3 | `space`: `srgb` \| `linear`; `wrap`: `repeat` \| `clamp` \| `mirror`; `channel`: `rgb` \| `r` \| `g` \| `b` \| `a` | Outputs `color` color3, `value` float (the luminance or the chosen channel), `alpha`. Filtered with mips (§4.3). |
| `normal.from_height` | `height` float (with slope), `relief_mm` [0, 1] | – | Perturbs the geometric normal along the texcoord's tangent frame, as `look_detail` does. |
| `normal.map` | `color` color3 (linear, from `image`), `strength` [0, 1] | `convention`: `opengl` \| `directx` | Tangent-space normal map, in the texcoord's frame. |
| `normal.blend` | `a`, `b` normal, `mix` [0, 1] | – | Normalised lerp. |

### 3.4 Maths nodes

Inputs default to a value with no effect, so a partly wired node is still valid. Types are
float or vector (component-wise) unless stated.

`constant`, `add`, `subtract`, `multiply`, `divide` (the denominator is held at least
1e-6 away from zero), `min`, `max`, `clamp`, `abs`, `floor`, `fract`, `power` (the base
is clamped to ≥ 0), `sqrt` (of max(x, 0)), `sin`, `cos`, `mix`, `step`, `smoothstep`,
`remap` (in-range → out-range, clamped), `invert` (1 − x), `dot`, `length`,
`normalize` (zero → +Z), `combine3`, `separate3`, `luminance`, `hsv_adjust`
(colour only; hue ±π, saturation and value scales 0–4), `convert.float_to_color3` and the
other explicit conversions.

There are no loops, no conditionals that branch control flow (`step` and `smoothstep` are
select-free arithmetic), and no recursion. The generated code is therefore straight-line,
with bounded cost, and derivatives stay valid everywhere (§5.1).

### 3.5 Limits

| Limit | Value | Why |
|---|---|---|
| Graphs per design | 4096 | A design library, not per-face copies. |
| Nodes per graph | 128 | Larger than every worked example ×4. |
| Connections per graph | 512 | Each input has at most one connection. |
| Interface inputs | 32 | |
| Ids | 1–64 bytes `[a-z0-9_.-]`, unique within a graph | Never used as identifiers in generated code. |
| Names | 1–256 UTF-8 bytes, no control characters | As body names. |
| Cost after expansion (§3.6) | 2000 units | Bounds per-pixel ALU (target, §5.4). |
| Texture samples after expansion | 8 | Triplanar images count three each. |
| Distinct textures per graph | 4 | WebGL2 has 16 sampled textures per stage (§5.3). |
| Texture assets | ≤ 16384 px a side, ≤ 256 MiB encoded, ≤ 256 per design, ≤ 1 GiB decoded per design | Safety only (§4.3); the renderer clamps the resident size. |
| Uniform constants per graph | 256 floats (64 vec4) | Fits a 16 KiB uniform page with ≥ 16 materials. |
| Shapes compiled per design | not limited; the GPU cache holds 64 (LRU) | §5.2 |

### 3.6 Cost model

Each node kind has a fixed cost in "units", roughly ALU operations, taken from its prelude
function: `constant` 0, arithmetic 1, `smoothstep` 4, `pattern.noise` 2D value 20 per
octave (3D 40, gradient ×1.5, cellular ×3), `pattern.weave` 60, `image` 8 plus one sample,
and `standard_surface` 0 (the lighting is fixed cost). Slope-carrying wires cost ×3 and
triplanar subgraphs ×3. Validation computes the expanded cost and refuses a graph over the
budget, so an author learns at once, not at frame time. The table is part of the node
catalogue that `appearance.graph.nodes` returns (§7.2).

### 3.7 Today's looks, expressed

- **Opaque PBR look (`pbr`):** it stays in the file exactly as today and is never
  rewritten (§4.4). The renderer reads it, in memory, as one `standard_surface` with
  `base_color` = the look's `srgb8` colour, `base_metalness`, `specular_roughness` and
  `specular_ior`. Only an explicit "Convert to material graph" edit creates a graph.
- **Clear coat:** `coat_weight`, `coat_roughness` and `coat_ior`, which are exactly
  `ClearCoat`'s weight, roughness and IOR. A pigmented body under lacquer keeps today's
  rule: base `specular_ior` = 1 (no second interface).
- **`Weave`:** `pattern.weave` (plain, pitch 0.577 × 0.819 mm) on a `texcoord.triplanar`
  in object space, `scale_mm` (1, 1, 0.173). Its `value` goes through
  `remap`(0.5 ± 0.2·strength) and multiplies `base_color`; its height goes through
  `normal.from_height` (6 µm·strength). The edge's 0.25 strength is
  `mix(0.25, 1, smoothstep(0.65, 0.75, |geometry.normal.z|))`. This equals today's look on
  faces and on axis-aligned edges. A diagonal edge is blended across two planes, where
  today's look runs along the edge exactly. That is accepted; the alternative is a
  dedicated edge projection node later.
- **`OrangePeel`:** two `pattern.noise` nodes (value, 2D) at 2.4 mm and 0.9 mm, combined by
  `add` with relief weights 6 µm and 1 µm, into `normal.from_height` →
  `geometry_coat_normal` (or `geometry_normal` with no coat).
- **`PeelOverWeave`:** both of the above; the weave drives only the albedo.
- **`InkGrain`:** noise at 0.05 mm and 0.16 mm, an albedo of ±18 %, and 2 µm of relief.

The built-in library ships these four as graphs (§10, track G). The `SurfacePattern` enum
and `pbr_tri_with`'s detail vertices are **not part of the upstream package**: graphs
replace them before submission. The fork keeps them only until Pcbcraft has moved to graphs
(track H).

### 3.8 Worked examples

Each example is a complete graph in the proposed JSON (§4.1); `"srgb8"` marks an exact
sRGB constant. Values are visual estimates, not measured optical data.

**Brushed nickel silver** (a stamped shield can, brushed along X):

```json
{"name": "Brushed nickel silver",
 "nodes": [
  {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"},
   "inputs": {"origin": [0,0,0], "u": [1,0,0], "v": [0,1,0]}},
  {"id": "lines", "kind": "pattern.brushed",
   "inputs": {"texcoord": {"from": "uv"}, "density_per_mm": 40, "streak_mm": 4, "contrast": 0.6, "jitter": 0.3, "seed": 7}},
  {"id": "rough", "kind": "remap", "inputs": {"in": {"from": "lines"}, "out_min": 0.41, "out_max": 0.49}},
  {"id": "tint", "kind": "remap", "inputs": {"in": {"from": "lines"}, "out_min": 0.97, "out_max": 1.03}},
  {"id": "base", "kind": "multiply", "inputs": {"a": [0.80, 0.77, 0.70], "b": {"from": "tint"}}},
  {"id": "surface", "kind": "standard_surface", "inputs": {
     "base_color": {"from": "base"}, "base_metalness": 1,
     "specular_roughness": {"from": "rough"}, "specular_roughness_anisotropy": 0.6,
     "geometry_tangent": {"from": "lines.tangent"}}}],
 "output": "surface"}
```

Cost: planar 4 + brushed 30 + 2 × remap 2 + multiply 1 = 39 units.

**Laser-etched metal** (a logo on the same can; the mask is a PNG asset, planar-projected
onto the lid):

```json
{"name": "Laser-etched nickel silver",
 "interface": [{"name": "mark", "type": "texture"}],
 "nodes": [
  {"id": "lid", "kind": "texcoord.planar", "params": {"space": "object"},
   "inputs": {"origin": [-6,-4,0], "u": [12,0,0], "v": [0,8,0]}},
  {"id": "mask", "kind": "image", "params": {"space": "linear", "wrap": "clamp", "channel": "a"},
   "inputs": {"texcoord": {"from": "lid"}, "file": {"interface": "mark"}}},
  {"id": "pits", "kind": "pattern.noise", "params": {"kind": "cellular", "dimensions": 2, "octaves": 1},
   "inputs": {"texcoord": {"from": "lid"}, "scale_mm": 0.03, "seed": 3}},
  {"id": "pit_n", "kind": "normal.from_height", "inputs": {"height": {"from": "pits"}, "relief_mm": 0.003}},
  {"id": "plain", "kind": "standard_surface", "inputs": {"base_color": [0.80,0.77,0.70], "base_metalness": 1, "specular_roughness": 0.45}},
  {"id": "etched", "kind": "standard_surface", "inputs": {"base_color": [0.30,0.29,0.27], "base_metalness": 1, "specular_roughness": 0.70, "geometry_normal": {"from": "pit_n"}}},
  {"id": "surface", "kind": "surface.mix", "inputs": {"fg": {"from": "etched"}, "bg": {"from": "plain"}, "mix": {"from": "mask.value"}}}],
 "output": "surface"}
```

The mark is an interface input, so each look (or a runtime override, §7.3) binds its own
image without recompiling: a texture binding is not part of the shape.

**Glossy solder mask over copper** (a board's trace; the copper shows through a tinted
coat). This is physically closer than today's precomputed composite colour, and it needs
`coat_color` (§5.5):

```json
{"name": "Gloss green solder mask over copper",
 "nodes": [
  {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"}, "inputs": {"u": [1,0,0], "v": [0,1,0]}},
  {"id": "peel_a", "kind": "pattern.noise", "params": {"kind": "value", "dimensions": 2, "octaves": 1}, "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 2.4, "seed": 1}},
  {"id": "peel_b", "kind": "pattern.noise", "params": {"kind": "value", "dimensions": 2, "octaves": 1}, "inputs": {"texcoord": {"from": "uv"}, "scale_mm": 0.9, "seed": 2}},
  {"id": "peel_b6", "kind": "multiply", "inputs": {"a": {"from": "peel_b"}, "b": 0.1667}},
  {"id": "peel", "kind": "add", "inputs": {"a": {"from": "peel_a"}, "b": {"from": "peel_b6"}}},
  {"id": "coat_n", "kind": "normal.from_height", "inputs": {"height": {"from": "peel"}, "relief_mm": 0.006}},
  {"id": "surface", "kind": "standard_surface", "inputs": {
     "base_color": [0.955,0.638,0.538], "base_metalness": 1, "specular_roughness": 0.35,
     "coat_weight": 1, "coat_color": [0.10,0.45,0.12], "coat_roughness": 0.12, "coat_ior": 1.52,
     "geometry_coat_normal": {"from": "coat_n"}}}],
 "output": "surface"}
```

Over FR-4 instead of copper, the base becomes a dielectric of the laminate colour with
`specular_ior` 1, plus the weave (§3.7).

**Black epoxy with a clear coat** (an EDM-textured moulded body, lacquered top):

```json
{"name": "Black epoxy, clear coated",
 "nodes": [
  {"id": "p", "kind": "geometry.position", "params": {"space": "object"}},
  {"id": "edm", "kind": "pattern.noise", "params": {"kind": "cellular", "dimensions": 3, "octaves": 2}, "inputs": {"position": {"from": "p"}, "scale_mm": 0.04, "seed": 11}},
  {"id": "edm_r", "kind": "remap", "inputs": {"in": {"from": "edm"}, "out_min": 0.40, "out_max": 0.52}},
  {"id": "surface", "kind": "standard_surface", "inputs": {
     "base_color": [0.022,0.022,0.024], "specular_roughness": {"from": "edm_r"}, "specular_ior": 1,
     "coat_weight": 1, "coat_roughness": 0.08, "coat_ior": 1.5}}],
 "output": "surface"}
```

The base IOR is 1 because the coat is the only interface (today's rule).

**Aluminium electrolytic sleeve** (a PVC sleeve with a polarity stripe; the printed text is
an image on the same cylindrical projection):

```json
{"name": "Electrolytic sleeve, blue",
 "interface": [{"name": "sleeve", "type": "color3", "default": [0.020,0.045,0.16]},
               {"name": "print", "type": "texture"}],
 "nodes": [
  {"id": "cyl", "kind": "texcoord.cylindrical", "params": {"space": "object", "unwrap": "angle"},
   "inputs": {"origin": [0,0,0], "axis": [0,0,1], "reference": [1,0,0], "radius_mm": 4}},
  {"id": "ang", "kind": "separate3", "inputs": {"in": {"from": "cyl.uv"}}},
  {"id": "stripe", "kind": "smoothstep", "inputs": {"edge0": 2.6, "edge1": 2.62, "in": {"from": "ang.x"}}},
  {"id": "text", "kind": "image", "params": {"space": "linear", "wrap": "repeat", "channel": "a"},
   "inputs": {"texcoord": {"from": "cyl"}, "file": {"interface": "print"}}},
  {"id": "ink", "kind": "max", "inputs": {"a": {"from": "stripe"}, "b": {"from": "text.value"}}},
  {"id": "pvc", "kind": "standard_surface", "inputs": {"base_color": {"interface": "sleeve"}, "specular_roughness": 0.35, "specular_ior": 1.54}},
  {"id": "printed", "kind": "standard_surface", "inputs": {"base_color": [0.80,0.80,0.78], "specular_roughness": 0.45, "specular_ior": 1.5}},
  {"id": "surface", "kind": "surface.mix", "inputs": {"fg": {"from": "printed"}, "bg": {"from": "pvc"}, "mix": {"from": "ink"}}}],
 "output": "surface"}
```

The aluminium top disc is a separate face look ("Aluminium", metal, roughness 0.30).

### 3.9 Later extensions (not v1)

- Further OpenPBR lobes (sheen, thin film, subsurface), closure-level `surface.mix`, and an
  edge-following projection.
- `.mtlx` import and export of the subset, mapping node kinds 1:1 where MaterialX has the
  same node (`mix`, `noise3d`/`fractal3d`, `cellnoise3d`, `image`, `triplanarprojection`,
  `checkerboard`, `ramplr`, `normalmap`, `heighttonormal`, `open_pbr_surface`).
- **Sandboxed custom-WGSL node.** It would carry one pure function over typed ports. Naga
  would validate it after a rewrite that bans bindings, pointers, loops and derivatives, and
  it would have an author-declared cost that the validator checks against a measured
  instruction count. This is mentioned only as a possibility: it moves trust onto shader
  code, so it needs its own design and review.

## 4. File format

### 4.1 Where it lives

Graphs live in a design-level table under `appearances`, and looks refer to them by id.
Texture assets live beside them and are stored as container blobs.

```json
"appearances": {
  "graphs": {
    "brushed-nickel-silver": {
      "name": "Brushed nickel silver",
      "interface": [],
      "nodes": [ … ],
      "output": "surface",
      "source": {"library": "6f1c…uuid", "entry": "brushed-nickel-silver", "revision": 3}
    }
  },
  "textures": {
    "9b2e…sha256": {"name": "logo.png", "width": 512, "height": 256, "image_blob": "blobs/9b2e….png"}
  },
  "bodies": {
    "Can": {"name": "Brushed nickel silver", "color": [227,224,218], "opacity": 1,
            "graph": "brushed-nickel-silver", "values": {}}
  }
}
```

- `Look` gains `graph: Option<GraphId>` and `values: BTreeMap<String, InterfaceValue>`,
  both omitted when empty. `graph` and `pbr` are mutually exclusive. `pbr` remains a valid,
  unchanged form, read and written as today. Looks without either keep scalar shading.
- `appearances.graphs` and `appearances.textures` are omitted when empty, so a design
  without graphs serialises exactly as today.
- A look's `color` stays required. With a graph it is the colour the standard view shows
  (the base response's base colour, sRGB8), which the assigning command computes. It is also what
  STEP/3MF colour export and older readers' tools use.
- `source` is generic provenance (library id, entry id, revision). It is optional and
  inert, and drives only an explicit "update from library" with an impact preview (§7.2).
- `layout` is an optional, **editor-only** map from node id to a canvas position, `[x, y]`.
  Renderers, the IR and the shape key ignore it, so moving a node never changes a picture or
  recompiles anything. Validation bounds it. A later node canvas (O2) stores positions
  there without a format change; the layer list leaves it empty.
- A graph with `geometry_opacity` < 1 or `transmission_weight` > 0 requires the look's
  `opacity` = 1. Opacity stays the scalar legacy blend; it does not combine with graph
  optics, which keeps today's "PBR requires opacity 1" rule.
- **Physical-material default looks** (`material.create/edit`) use the same `Look`, so
  they reference graphs too. A physical material's density and engineering cards remain
  separate.
- **Textures:** in a container (`.sc`), `image_blob` is `blobs/<sha256>.png|jpeg`, using
  the canvas blob mechanism that exists today ([design containers](design-container.md)).
  In a plain JSON design, `data` holds base64, as canvases do. The key is the SHA-256 of the
  exact encoded bytes. Textures no graph or look references are dropped on save. Graphs
  are design data and stay until deleted, even when no look uses them. The existing
  `png`/`jpeg` blob kinds are reused, so the container format (`solidcraft-container/1`)
  does not change.

### 4.2 Shared material libraries

The local material library ([local libraries](book/src/dev/local-libraries.md)) becomes
**portable catalog v3**: catalog v2 plus `graphs` and `textures`. A v3 catalog with
textures is a ZIP container (`container.json`: `{"format":"solidcraft-library/1"}`,
`catalog.json`, `blobs/…`) read by the same bounded container reader. One without textures
may stay plain JSON. `library.apply` copies a graph (and its textures) into the design and
records `source`, so the design never depends on the library file. A collision of graph ids
refuses unless the command names a new id: an existing graph is never silently replaced.
Export writes v2 when the exported snapshot has no graphs, so material-free libraries stay
readable by the last release (the same rule as designs, §4.4). Explicitly exporting v2 from
a snapshot with graphs refuses, as exporting v1 refuses PBR loss today.

### 4.3 Texture limits

**Files keep images at full quality.** The file's limits exist only for safety against hostile
input; they never cap quality. **The renderer chooses the resident resolution** for each
device.

| Limit (file, safety) | Value | Why this number |
|---|---|---|
| Encodings | PNG, JPEG; 8-bit, and 16-bit PNG for data maps (height) | What `texture_input` admits; ICC and EXIF orientation as it handles them today |
| Size | 1–16384 px per side | The largest 2D texture any current desktop GPU holds in one piece (`max_texture_dimension_2d` is 16384 on Apple, most Vulkan/D3D12 parts). Larger images cannot be resident anywhere, so the bound costs no quality. |
| Encoded bytes | ≤ 256 MiB per image | The container's existing per-part cap (`design-container.md`), so one rule bounds every blob. A noisy 16384² PNG fits; typical maps are far smaller. |
| Decoded memory per design | ≤ 1 GiB of decoded pixels (RGBA8 counts 4 B/px, 16-bit data 2 B/px), checked from the headers before any decode | Exactly one maximum-size RGBA8 image, or sixteen at 4096². It leaves room inside wasm32's 4 GiB address space and on 8 GiB machines while the app's own geometry is loaded. |
| Textures per design | ≤ 256 | Hostile-input protection for the per-texture bookkeeping (table entries, blob lookups, bind groups and worker jobs), not quality. 256 holds the whole starter pack (48 recipes × 5 maps = 240) in one design. |
| Codec allocation guard | the header's decoded size (≤ the remaining design budget) + 64 MiB codec scratch, passed as `image::Limits::max_alloc` | The real defence against decompression bombs: a header that lies about its size cannot make the codec allocate past the budget. It replaces today's fixed 64 MiB guard, which only works because the edge is 2048. |

**Resident resolution (renderer, never stored).** Each texture is made resident at the
largest mip level that fits both the device's `max_texture_dimension_2d` and a GPU memory
budget for material textures:

- **Native:** usually the full size; the budget is 2 GiB, with the least-recently-drawn
  textures dropping a level first.
- **WebGL2:** 2048 px per side (the downlevel limit) and a 256 MiB budget.
- **The path tracer** takes the same clamp against its `max_storage_buffer_binding_size` and
  its texture-buffer budget (`job::textures::MAX_TEXTURE_PIXELS_BYTES`).
- **Downsampling above the device limit:** when an image exceeds the device limit, the
  decode worker box-filters it by powers of two down to the first level that fits. This is
  data preparation, done once per load. The GPU then builds the rest of the mip chain.

**Mips** are built on the GPU by render passes: each level is a linear-filtered downsample
drawn into the next. This works on WebGL2, which has no compute.

**Upstream's `texture_input` is raised generically** as part of the package (track C3):

- `MAX_TEXTURE_EDGE` goes from 2048 to 16384, and `MAX_TEXTURE_BYTES` from 16 MiB to 256 MiB;
- the fixed `MAX_CODEC_BYTES` becomes the header-derived guard above;
- `MapRole::Height` is added;
- `texture_lod`, `texture_sampling` and HDR normalization follow the same constant, and their
  tests are written against it.

The path tracer's runtime `raytrace.textures` request keeps its own 1024 × 1024 job limit
unchanged (no new path-tracer features). **Effect on the starter pack and
`render.library`: none.** Its maps are 512 × 512, a few hundred KiB, and admitted before
and after.

Decoding uses `solidcraft_render::texture_input` on a worker; this is data preparation.
Rendering into a level of an `Rgba8UnormSrgb` texture on WebGL2 (`SRGB8_ALPHA8` is
colour-renderable in ES 3.0) is verified in track C3. If a driver fails it, the fallback is a
linear texture with decode in the shader, never CPU mips. Until a texture has decoded, its
`image` node reads its `default` colour.

### 4.4 Compatibility and the pending format change

**How the written format number is decided today.** `Formats::current()`
([`format.rs`](../crates/doc/src/format.rs)) is the last folded format, plus one while
`format/pending/` holds any change. `format::read` sets every loaded document's `format` to
that `FORMAT` and its `format_pending` to all pending names. Saving serialises them as they
are. The number therefore depends on the **build, not the content**: once any pending change
exists, every design this build saves gets N+1, whether or not it uses the change, and the
last release refuses all of them. After the release fold, every save writes the new number
again. This is fine for most changes, but it would make "a material graph exists somewhere
in the code" break every user's files for the last release.

**Proposal: write the oldest format a design needs (opt-in per change).**

- `Part` gains `used: Option<fn(&Value) -> bool>`, a predicate on the design's saved
  JSON. `None` means "always used", which is what every existing part gets, so nothing about
  today's changes moves.
- On save, `Family::written_for(&Value)` picks the written number. It starts at the
  current format and steps down past every format whose parts all declare `used` and are
  all unused by this design. It stops at the first format with an undeclared or used part,
  or at the last hand-written format. The result is that format's number, and, if it is a
  pending format, its full `format_pending` list as today.
- The rule lives in one place (the design writer, JSON and container alike, and autosave
  and recovery). The in-memory `Document::format` stays `FORMAT`.
- `appearance-graphs.rs` declares `used = appearances.graphs is non-empty`. A design
  without graphs has no new JSON at all: the new fields are omitted when empty and `pbr` is
  unchanged. While the change is pending, such a design is therefore written with the last
  release's number, byte for byte what that release would write.
- After the release fold the predicate travels with the part into `v<N+1>/`. Release N+1
  keeps writing N for designs without graphs, as long as no other change of N+1 is used or
  undeclared. The mechanism is generic, so later changes can opt in the same way.

**Files without materials** open and save exactly as today: the same JSON, the same
format number, and no `format_pending` added. This is enforced by a test that saves every
design in `apps/solidcraft-cli/tests/corpus` and the format tests' fixtures, and compares
the bytes with the output of the build before the change. Also tested:

- a design with a graph writes N+1 and lists `appearance-graphs`;
- deleting its last graph writes N again;
- another pending part without a predicate still forces N+1, so the rule is conservative;
- track I's evidence includes the last release's `solidcraft-cli` opening the down-levelled
  files.

If upstream declines the `format.rs` change, the package still works. The only loss is that
material-free designs saved during the release cycle get N+1, as every change does today
(decision O8).

**`pbr` looks are not migrated.** They are already part of the released format, so
rewriting them into graphs would force N+1 onto every file that uses them, which is exactly
what the founder ruled out. A `pbr` look reads and writes unchanged. In the realistic view it renders
as the equivalent one-node graph, with bit-identical pixels to today's look (the same f32
values reach the same `pbr_shade`). `appearance.assign {pbr: …}` keeps writing `pbr`. A
coat, pattern or texture needs a graph. The founder's rule is met: no new fixed field
appears, and the one existing field stays for compatibility.

**The pending file**, `crates/doc/src/format/pending/appearance-graphs.rs`, has no step,
because older designs need no rewrite:

```rust
//! Looks can name a material graph (`appearances.graphs`, by id) with interface values, and
//! graphs can use texture assets (`appearances.textures`, container blobs or base64). An
//! older design gets none: its looks (scalar or `pbr`) are unchanged. A design without
//! graphs is still written in the older format (`USED`); the last release refuses a design
//! that has graphs (the newer format number).
pub const USED: fn(&serde_json::Value) -> bool = |v| {
    v.pointer("/appearances/graphs").and_then(serde_json::Value::as_object).is_some_and(|g| !g.is_empty())
};
```

**What an older release does with a file that has graphs:** it reads the format number,
finds it newer than any it knows, and refuses to open the design with its standard message
("this design was saved by a newer SolidCraft (file format N+1 …): update SolidCraft to open
it"). It neither opens it partly nor drops the graphs. Recovery files behave the same.

Solidcraft is a 0.x package, so the package adds no other backwards-compatibility
feature: material-free files staying exactly readable is enough, and there is no
downgrading export.

**Node-set versions:** the node catalogue is versioned with the design format. A new node
kind or input arrives in its own pending file, with its own `used` predicate. An unknown
kind, param or input refuses (`deny_unknown_fields`) rather than being dropped.

### 4.5 What the graph builds on upstream (no second copy)

Upstream already has textured material resources, texture admission, a path-tracer texture
path and lighting rigs, all marked `unbound` or flattened today. Graphs **bind** them; nothing
here is duplicated.

**1. The starter surface pack, the Selene pack and the material catalogue.**
`assets/render/starter/materials/` holds 48 original recipes (`SURF-*`). Each has five PNG
maps (base colour, OpenGL normal, roughness, metallic, 16-bit height), a tile size in
millimetres and a seed, and the manifest marks them `runtime_status: unbound`.
`assets/render/selene/materials/` is the Selene appearance pack, and
`docs/render-materials/materials.json` is the `MAT-*` concept catalogue they cite. A
`SURF-*` maps to a graph as follows:

| Recipe data | Graph |
|---|---|
| `base_color` (sRGB) | `image` (`space: srgb`) → `base_color` |
| `normal_open_gl` | `image` (linear) → `normal.map` (`convention: opengl`) → `geometry_normal` |
| `roughness`, `metallic` | `image` (linear, `channel: r`) → `specular_roughness`, `base_metalness` |
| `height` (16-bit) | used only when a recipe has no normal map: `image` → `normal.from_height` |
| `tile_mm` | `texcoord.triplanar` `scale_mm`, or `texcoord.planar` when a frame is given |
| `pattern: brushed` and the like | `specular_roughness_anisotropy` and a tangent from the texcoord; the per-recipe amount is an authored estimate in the binding table, recorded as such |

The maps are copied into the design's textures (`appearances.textures`, by SHA-256, the
manifest's own digests), so the design stays self-contained. The graph's `source` records
`{library: "solidcraft-starter", entry: "SURF-001", revision: resource_version}`. The `MAT-*`
catalogue stays a concept backlog: a `MAT-*` gains a graph only through a `SURF-*` (or a
Selene entry) that cites it.

**2. `render.library.appearance.apply`** (`crates/engine/src/cmd/render/library.rs`).

- **`uniform_opaque` stays exactly as it is** (decision O17): colour + `pbr`, omitting
  `["spatial_maps", "height_displacement", "true_anisotropic_brdf"]`. Scripts that rely on it
  see no change, and the design format is untouched by it.
- **A new variant `graph`** builds the graph above, assigns it, and returns the same reply
  shape.
  - Its `omitted` list drops `spatial_maps` and `true_anisotropic_brdf` in the Realistic
    look.
  - **It keeps `height_displacement`**, because graphs never move geometry: height feeds only
    the shading normal. Saying otherwise would hide a gap. Displacement stays a separate
    future feature.
  - Its optional `frame` chooses a planar projection; without one, it is triplanar in object
    space.
  - Selene entries get the same `graph` variant where their pack has maps; otherwise their
    `baseline_projection` becomes a one-node graph.
- `gpu_planar_maps` (the path tracer's job-scoped maps) stays as it is: it is a runtime job
  binding, not a design look.

**3. `solidcraft_render::texture_input`** (`MapRole`, `Interpretation`, `TextureSource`,
`DecodedTexture`, bounded decode on a caller-owned worker) is **the** admission and decoder.

- Design textures and `image` nodes go through it; there is no second decoder.
- Its limits are raised generically to the file's safety bounds (§4.3); the renderer, not the file, clamps resolution.
- The one addition is `MapRole::Height` (16-bit data, `Interpretation::Data`) for the
  starter pack's height maps (track C3).
- An `image` node's role follows from its use: `space: srgb` reads as `BaseColor`, linear
  data as the matching data role.

**4. The path tracer's texture path** (`crates/gpu/src/path/textures.rs`, `textures.wgsl`,
`job::textures::{TextureMap, BaseColorMode, PlanarFrame}`, `texture_surface`) is
**reused** by the path back end (§5.7):

- `image` nodes pack their pixels into the same `texture_words` storage buffer, with the same
  identity de-duplication (`TextureMap::identity`), and sample with the same bilinear
  `texture_sample`. These move into the shared library as `sc_` functions (track S).
- `texture_surface` stays for runtime `raytrace.textures` requests on non-graph bodies, so
  existing jobs and their parity tests are unchanged.
- A graph look's maps never go through the runtime request; the graph does the same work
  through the same buffer.

**5. Lighting resources.** `job::lighting::StudioLighting` (sun, uniform environment, at most
four rectangular `AreaLight`s), `render_resources::HdrResource` (validated embedded HDR
environments) and the starter lighting rigs (`assets/render/starter/lighting/`, 24 `STU-*`
rigs from `docs/render-materials/studio-presets.json`) already exist. The rigs are rectangular
area lights scaled to the subject's extent B, calibrated by `CAL-LIGHT-001`, and
`runtime_binding: unbound`. The default Realistic studio (§5.8) is built from them:

| Reused | Genuinely new |
|---|---|
| A starter **rig** as the studio description: its key, fill and rim are `AreaLight`s placed in multiples of B (the room "sized to the scene"), with gains from `CAL-LIGHT-001` | The **real-time binding** of a rig: its area lights baked into the studio cube and used as direct lights (softbox α widening) |
| The renderer branch's CPU studio bake (data preparation) and split-sum/SH | **Box-projected** reflections against the rig's room |
| `StudioLighting`'s types for the rig's lights | A **cyclorama/horizon gradient**, because `StudioLighting.environment` is one uniform colour; it is added as an optional gradient on the rig, not a second studio type |
| `HdrResource`: a design that already has an HDR environment in its render settings can light Realistic from it through the same bake (an option in track L, not the default) | Per-preset **exposure** calibrated against the rig |

The default rig is chosen in track L and reviewed with the golden frames (O15). The
procedural `Product`/`Soft`/`Contrast` presets of the renderer branch are re-expressed as
rigs, or removed if a starter rig covers them.

## 5. Renderer

### 5.1 Code generation

Validation produces a typed, topologically ordered **IR**: one SSA value per output,
types resolved, triplanar and slope expansions applied, and constants assigned to uniform
slots. The IR is a pure Rust type in `solidcraft-render` (no wgpu) and is shared by both back
ends (§5.7). `solidcraft-gpu`'s raster back end turns it into one WGSL function:

```wgsl
// generated for shape 0x5c1e…; every value comes from the material's uniform slots
fn material_eval(c: MatCtx) -> Surface {
    let k = c.base;                                   // vec4 offset of this material
    let t0 = tc_planar(c.p_obj, m[k+0u].xyz, m[k+1u].xyz, m[k+2u].xyz, c.fp_obj);
    let v1 = pat_brushed(t0, m[k+3u].x, m[k+3u].y, m[k+3u].z, m[k+3u].w, bitcast<u32>(m[k+4u].x));
    let v2 = remap(v1.value, m[k+4u].y, m[k+4u].z);
    var s = surface_default();
    s.base_color = m[k+5u].xyz * remap(v1.value, m[k+4u].w, m[k+5u].w);
    s.metalness = m[k+6u].x;
    s.roughness = v2;
    s.anisotropy = m[k+6u].y;
    s.tangent = t0.tangent;
    return surface_sanitise(s);
}
```

- All pattern mathematics lives in a fixed **prelude** (today's `pat_hash`, `pat_noise`,
  `pat_weave_parts` and `pat_bumps`, generalised). The generator only emits calls,
  arithmetic and `select`. Prelude functions (the shared modules of §5.7) are tested once, against analytic values,
golden GPU frames and the test-only reference;
  generated code adds only composition.
- Identifiers are generated (`t0`, `v1`, …). No user string ever reaches WGSL. Constants
  are uniforms, not literals; only shape params (octaves, enum kinds) appear as code. The
  generated module is validated by naga before pipeline creation. A failure is an error
  value that selects the fallback (§5.6); it never panics.
- The code is straight-line, so `fwidth` and texture derivatives stay in uniform control
  flow. The existing derivatives guard, which scans `pbr::SOURCE`, extends to generated
  modules.

### 5.2 Specialisation and caching

- **Pipeline per shape:** the shape key is a 128-bit hash of the canonical IR without
  values. Graphs that differ only in values share one pipeline; a value edit uploads
  uniforms and never recompiles.
- **Uniform values:** one uniform page is `array<vec4<f32>, 1024>` (16 KiB, the WebGL2
  uniform-block limit). Each material takes a contiguous run of at most 64 vec4s. A vertex
  carries its material's vec4 offset as a flat `u32`, so **one draw covers every material
  of one shape** on a page. Pages are rewritten only when a material's values hash changes.
- **Vertices:** a graph-drawn vertex is position, normal and material offset (28 bytes),
  plus an object-space position (40 bytes) for shapes that read object space. Instanced
  meshes are already in object space. Today's 48-byte `pbr_tri` vertices remain for
  embedding clients and `pbr` looks. The 64-byte detail vertices stay in the fork only,
  until track H; they are not in the package.
- **Cache:** at most 64 live pipelines (LRU) per renderer. New shapes compile within a
  per-frame budget (target: at most two new pipelines a frame, and none in a frame already
  over 8 ms). Until a shape's pipeline exists, its materials draw their **base response** through the
  constant-graph pipeline, a GPU fallback that is always compiled. A new material never
  stalls a frame. On native, `wgpu` pipeline
  creation is still synchronous on the device thread; the budget bounds it.

### 5.3 WebGL2 path

The constraints of wgpu 30's `downlevel_webgl2_defaults` (checked in `wgpu-types` 30.0.1):

- **4 bind groups,** and all four are in use (0 frame uniforms, 1 accessibility, 2
  occlusion, 3 studio). The material page and its textures therefore join **group 3** with
  the studio cube. Group 3 becomes "studio + material": one bind group per (texture set,
  page), with the studio bindings repeated in each (cheap: the same views). The renderer
  ships in the same package, so this layout is designed once and never changes after
  review.
- **No storage buffers or storage textures** in any stage, hence uniform pages, not a
  material SSBO.
- **16 sampled textures and 16 samplers per stage:** the studio cube, the AO term and at
  most 4 material textures fit with room to spare.
- **15 inter-stage variables:** today's `PbrOut` uses 5; graph shapes add at most the
  object position and the flat material offset, 7 in all.
- **No compute:** mips are built by render passes. The studio bake stays the renderer
  branch's CPU data preparation (an environment texture, baked off the window thread); moving
  it to render passes is possible later.
- **GLSL ES 3.00** through naga's GL backend. The existing test (`view/ibl/tests.rs`, "The
  product look's PBR entry points translate to WebGL2's GLSL ES 3.00") is extended to
  translate **every shape in the test corpus** (all worked examples, the built-in library
  and a generated fuzz set). It must also cover dynamic indexing of a uniform-block member
  array by a flat varying, which ES 3.00 allows; the test checks that naga emits it.
- **Shader compile time** on WebGL2 (especially ANGLE over D3D) can be tens to hundreds of
  milliseconds per program. This is not measured, and it is why §5.2 never blocks on a
  compile.

The look's GPU frames have so far run only on native adapters; no browser has drawn a look
frame. A browser run of the graph path is part of the completion evidence (track I).

### 5.4 Cost per material and per frame (targets)

| Item | Target |
|---|---|
| Validation + IR + base response of a 128-node graph | ≤ 1 ms CPU (native, optimised) |
| WGSL generation + naga validation per new shape | ≤ 5 ms |
| Pipeline creation per new shape | ≤ 30 ms native, ≤ 150 ms WebGL2, off the critical frame by §5.2 |
| GPU memory per material | ≤ 1 KiB uniform + its textures (+⅓ for mips) |
| Draws per frame | one per (shape, page, texture set): ≤ 20 for a typical product or board scene |
| Fragment cost | constant-only graph = today's `fs_pbr`; a graph at the 2000-unit budget ≤ 1.5× today's look fragment time at 1080p |
| Value edit (slider drag) | uniform upload only, ≤ 0.1 ms, no rebuild of geometry |
| Selection and camera changes | no rebuild and no recompile |

They will be measured with the existing `gpu` perf harness and recorded in
`docs/perf.md`. Each stays labelled a target until then.

### 5.5 Fit with today's PBR, IBL and clear coat

- `fs_pbr` is split into `material_eval` (the generated function, or the built-in "vertex
  constant" evaluator that reproduces today's vertex path) and `pbr_shade(s: Surface, ctx)`,
  which holds today's lighting unchanged: product lights, studio split-sum and SH
  irradiance, `pbr_coated`, occlusion, softbox widening, PBR Neutral and dither. Graph and
  non-graph paths share every lighting line.
- **New in `pbr_shade`:**
  - *Anisotropic GGX for direct lights* (viewport only, in the shared `bsdf.wgsl`): the
    mathematics of `render::anisotropic_surface`, which serves as its oracle in tests
    ([anisotropic GGX](render-anisotropic-ggx.md)) ported to WGSL, with αt, αu from roughness and
    anisotropy as OpenPBR defines them.
  - *Anisotropic studio reflection:* an approximation that bends the reflection vector
    toward the tangent before the cube lookup (public technique, mathematics only, e.g. the
    Filament documentation). It must pass the white-furnace test within today's 5 %.
  - *`coat_color`:* the base lobe is multiplied by the coat colour once on entry and once on
    exit (a thin absorbing layer), with the `clear_coat.rs` oracle extended to match in tests.
  - *Emission:* added after lighting.
  - *Transmission:* see §5.6.
- The graph's `Surface` carries a coat normal separate from the base normal, which today's
  look cannot express (orange peel now perturbs the base normal too).
- **In the standard view mode** (§6, the default), nothing of this runs. A look draws its
  `color` exactly as today, and the existing `cpu-product` parity tests stay valid.
  Patterns, coats and textures show only in the realistic view.

### 5.6 Fallbacks

| Situation | Behaviour |
|---|---|
| Shape not compiled yet / compile failed / over the cache | The base response through the constant-graph GPU pipeline; a failure logs once per shape and reports in the render diagnostics. |
| Texture not decoded yet or failed | The image node's `default` colour. |
| GPU path tracer | The same IR through the path back end (§5.7); unsupported inputs approximated and reported. |
| CPU ray tracer, CPU product host (out of scope) | Today's behaviour: the display `color` for a graph look, the `pbr` values for a `pbr` look. |
| No GPU (the CPU viewport renderer) | The realistic view is unavailable: the switch is disabled with a tooltip and `view.look` refuses with "the Realistic view needs a GPU". The standard view works as today. |
| Standard view mode, STEP/3MF colour, Drawing | The look's `color` (for a graph, the base response's colour). |
| `transmission_weight` > 0 in the real-time view | Drawn as today's see-through glass bodies, tinted by `transmission_color`; no refraction. |
| A graph with inputs a GPU target lacks | The viewport supports every v1 input. The path tracer approximates the ones it has no lobe for, and reports each in `render.preflight` and the job result; nothing is dropped silently (O10). |

### 5.7 One material system, two GPU targets

The interactive viewport is the **primary** target. Upstream's GPU path tracer
(`crates/gpu/src/path/`, native only) is a **second target of the same material system**.
It gains no feature except evaluating graph materials, so a look renders from one
definition everywhere a GPU shades it. Sharing has four layers.

**1. One definition:** the graph in the file.

**2. One IR, and its relation to `OpaqueSurface`.** The IR lives in `solidcraft-render`, next
to `OpaqueSurface` and `ClearCoat` (`surface.rs`, `clear_coat.rs`), which the viewport, the
path tracer and the CPU renderer share today. **`OpaqueSurface` becomes the constant core of
the graph's output type; the graph neither replaces nor wraps it.**

- The surface node's output type is `SurfaceSample`: `OpaqueSurface` (base colour, metallic,
  roughness, dielectric IOR, optional `ClearCoat`) plus the inputs `OpaqueSurface` lacks
  (anisotropy and tangent, coat colour, coat and base normals, emission, transmission,
  opacity).
- A `pbr` look is exactly the constant graph whose `SurfaceSample` has default extensions,
  so the two convert losslessly both ways.
- `OpaqueSurface`'s admission bounds (roughness ≥ 0.045, IOR [1, 3] …) are the surface
  node's input ranges, from one table.
- The CPU renderer and CPU product host keep consuming `OpaqueSurface` from `pbr` looks,
  unchanged.
- The **base response** is a static read of the graph's output node: each constant input as
  it is, and each driven input's declared `fallback` constant (§4.1). It yields an
  `OpaqueSurface` without evaluating anything. It gives the look's display `color`, the
  fallback while a shape compiles (§5.2).

**3. One WGSL library.** The two shaders duplicate each other today. Both are assembled by
string concatenation: `path/mod.rs` uses `concat!(include_str!("path.wgsl"), …)`, and
`view.rs` uses `format!("{SHADER}\n{}", pbr::SOURCE)`.

| Function | Viewport (`pbr.rs`) | Path tracer (`path.wgsl`, `aging.wgsl`) |
|---|---|---|
| Safe normalise | `pbr_unit` | `unit` (same algorithm) |
| Schlick with the IOR-1 rule | `pbr_fresnel` | `base_fresnel` (same), plus a plain `fresnel` |
| GGX D and separable Smith | inline in `pbr_brdf` and `pbr_coated` | `distribution` + inline in `brdf` |
| Base BRDF (spec + Fresnel-weighted diffuse) | `pbr_brdf` | first half of `brdf` |
| Coat lobe and attenuation bound | `pbr_coat_bound`, `pbr_coated` | `coat_energy_bound`, second half of `brdf` |
| Lattice hash and value noise | `pat_hash`, `pat_noise` (2D) | `aging_hash`, `aging_noise` (3D) |
| Planar projection | none yet (proposed `tc_planar`) | `texture_uv` in `textures.wgsl` |

These become shared modules in `crates/gpu/src/shading/`, prefixed `sc_` so that no name
collides with either shader:

- `bsdf.wgsl`: normalise, Fresnel, GGX/Smith, base BRDF, coat, and anisotropic GGX
  evaluation for the viewport;
- `noise.wgsl`: hashes and value, gradient and cellular noise in 2D and 3D;
- `patterns.wgsl`: brushed, weave, checker, grid and gradient, with footprint fades;
- `projection.wgsl`: planar, triplanar, cylindrical.

Each is a plain `const &str`, concatenated into both shaders exactly as today; no new
dependency is needed. `naga_oil` (MIT OR Apache-2.0) would add real imports, but
concatenation already works, keeps one naga parse per module, and keeps the build simple
(decision O11).

The shared modules hold only pure functions over values. They have no bindings, no
storage, no pointers to storage and no derivatives: those stay in each shader, and the
viewport passes the footprint in. The modules therefore translate to GLSL ES 3.00, and the
existing WebGL2 translation test, which covers the viewport entry points, checks them on
every change. The path tracer's sampling and PDF (`sample_surface`, `surface_pdf`), its
media and its sheets stay path-only.

**4. One code generator, two back ends.** The front end (validation, IR, slot assignment,
cost) is shared. The back ends:

| | Raster back end (viewport) | Path back end (path tracer) |
|---|---|---|
| Runs on | native and WebGL2 | native only (storage buffers) |
| Emits | `material_eval(MatCtx) -> Surface`, one pipeline per shape (§5.2) | `graph_surface(t: Triangle, p: vec3<f32>, n: vec3<f32>) -> Triangle` for every shape in the job's scene, with a `switch` on the shape index, inside the one path pipeline |
| Values | uniform pages (16 KiB), vec4 offset in the vertex | a storage-buffer material table; the triangle names its record in `interior.z`, today "reserved" |
| Textures | 2D textures with GPU-built mips, samplers in group 3 | the existing `texture_words` storage buffer and bilinear `texture_sample` |
| Derivatives / filtering | `fwidth` footprint, mips | no derivatives in compute: patterns are evaluated unfiltered at the hit, as `aging.wgsl` is (path samples integrate over the pixel) |
| Recompile | on a new shape | once per captured job, when its shape set differs (jobs are immutable snapshots) |

`graph_surface` follows the existing hook pattern: like `texture_surface` and
`aged_surface`, it returns the hit's resolved `Triangle` (base, parameters, coat, shading
normal) before sampling. The integrator, its importance sampling, its PDF and its
random-number use are therefore unchanged, and graph evaluation draws no random numbers.

**What the path tracer does not gain.** Inputs it has no lobe for (anisotropy, coat colour)
are evaluated as isotropic and untinted. Each such approximation is reported, never silent:
`render.preflight` lists it per body, and so does the job result (decision O10). Emission
maps to its existing surface emission, transmission to its existing runtime transmission,
normals to its shading normal.

**Path-tracer parity for looks without graphs.**

- A triangle without a graph never enters `graph_surface`.
- `brdf` keeps its exact expression order: the shared module is extracted from
  `path.wgsl`'s version, and `pbr_brdf` is ported onto it. Its frames are within the
  existing test tolerances, and the viewport parity checks guard that port.
- `crates/gpu/src/path/tests.rs` therefore passes unchanged: the BRDF/PDF tests against the
  f64 `render::surface` and `clear_coat` oracles, the zero-coat random-number checks, and the
  coat sampling oracle.
- New tests: a constant graph equals its `pbr` look in path output, and the path back end's
  pattern values equal the raster back end's at the same points. Both back ends compile the
  same IR, which is how two implementations are avoided.

**What sharing saves and costs (agent-hours):**

- **Saves** an estimated 28 hours that a separate path-tracer material system would need:
  its own IR and code generator (about 14), a second port of the patterns (about 8), and a
  second BSDF and coat with their oracle tests (about 6). It also removes the duplication in
  the table above for good.
- **Costs** 20 hours: extracting the shared library with parity re-verified in both
  shaders (track S, 8) and the path back end (track PT, 12).
- **Net:** about 8 hours saved now, plus one place to fix every later BSDF change.

**Out of scope:** the CPU ray tracer and the CPU product host. For a graph look they show
what they show today for a look without `pbr`: its display `color`. A `pbr` look keeps its
`pbr` values. There is no CPU graph evaluator in product code.

### 5.8 Default lighting environment

**Today's evidence fails.** In the renderer branch with the look on, the Sample Plate in a
metal look shows a clipped white top and pitch-black sides, and the plastic shows a
washed-out top with no highlight. (Screenshots were taken while preparing the renderer
branch.)

**The cause is structural, not only a matter of values.** The studio is a cube map at
infinite distance. A flat face therefore reflects one direction at every point, so a smooth
metal face shows **one** colour:

- faces turned toward the bright cyclorama clip to white;
- faces turned toward the dark floor and walls crush to black.

Better softbox values alone cannot fix this. The default environment for the Realistic
view is redesigned, generically and procedurally, with no bundled image (decision O14):

- **A finite studio with position-dependent reflections.** The studio is a room sized to
  the scene's bounds (a box about 4× the model's bounding radius, centred on it). Reflections
  use **box projection**: the reflected ray is intersected with the room box before the cube
  lookup, a public real-time technique (mathematics only). A flat face then sweeps across
  the softboxes and shows gradients. This is one ray-box intersection per lookup, and it is
  WebGL2-safe.
- **Built on a starter lighting rig** (§4.5): the rig's rectangular area lights, placed in
  multiples of the subject extent, are the softboxes, and `CAL-LIGHT-001` gives their gains.
- **Graded softboxes:**
  - a large key above front-left, whose radiance falls off smoothly toward its edges
    (no hard rectangle);
  - a fill card front-right at about ⅓ of the key;
  - a strip rim light behind at about ½.

  The key and the rim are also direct lights, through the existing softbox α widening, so
  coats get soft highlights.
- **Horizon and floor:** a cyclorama gradient from a mid-grey floor through a brighter
  horizon band to a soft ceiling. **No direction is darker than about 10 % of the mean
  radiance**, so no face reflects black. The floor receives the existing ground shadow
  (`ground_shadow`) and AO contact darkening.
- **Exposure is calibrated per preset, not automatic,** so frames are stable while
  orbiting: a white diffuse face under the key maps to about 0.8 display value before the
  tone curve's shoulder. PBR Neutral stays the tone curve. The environment's peak radiance
  is bounded, so that only small highlight areas reach the shoulder, never whole faces.
- **Kept from the renderer branch:**
  - the CPU bake of the environment texture (data preparation, off the window thread,
    stepped on the web) in `ui-egui/src/gpu.rs`, reused as is;
  - split-sum prefiltering, SH irradiance and multiple-scattering compensation;
  - the white-furnace tests.

  The `Product`, `Soft` and `Contrast` presets are re-tuned under the new model. `Product`
  becomes the Realistic default.

Track L builds this, and the acceptance frames (§5.9) judge it.

### 5.9 Demo scene and golden frames (acceptance)

**Demo scene.** `examples/realistic-showcase.json` is an original example design, built by
commands as `examples/material-finishes.json` is. It is shipped with the package and opened
in the Realistic view. It contains:

| Part | Material | Shows |
|---|---|---|
| Rounded enclosure lid | brushed aluminium graph (anisotropic, `pattern.brushed`) | brushed metal, stretched highlights |
| Cylinder and sphere | polished steel (`pbr`, roughness 0.08) | polished metal through the default studio, and that `pbr` looks still work |
| Rounded box | black epoxy with a clear coat (worked example) | coated plastic, coat highlights over a dark base |
| Plate | carbon-fibre twill (`pattern.weave`) with an image label (an original PNG asset, ATTRIBUTION row) | patterned and textured parts, `texcoord.planar`, `image` |
| Dome | clear silicone graph (`transmission_weight` 1) | the transparent-ish fallback (see-through, tinted) |
| Sample Plate | one scalar look and one metal `pbr` look | compatibility; the case that fails today |

Two camera poses are part of the example: isometric front-left and a low three-quarter
back view.

**Golden frames.** The scene is rendered by the actual viewport on a native GPU at
1600 × 1000 with 2 × 2 supersampling, in both poses and both looks. The frames are stored
under `crates/gpu/tests/golden/realistic/`, compared within tolerance by a test, and
regenerated only by an explicit script. The CPU is used only for the comparison, in tests.

**Acceptance bar.** These are measured on the golden frames with per-part masks (target
values for the founder to confirm, O15):

| Check | Bar |
|---|---|
| Clipped pixels | ≤ 1 % of each part's pixels at display luminance ≥ 254/255 (small highlights allowed) |
| Crushed pixels | ≤ 1 % of each part's pixels at ≤ 5/255, cavities (AO term < 0.5) excluded |
| Faces read as faces | every planar metal face spans ≥ 12/255 of luminance across itself (a gradient, not a flat fill); every visible face's median lies in [20, 235]/255 |
| Materials distinguishable | brushed vs polished metal, coated vs uncoated plastic: mean colour difference ≥ 6/255, or a visibly different highlight footprint |
| Sample Plate regression | top and side medians of the metal plate both in [40, 220]/255 (today: 255 and about 10) |
| WebGL2 matches native | browser frames of the same scene: mean channel error ≤ 2/255, 99th percentile ≤ 8/255 |
| Standard look untouched | standard-look frames bit-identical to the build before the package |

**Founder review gate.** The golden PNGs are shown side by side with today's frames. They
are part of the completion evidence, and **nothing goes upstream before the founder approves
them**. The numeric bar keeps them from regressing afterwards.

## 6. Realistic view mode

For modelling, realistic rendering mostly distracts from the shapes. The **standard** view, which
is today's renderer, therefore stays the default. **Realistic** is an additional way to
view the same model, and it can be switched on and off at any time.

**Names.** The user-facing name is **Realistic**, following common CAD usage: the
"Realistic" visual style in Inventor and AutoCAD, SolidWorks' RealView and Blender's
"Material Preview". Command titles, the menu row, locale strings (`assets/locales/pending/`)
and the docs all say "Realistic". "Photoreal" and "product look" remain only as references
to the renderer branch's history (`upstream-photoreal-look`, `Frame::product_look`).

The default is called **Standard**. Solidcraft's visual-style names ("Shaded with edges",
"Shaded", "Wireframe", "Shaded with hidden edges") describe faces and edges, not lighting.
Their natural word, "Shaded", is already a visual style, so reusing it would be ambiguous
("Shaded" style in "Shaded" look).

### 6.1 The switch

- **Model.** A viewport has two independent settings:
  - its existing **visual style**: shaded with edges, shaded, wireframe or shaded with
    hidden edges (`view.visual_style`);
  - its new **look**: `standard` (default) or `realistic`.

  Edges follow the visual style in both looks. In Wireframe there are no faces, so the look
  has no visible effect; it stays set for when faces return.
- **Command.** `view.look`, with input `{look?: "standard" | "realistic", view?: "model" |
  "render_preview"}` and output `{look, view}`.
  - Without `look` it toggles; without `view` it acts on the active view.
  - Like `view.visual_style`, it is `noundo` and `doc_only`, has input and output schemas,
    and is the same command for the UI, the CLI and MCP.
  - The view's state query reports the look alongside the visual style.
- **Key.** **Shift+R** is taken: it is `edit.repeat` ("Repeat Last Command",
  `crates/engine/src/cmd/edit.rs`). The default is therefore **Shift+P** (P for picture).
  It is free in today's command table and not on
  `keymap.rs`'s reserved list, and it can be rebound in Keyboard Shortcuts like any command.
- **Where it lives.** It sits in the navigation bar's visual-style menu, as a "Realistic"
  check row below the four styles. When realistic is on, the slot's icon shows a small
  badge. It is also in the command palette. It adds no toolbar of its own.
- **Not saved in the design.** Solidcraft's convention for view settings is the visual
  style: a viewport UI preference in the app's preferences JSON (`SolveApp::prefs`,
  `visual_style`), never in the design or its undo history. The look follows that
  convention exactly:
  - it is stored as `view_look`, remembered between runs (decision O7), and `standard` by
    default; Restore Defaults returns it to standard;
  - opening a design never changes it, so a file from someone else never switches a user
    into the realistic view;
  - one viewport serves all design tabs, as visual style does. The Render workspace preview
    is a separate view with its own look. If Solidcraft gains split views, each view carries
    its own look as it would its own visual style.

### 6.2 Realistic keeps modelling clear

The realistic view changes how **faces** are lit: the studio IBL, material graphs, coat, ambient
occlusion, PBR Neutral tone mapping and dither of the product look. Everything used to model
is drawn as in the standard view, composited **after** tone mapping in display space with
the theme's colour tokens, so neither exposure nor the tone curve alters it. That covers:

- selection and hover highlights of faces, edges, vertices, bodies and components;
- edges and silhouettes per visual style, hidden edges dashed;
- sketches, constraints, dimensions, construction planes and axes, the grid, move
  arrows and gizmos, measurements, section caps and clipping;
- faded context while editing a component in place: the context keeps its standard faded
  drawing, and only the active component is realistic;
- accessibility modes.

A highlight is never a material change: selecting or hovering rebuilds no geometry and
compiles nothing. Picking does not depend on the look.

In the realistic view, a **scalar look** (no `pbr` and no graph) and imported paint are lit as an
implicit dielectric `standard_surface`: base `color`, roughness 0.5, IOR 1.5. A design with
no appearances therefore looks sensible in the realistic view. Without this, today's look draws
non-PBR faces black when there are no product lights. Transparent scalar looks keep today's
alpha blend.

The tests are pixel comparisons in both looks:

- the selection and hover highlight colours, edge colour and sketch overlay pixels are
  identical;
- the grid and dimensions are unchanged;
- a face selected in the realistic view shows the token colour exactly.

### 6.3 The standard view is unaffected

- The standard look is today's renderer, unchanged. A look shows its `color` and opacity
  as today, whether it has a `pbr` response, a graph or neither.
- The standard look compiles no graph pipeline, decodes no texture, bakes no studio and
  allocates no material page. All of that starts the first time a view switches to
  realistic: the studio bakes off the window thread, as now, and the standard picture shows
  until it is ready.
- Test: a design with graphs and the same design with the graphs removed (same colours)
  give bit-identical standard frames and equal GPU memory.
- Switching rebuilds no geometry. Both looks draw the same tessellation. The realistic view's
  vertex streams are built from it the first time, off the window thread, and kept. After
  that, a switch takes one frame. A first switch on a model of one million triangles should
  take at most 300 ms off the window thread. Both figures are targets.

### 6.4 The existing graphics preference

The fork's opt-in, **Preferences > Graphics > Studio lighting in the Render preview**
(`render_studio_look`), is **replaced** by the view mode. The Render workspace preview gets
its look from `view.look` with `view: "render_preview"`, still `standard` by default. The
preference is not part of the upstream package. A fork user's saved `render_studio_look:
true` becomes the render preview's `realistic` once, and the key is then dropped. The
embedding API `Frame::product_look` remains the mechanism underneath: the view mode sets
it, and programs that embed `solidcraft-gpu` keep using it directly.

## 7. Editing and commands

### 7.1 The editor

Solidcraft's Appearance editor gains a **Material** section for the selected look, shown
when the look has a graph.

| | Node canvas | Layered stack (recommended for v1) |
|---|---|---|
| What it shows | Every node and wire on a pannable canvas | The surface inputs as rows, grouped (Base, Specular, Coat, Transmission, Emission, Geometry). A row is a constant, or a driven value: pattern + projection + remap range. Masked layers (`surface.mix`) are a list. |
| Fit with Solidcraft UI | A new container kind, camera-like navigation, wire picking; conflicts with the one-container and one-camera-path rules unless designed carefully | Ordinary panel rows, theme tokens, the existing disclosure and form widgets |
| Covers | Every valid graph | "Stack-shaped" graphs: one surface, chains of projection → pattern → maths → input, and mix layers with masks. Every worked example in §3.8 is stack-shaped. |
| Agents | Not needed (they use commands) | Not needed |
| Estimate | ~30 agent-hours to a usable canvas | ~16 agent-hours |

**Recommendation: the layered stack first.** It maps directly to how CAD users think
("brushed, with a coat, and a printed mark"), reuses existing widgets, and covers the
examples. A graph that is not stack-shaped (made by a command or a library) opens
read-only, with a summary (nodes, cost, textures) and "Edit as JSON" through
`appearance.graph.set`. A node canvas can follow once the stack has users. If one is built,
a permissively licensed egui node-graph widget would be evaluated then, with its licence
named in the PR.

The editor shows a live swatch (a sphere and a flat coupon, rendered on the GPU with the product look
off the window thread) and the graph's cost against the budget. Every edit is a command
(below); a slider drag is one undo step on release.

### 7.2 Commands

All commands are atomic, have input and output schemas, and never open dialogs. A failed
command leaves the design unchanged.

| Command | Input | Output |
|---|---|---|
| `appearance.graph.nodes` (read-only) | – | The node catalogue: kinds, ports, types, ranges, defaults, params, cost units, catalogue version |
| `appearance.graph.validate` (read-only) | `{graph}` | `{ok, cost, samples, textures, shape, errors: [{node, port, code, message}]}` |
| `appearance.graph.create` | `{id?, name, graph? \| from: "standard"}` | `{id}` |
| `appearance.graph.set` | `{id, graph}` (full replace) | `{id, cost}` |
| `appearance.graph.edit` | `{id, ops: [{op: "add_node" \| "remove_node" \| "connect" \| "disconnect" \| "set_input" \| "set_param" \| "set_interface", …}]}` | `{id, cost}` (all ops apply or none) |
| `appearance.graph.get` / `.list` (read-only) | `{id}` / `{}` | the graph with stats / `[{id, name, uses, source}]` |
| `appearance.graph.duplicate` | `{id, new_id?, name?}` | `{id}` |
| `appearance.graph.delete` | `{id, replace_with?}` | `{replaced}`; refuses while referenced unless `replace_with` is given |
| `appearance.texture.add` | `{path \| data_base64, name?}` | `{texture: sha256, width, height}` |
| `appearance.texture.list` (read-only) | – | `[{texture, name, width, height, bytes, uses}]` |
| `appearance.assign` (extended) | existing targets + `graph`, `values` (`pbr` remains shorthand) | unchanged |
| `appearance.graph.update_from_library` | `{id, library, preview: bool}` | an impact preview (changed inputs, affected looks), or the applied update |
| `library.export` / `library.apply` (extended) | catalog v3 | unchanged shapes, plus graph and texture counts |

Example (MCP or CLI, `solidcraft-cli run`):

```json
{"command":"appearance.graph.edit","params":{"id":"brushed-nickel-silver","ops":[
  {"op":"set_input","node":"surface","port":"coat_weight","value":1},
  {"op":"set_input","node":"surface","port":"coat_roughness","value":0.1}]}}
```

### 7.3 Runtime overrides

Render jobs (`render.start`) and the embedding API (`solidcraft-gpu`) can override a
graph's **interface** values for one job or frame, for example a texture bound to a
`texture` interface input. The design, undo history and saved file do not change, the same
contract as today's runtime `clear_coats`. Overrides are values and texture bindings, so
they never compile anything. This generic mechanism is what lets any program bind
per-instance images (labels, markings) at render time.

## 8. Pcbcraft side

Pcbcraft's cited table (`pcbcraft-core::materials`, 29 `MaterialId`s) stays in Pcbcraft.

1. A Pcbcraft build step turns each `MaterialDef` into a graph:
   - `base_linear`, `metallic`, `roughness` and `ior` map to `standard_surface`;
   - `coat` maps to the coat inputs;
   - `transmission` maps to the transmission inputs;
   - `PatternKind::EdmTexture` maps to a cellular noise roughness graph and `MetalGrain` to
     a brushed one;
   - the soldermask composites become coat-over-base graphs (§3.8).

   The result is written as a catalog v3 library file that ships with Pcbcraft. The entry
   ids are Pcbcraft's data inside Pcbcraft's file; Solidcraft code never sees a Pcbcraft
   name.
2. Part templates assign looks from that library in Solidcraft. The saved design carries
   the graphs, with generic `source` provenance (library id, entry, revision). This
   replaces the `pcbcraft:<material_id>` name convention and the planned `Look` coat and
   pattern fields (part-templates D8, SC4 and SC6).
3. Pcbcraft maps a body to its `MaterialId` through `source.entry` and checks the revision
   against its table. A changed table produces an "update from library" impact preview,
   never a silent change.
4. Material slots (render parameters) select between library graphs; markings bind
   interface textures at render time (§7.3).
5. `models::look` then projects to a graph instead of `OpaqueSurface` + `Detail`. Pcbcraft's
   3D view draws through the same graph renderer, so a part looks the same in Solidcraft
   and in Pcbcraft.

## 9. Upstream plan

The package is **one proposal**: "Realistic view and material graphs". There is no
renderer-only PR. The branch `upstream-photoreal-look` becomes the first commits of the
package, not a submission of its own, so the agent preparing it must not open it separately.

Every upstream step needs the founder's go: the proposal, the draft-PR claim and the PR.
The repository's rules apply: claim by draft PR (lowest number wins), the format change as
one pending file, real screenshots and a recording, and `cargo xtask ci` before every
commit. The steps are:

1. **Proposal first.** A short issue or discussion (this note, trimmed) asks maintainers to
   agree the model, the view mode and the format rule (§4.4) before any code is posted.
   Their render-materials backlog asks for "versioned appearance … resource contracts"
   ([handoff](render-materials/IMPLEMENTATION_HANDOFF.md), task 2), which this package
   provides.
2. **One draft PR, stacked commits.** Each commit builds and passes `cargo xtask ci` on its
   own, so review can go commit by commit. If maintainers prefer, the same commits become
   stacked PRs. They are still reviewed and merged as one package; none of them merges
   alone.

| # | Commit (group) | Contents |
|---|---|---|
| 1 | Realistic renderer | The content of `upstream-photoreal-look`: studio IBL (split-sum, SH irradiance), coat, AO, PBR Neutral, dither, softbox widening. `fs_pbr` is split into `material_eval` + `pbr_shade`. The fixed-field sites the graph replaces are removed (list below). |
| 1b | Default studio | The finite, box-projected studio, graded softboxes, horizon, calibrated exposure (§5.8) |
| 2 | Shared shading library | `crates/gpu/src/shading/` (BSDF, noise, projection), used by both `viewport.wgsl`/`pbr.rs` and `path.wgsl`. No output change: path tests unchanged, viewport frames within parity. |
| 3 | Realistic view mode | `view.look`, its key and navigation-bar row, overlays after tone mapping, scalar looks lit, lazy realistic resources, GPU requirement, the Render-preview look (§6) |
| 4 | Graph model | Types, validation, IR, cost model, base response, `SurfaceSample` around `OpaqueSurface` (`solidcraft-render`); the test-only reference |
| 5 | Format and compatibility | The pending file, `appearances.graphs`/`textures`, texture blobs, the oldest-sufficient-format rule (§4.4) |
| 6 | Commands | `appearance.graph.*`, `appearance.texture.*`, `assign` extension, runtime overrides; schemas |
| 7 | GPU graphs, viewport | The raster back end: code generation, shape cache, uniform pages, group-3 layout, pattern/projection/image nodes, GPU mips, anisotropy, `coat_color`, emission; the GLSL ES corpus test |
| 8 | GPU graphs, path tracer | The path back end: the material table, the `graph_surface` hook, the shape switch, approximation reports in preflight (§5.7) |
| 9 | Editor | The layered stack and GPU swatches in the Appearance editor; locale strings |
| 10 | Libraries and content | Catalog v3, built-in graphs (today's four patterns and the examples), user docs, examples, screenshots |
| 11 | Showcase and goldens | `examples/realistic-showcase.json`, its texture asset, the golden frames and the acceptance test (§5.9) |

**Commit 1 removes these fixed-field sites from the renderer branch** (as identified by the
renderer agent):

- `SurfacePattern` (`view/ibl.rs:149`);
- `ProductLook`'s `weave_pitch` and `ply_thickness` (`ibl.rs:82–86`) and the `look_detail`
  uniform slot (`viewport.wgsl:24`). `units_per_mm` moves to the frame uniform, because
  every graph's millimetre lengths need it; it stays a scene constant, not a material field;
- `GpuScene::pbr_tri_with` and its 64-byte detailed vertices, `vs_pbr_detailed`, and the
  `Source::Pbr { detailed }` occlusion pipelines (`view.rs:827`, `view/ao.rs`);
- `look_detail()` (`pbr.rs:357`);
- `render_studio_look` and `preview_look()` (`render_workspace.rs`), replaced by `view.look`.

The studio bake in `ui-egui/src/gpu.rs` is reused as is.

The PR body's `Changes a default:` line reads "No". The standard view stays the default,
files without graphs save unchanged, and the Render preview stays standard.

In the fork, the same series lands on `pcbcraft-via2` first. Pcbcraft keeps the fork-only
`SurfacePattern` path until track H moves it to graphs; the path is then removed from the
fork too.

### Branches and base

- **The package branch is `material-graph`, based on upstream main `21c5be2b2`.** The package
  targets upstream, so it is built and tested on upstream's code. Its first commits are the
  renderer branch (`77b22947a`, `ca4c4acdd` from `upstream-photoreal-look`). Then come this note
  and the C0 contract, then the tracks.
- **The fork's `pcbcraft-via2` stays on its older upstream (`4f3ac07ae`, #8747)** with the
  fork-only work (drilled boards, instancing, the via scaling). The package is not built on it:
  that base is about 2,300 upstream PRs behind, and the package would then carry fork-only
  changes into an upstream PR.
- **How the two stay in sync:**
  - The package lands in the fork by **merging `material-graph` into a fork integration branch**
    (`pcbcraft-via2` or its successor), which then also brings in upstream main. Fork-only work
    that conflicts (the via-scaling changes to `view.rs` and `pbr.rs`) is resolved there, once
    per package milestone, not in the package.
  - Pcbcraft pins a fork revision that contains the merge.
  - Fixes found in the fork that belong to the package are made on `material-graph` first, then
    merged again. The package never takes fork-only commits.
  - When upstream main moves, `material-graph` merges it (no rebase after the first track
    commit, so parallel tracks never lose their base).
- `material-graph-on-via2` is a local backup of the note's history on the old base; it is
  deleted once this layout is accepted.

## 10. Work breakdown

The contract comes first. **C0** fixes the following before any track starts:

- the Rust types of the node catalogue, the IR, `SurfaceSample` and the base response;
- the JSON schema;
- the command and `view.look` schemas;
- the signatures of the shared WGSL modules;
- the uniform-page layout and the path material-table layout;
- `graph_surface`'s signature;
- `Part::used`;
- the five worked examples as golden fixtures. Their expected values are analytic where
  possible, and otherwise come from the test-only reference.

All later tracks build against those signatures.

| Track | Scope | Depends on | Agent-hours |
|---|---|---|---|
| C0 | Contract (above) | – | 7 |
| R | Renderer integration: bring `upstream-photoreal-look` into the package branch, remove the fixed-field sites listed in §9 (patterns, weave/ply fields, detailed vertices and their AO pipelines, `look_detail`, `render_studio_look`/`preview_look`), move `units_per_mm` to the frame uniform, split `fs_pbr` into `material_eval` + `pbr_shade` with bit-identical frames | C0 | 7 |
| L | Default studio (§5.8): a finite, box-projected room sized to the scene, graded key/fill/rim softboxes, horizon cyclorama with a radiance floor, ground shadow and AO on the floor, calibrated exposure, re-tuned presets; furnace tests kept; per-part luminance metrics tooling for §5.9 | R | 12 |
| X | Showcase scene: `examples/realistic-showcase.json` built by commands (brushed, polished, coated, weave + image label, transmissive dome, Sample Plate), an original PNG asset with ATTRIBUTION row, camera poses; authored against C0's command schemas | C0, then D | 4 |
| S | Shared shading library: extract BSDF, coat, noise and projection into `crates/gpu/src/shading/` from `path.wgsl`'s versions; port `pbr.rs` and `path.wgsl` onto it; path tests unchanged, viewport parity, GLSL ES translation | R | 8 |
| V | View mode: `view.look` and schema, Shift+P, navigation-bar row, prefs (and the fork prefs migration), Render-preview look, overlays after tone mapping, scalar looks lit in the realistic view, lazy realistic resources, shared tessellation, the no-GPU refusal; pixel tests in both looks | R | 12 |
| A | Validation, IR, cost model, `SurfaceSample`, base response; hostile-input tests (`hostile_params_never_panic` style, random graphs) | C0 | 10 |
| T | Test-only reference: an f64 interpreter of the IR under `#[cfg(test)]`/dev-dependencies, never in product code; it checks both back ends' frames | A | 6 |
| B | Format: tables, `Look.graph/values` (with `pbr` unchanged), connection fallbacks, texture blobs and base64, save GC, catalog v3 container | C0 | 10 |
| K | Compatibility: `Part::used` and the oldest-sufficient writer in `format.rs`, corpus byte-identity tests, the v2/v3 catalog choice | B | 5 |
| C1 | Raster back end: code generator over the shared modules, shape key, pipeline cache with budget and base-response fallback, uniform pages, new vertex layouts, group-3 layout | A (IR), S at integration | 14 |
| C2 | Shared BSDF additions for the viewport: anisotropic GGX direct + studio, `coat_color` (oracle in tests), emission, transmission fallback; furnace tests | S | 9 |
| C3 | Textures on the GPU: decode worker, mips by render passes (the sRGB render-target check on WebGL2), bind groups, `image` node sampling; `MapRole::Height` (16-bit) in `texture_input`, the one decoder; the file's safety limits in `texture_input` (16384 px, 256 MiB, header-derived codec guard, 1 GiB per design), resident-size clamp and decode-time downsampling, the texture memory budget (§4.3). The admission part starts after B; only binding and sampling wait for C1 | C1, B (assets) | 11 |
| PT | Path back end: storage material table, triangle record in `interior.z`, per-job generated `graph_surface` with a shape switch, `image` via `texture_words`, approximation reports in preflight and job results; tests: no-graph parity, constant graph = `pbr`, both back ends agree at fixed points; reuses `path/textures.rs` packing, de-duplication and `texture_sample` (§4.5) | A (IR), S | 11 |
| D | Commands and schemas, undo/atomicity, CLI/MCP docs, runtime overrides in `render.start`; the `graph` variant of `render.library.appearance.apply` for `SURF-*` and Selene entries (§4.5) | A, B | 15 |
| E | Layered stack editor, GPU swatches, texture picker, locale strings; built against C0's command schemas, then joined to D (4 of its 16) | C0, then D | 16 |
| G | Built-in graph library (today's four patterns + examples), user docs, `examples/material-graphs.json`; the `SURF-*`/Selene binding table (anisotropy estimates recorded as such) | A, D | 7 |
| I | Evidence: `cargo xtask ci`, wasm build, an actual WebGL2 browser frame of every corpus shape in both looks, path-tracer frames of graph looks, the last release opening down-levelled files, perf measurements against §5.4 and §6.3; golden frames of the showcase, the acceptance test (§5.9) and the founder review set | C1–C3, PT, V, K, L, X | 14 |
| P | Package assembly: the commit series, the proposal text, PR body, screenshots and a recording | all | 4 |
| H | Pcbcraft (outside the package): table → catalog v3 generator, `models::look` → graphs, provenance mapping, parity renders against today's generated models | B, D, C1 | 10 |

- **Total:** about **182 agent-hours** for the upstream package, plus **10** for Pcbcraft:
  **192** in all. Full-quality textures (§4.3) added 2 to C3. Building on upstream's packs (§4.5) added 5: D +3, C3 +2, G +1, PT −1. The
  studio track L stays at 12, because reusing the rigs offsets binding them.
- **Critical path:** C0 (7) → A's IR (6 of its 10) → C1 (14) → integrating C2, C3 and S
  (6) → I (14) → P (4) = about **51 agent-hours**. The founder's review of the golden
  frames is a gate after I, not agent time.

The other chains fit inside it:

- R → L finishes by hour 26, well before I.
- X joins D at hour 29 and is done by hour 31.

- R → S finishes by hour 21. PT starts there, ends by hour 33, and joins I.
- R → V finishes by hour 25.
- D is done by hour 29, and E's joining to it by hour 33.

Changes since the previous estimate (140 + 10); the Save As export (3 hours of K) is removed, and the lighting and acceptance work adds L (12), X (4), golden frames (+3 in I) and the fuller cleanup (+1 in R):

- **Removed:** CPU consumers (old track F, 6) and the CPU evaluator and flattening in
  product code (4 from A).
- **Added:**
  - the shared library S (8) and the path back end PT (12);
  - the test-only reference T (6);
  - GPU mips (+1 in C3), the larger contract (+1) and path evidence (+1 in I).
- C2 is one hour smaller, because the coat already lives in the shared module.

Tracks R, A, B, E and X start in parallel after C0. S, C2, V and L start after R; PT starts after
S; D, K, T and H fan out once A and B land. Reviews and merge rounds come on top of both
figures.

### The C0 contract and who owns which files

**C0 is in place.** Its parts:

| Part | Where |
|---|---|
| Graph types, catalogue (v1), limits, IR types, `SurfaceSample`; `validate`/`lower`/`base_response` refuse "not built yet" | `crates/render/src/material_graph/` |
| Worked examples as fixtures | `crates/render/tests/fixtures/material_graph/` |
| `GraphId`, `TextureAsset`, `GraphTables` (not wired) | `crates/doc/src/appearance_graph.rs` |
| Format change, no step | `crates/doc/src/format/pending/appearance-graphs.rs` |
| Twelve `appearance.graph.*` / `appearance.texture.*` commands with schemas: `nodes` runs, the rest check their parameters and refuse | `crates/engine/src/{cmd,schema}/appearance_graph.rs` |
| `view.look` with schema, refusing `realistic`; the `view_look` UI setting | `crates/engine/src/cmd/view_navigation.rs`, `crates/ui-egui/src/lib.rs` |
| Shared WGSL module layout and its rules test | `crates/gpu/src/shading/` |
| Golden-frame location | `crates/gpu/tests/golden/realistic/` |
| Command labels | `assets/locales/pending/material-graph.json` |

**Ownership rules:**

- Each track **owns** the files below. Touching another track's file needs a note to its owner.
- Contract types change only by a commit that says so, after the affected owners agree.
- Every track lifts its own "not built yet" refusals and keeps `cargo xtask ci` green.

| Wave | Track | Owns | Relies on | h |
|---|---|---|---|---|
| 1 | R | `gpu/src/pbr.rs` (the `fs_pbr` split; `look_detail`), `gpu/src/view.rs` (`pbr_tri_with`, detailed vertices, `units_per_mm` to the frame uniform), `gpu/src/view/ao.rs`, `gpu/src/view/ibl.rs` (`SurfacePattern`, `ProductLook` fields), `assets/shaders/viewport.wgsl`, `ui-egui/src/render_workspace.rs` + `prefs.rs` (`render_studio_look`), `assets/locales/pending/render-studio-look.json`, `docs/render-opaque-pbr.md` | `SurfaceSample` as WGSL `Surface` | 7 |
| 1 | A | `render/src/material_graph/{ir.rs, catalogue.rs}` and new `validate.rs`, `lower.rs`, `base.rs` | `graph.rs`, `limits`, IR types, fixtures | 10 |
| 1 | B | `doc/src/appearance.rs` (`Look.graph/values`, `Appearances` tables), `doc/src/appearance_graph.rs`, the pending file's note, `io/src/design.rs` + `io/src/design/` (texture blobs), `io/src/catalog.rs` (v3) | `GraphTables`, `limits`, A's `validate` (refusing until A) | 10 |
| 1 | E | `ui-egui/src/dialogs_appearance.rs`, new `ui-egui/src/appearance_graph_editor.rs`, its locale file | the C0 command schemas; joins D | 16 |
| 1 | X | `examples/realistic-showcase.json`, its PNG under `assets/`, `attribution/pending/realistic-showcase.md` | the C0 command schemas; runs once D lands | 4 |
| 2 | S | `gpu/src/shading/{bsdf,noise}.wgsl`, `gpu/src/path/{path.wgsl, aging.wgsl, textures.wgsl, mod.rs}` (shader assembly), the BRDF functions of `pbr.rs` | module rules (`shading/tests.rs`), R's split | 8 |
| 2 | V | `engine/src/cmd/view_navigation.rs` + `schema/view_navigation.rs` (`view.look`), `ui-egui/src/{lib.rs, control.rs, navigation_bar*.rs, viewport.rs, keymap.rs, command_help.rs}`, overlay order in `gpu/src/view.rs` (coordinate with C1) | `view.look` schema, R | 12 |
| 2 | L | `gpu/src/view/ibl.rs` + `ibl/` (studio, box projection, exposure), the studio lookups of `pbr.rs`, `ui-egui/src/gpu.rs` (bake reuse), binding a starter rig from `assets/render/starter/lighting/` | R; merges after S in `pbr.rs` | 12 |
| 2 | T | new `render/src/material_graph/reference.rs` (`#[cfg(test)]` only) | A's IR | 6 |
| 2 | K | `doc/src/format.rs` (`Part::used`, `written_for`), `doc/build.rs`, the writer call in `io/src/design.rs` | B | 5 |
| 2 | D | `engine/src/{cmd,schema}/appearance_graph.rs`, `engine/src/cmd/inspect.rs` (`appearance.assign`), `engine/src/cmd/libraries.rs`, the `render.start` overrides, `engine/src/cmd/render/library.rs` (the `graph` variant) | A, B | 15 |
| 3 | C1 | new `gpu/src/material/` (raster code generator, shape cache, uniform pages), `gpu/src/shading/{patterns,projection}.wgsl`, PBR draw batching and vertex layouts in `view.rs`, the group-3 layout function in `ibl.rs` | A's IR, S, R | 14 |
| 3 | C2 | `gpu/src/shading/bsdf.wgsl` (anisotropic GGX, coat colour, emission), `pbr_shade` in `pbr.rs`, `render/src/clear_coat.rs` (test oracle) | S | 9 |
| 3 | PT | new `gpu/src/path/material.rs` (path back end), `gpu/src/path/scene.rs` (material table, `interior.z`), the `graph_surface` hook in `path.wgsl`, reusing `gpu/src/path/textures.rs` | A's IR, S | 11 |
| 3 | G | built-in graphs under `assets/appearances/graphs/`, `docs/render-material-graphs.md`, `examples/material-graphs.json`, the starter/Selene binding table | A, D | 7 |
| 4 | C3 | new `gpu/src/material/textures.rs` (upload, GPU mips), the decode worker hook, `render/src/texture_input.rs` (`MapRole::Height`), `texture_lod.rs`, `texture_sampling.rs`, `texture_input/normalization/` (raised limits) | C1, B | 11 |
| 4 | I | golden frames, the acceptance test, the browser run, perf records | everything | 14 |
| 5 | P | the commit series, the proposal and PR text | everything | 4 |
| – | H | Pcbcraft repo only | B, D, C1 | 10 |

Waves are start order, not barriers: a wave-2 track starts when what it relies on has landed.

### Risks

1. **WebGL2 is unproven.** No browser has drawn a look frame yet. Uniform-array indexing
   by a varying, sRGB mip render targets, compile and link times (ANGLE) and the
   four-group limit are the likely failure points. *Mitigation:* the GLSL corpus test from
   commit 7 on, a browser frame in track I before the package is offered, and the
   non-blocking compile with the base-response fallback.
2. **The package is large** for upstream review, twelve commit groups. *Mitigation:* the
   proposal first, a stacked series in which each commit passes CI on its own, and the
   offer of stacked PRs.
3. **Upstream disagreement on the model** (naming, table vs inline, scope), which would
   force a rewrite after the fork has shipped files. *Mitigation:* the proposal before code,
   and keeping the fork's format pending (unreleased) until upstream agrees.
4. **The format rule (§4.4) may be declined.** It touches shared `format.rs` policy.
   *Mitigation:* it is opt-in per change and conservative by default, and the package works
   without it (O8).
5. **Refactoring `path.wgsl`** onto shared modules could shift path output in the last
   bits, or change its random-number use. *Mitigation:* the shared functions are extracted
   from `path.wgsl`'s exact expressions, the path tests must pass unchanged, and graph
   evaluation consumes no random numbers.
6. **Overlay compositing in the realistic view:** drawing highlights, edges and sketches after tone
   mapping needs a clean pass order with the AO, supersample and cached-still paths.
   *Mitigation:* pixel tests for each overlay in both looks (track V).
7. **Path and viewport disagree** for inputs the path tracer approximates (anisotropy, coat
   colour). *Mitigation:* the approximation reports (O10); no new path lobes in this package.
8. **Parameter-blend `surface.mix`** shows artefacts at soft metal/dielectric edges.
   *Mitigation:* keep masks sharp in the examples; the closure mix comes later.
9. **The node set freezes on release.** Every addition is then a format change.
   *Mitigation:* start small, version the catalogue, refuse anything unknown.
10. **The anisotropic studio approximation** may fail the furnace bound at high anisotropy.
    *Mitigation:* clamp anisotropy in the studio term to what passes, documented.
11. **Pipeline count** in a design with many differently shaped graphs. *Mitigation:* the
    LRU and the base-response fallback; the stack editor produces few distinct shapes.
12. **The default studio may still not flatter every material.** Lighting quality is partly
    taste. *Mitigation:* box projection fixes the structural cause (uniform flat faces); the
    numeric bar (§5.9) catches regressions; the founder's review gate decides before
    anything goes upstream.
13. **Box projection misleads at the room's scale.** Parts far from the centre, or very
    flat scenes, can show stretched reflections. *Mitigation:* size the room from the scene
    bounds; the showcase includes a wide part to check it.

### Decisions

The founder accepted O1–O13 as recommended on 3 November 2026. They are now binding for
the tracks.

| # | Decision | Decided |
|---|---|---|
| O1 | Graph identity: a design-level table of named graphs referenced by looks, or an inline graph copied into every look (value semantics, as `pbr` is today) | **Table.** Editing a material updates every use (as CAD users expect), files stay small with 100,000 overrides, and libraries map to it with provenance. `duplicate` gives divergence when wanted. |
| O2 | First editor: layered stack or node canvas | **Layered stack** (§7.1); a canvas later. |
| O3 | `surface.mix` semantics in v1 | **Parameter blend**; closure mix later. |
| O4 | Default projection space | **Object**, so patterns stick to moving and instanced parts; today's look uses world millimetres, which `space: world` keeps. |
| O5 | Transmission inputs in v1 | **Include** them (validated; the viewport draws today's see-through glass, the path tracer its existing transmission), so glass and clear silicone need no second format change. |
| O6 | Constant colour encoding | Store linear floats; accept `{"srgb8": …}` for exact `pbr` equivalence; commands also accept hex sRGB. |
| O7 | Is the view look remembered between runs? | **Yes, like visual style** (the existing convention), defaulting to standard and reset by Restore Defaults. Opening a design never changes it. |
| O8 | Written format number for material-free designs | **Opt-in "oldest sufficient format"** (`Part::used`, §4.4): generic, conservative, no change for existing parts. If upstream declines it, the fallback is today's build-wide bump for the release cycle. |
| O9 | `pbr` → graph migration | **None, on open or on save.** A `pbr` look stays `pbr`. A graph is created only by an explicit edit that needs one (a coat, a pattern, "Convert to material graph"), so only designs that use graphs get the new format. |
| O10 | Path tracer and inputs it has no lobe for (anisotropy, coat colour) | **Approximate** (isotropic, untinted) and **report** each one in `render.preflight` and the job result. No new path features. The alternative, refusing such jobs, would block every brushed-metal render. |
| O11 | Composing the shared WGSL modules | **Concatenation** of `sc_`-prefixed modules, the mechanism both shaders already use. `naga_oil` (MIT OR Apache-2.0) only if module count grows. |
| O12 | Test oracle | **Keep a test-only f64 IR interpreter** (track T) beside analytic checks and golden GPU frames. Patterns have no closed form, and an independent reference catches back-end drift. It never ships in product code. |
| O13 | The view-mode key | **Shift+P**, which is free and rebindable; Shift+R is `edit.repeat`. Founder or maintainers may prefer another; nothing else depends on it. |

### Open decisions (with recommendations)

| # | Decision | Recommendation |
|---|---|---|
| O14 | Default studio: a starter rig, a new procedural studio, or a bundled HDR image | **A starter lighting rig** (`STU-*`, upstream's own, MIT OR Apache-2.0) with a cyclorama gradient added, bound in real time (§4.5, §5.8). No new studio type and no third-party image; a design's own `HdrResource` can light Realistic as an option. |
| O15 | Acceptance thresholds (§5.9) | **Adopt the proposed numbers as targets**, and let the founder's review of the golden frames confirm or tighten them before upstream. |
| O16 | Reflections of the default studio | **Box-projected, from a room sized to the scene.** This is the only cheap way flat faces get gradients; it is WebGL2-safe, at one ray-box test per lookup. |
| O17 | `render.library.appearance.apply` for `SURF-*` | **Keep `uniform_opaque` unchanged and add a `graph` variant** (§4.5); `height_displacement` stays in its omitted list, because graphs never displace geometry. |
