# Appearance graphs (design)

Status: a series in steps. The graph model (its types, the node catalogue, validation, the cost
model, the base response and the lowered form that later code generators read) and the design
format (§4.2) and the commands (§6) are in. Nothing draws a graph yet. Section 7 lists the later
steps.

## 1. Why and what

Today a look is a colour, an opacity and three optional PBR numbers (`Look`,
[`appearance.rs`](../crates/doc/src/appearance.rs),
[`pbr_appearance.rs`](../crates/doc/src/pbr_appearance.rs)). Clear coat, image maps, aging and
Glass live in a render setup, and only CPU and GPU images use them. Each new finish has so far
meant a new fixed field, with its own format change, command flags and editor rows.

An appearance graph is one optional, data-only description of a finish:

- An appearance graph is a small **node graph**: pattern, projection and maths nodes feed one **standard
  surface** node. The node vocabulary follows MaterialX's standard library, and the surface
  node's inputs follow OpenPBR Surface (itself derived from Autodesk Standard Surface).
- Clear coat, anisotropy, patterns and image maps are **inputs and nodes**, not format fields.
- A graph is validated when it is read: bounded size, typed ports, no cycles, value ranges and
  a cost bound. SolidCraft generates the GPU code itself; a file never carries shader code.
- One lowered form feeds two GPU back ends, the viewport and the GPU path tracer, so a look is
  defined once wherever a GPU shades it. The CPU ray tracer keeps using a look's base values.
- It is **optional**. A design without graphs stays as it is, and existing `pbr` looks are not
  rewritten.

What it is not: a general shading language, a MaterialX runtime, or a node editor.

### Clean room

The sources are public specifications, read as documentation:

- the MaterialX specification (Academy Software Foundation, Apache-2.0);
- the OpenPBR Surface specification (ASWF, Apache-2.0);
- the Autodesk Standard Surface specification (published under Apache-2.0).

Input **names** and the layering concept come from them, so that a later `.mtlx` exchange is a
mapping, not a translation. No MaterialX, OpenPBR or shader-library code is read, copied or
linked. The pattern mathematics is original or textbook material, cited as mathematics only:
value noise; gradient noise (Perlin 2002, "Improving Noise"); cellular noise (Worley 1996); fBm.

## 2. Concepts

| Term | Meaning |
|---|---|
| **Graph** | A named appearance definition: nodes, typed connections, one output surface node, and an optional *interface*. |
| **Node** | One operation from the fixed v1 catalogue (§3). It has typed **inputs** (a constant, a connection, or an interface reference) and compile-time **params** (enums and small integers that change the generated code). |
| **Interface** | Up to 32 named, typed inputs a graph exposes (MaterialX's nodegraph interface). A look can set them, so one "Anodised aluminium" graph serves every colour. |
| **Shape** | Everything about a graph that changes generated code: node kinds, params, topology, which inputs are connected and which are constants, texture colour spaces. Values never belong to the shape. |
| **Base response** | A static read of the graph's output node: constant inputs as they are, driven inputs at their declared `fallback`. It is an `OpaqueSurface` (§5.3), computed without evaluating anything. It gives a look's plain colour and what a renderer shows while a shape compiles. |

Lengths are millimetres and angles are radians. Colours in graphs are linear Rec.709 RGB; a
constant may also be written `{"srgb8": [r, g, b]}`, which is decoded exactly as
`srgb8_to_linear` decodes a look's colour.

## 3. Node set v1

**Types:** `float`, `color3` (linear), `vector2`, `vector3`, `texcoord` (a projected
coordinate: uv in mm, the tangent and bitangent it defines, and the pixel footprint in mm),
`normal` (a unit shading normal in world space), `texture` (a texture asset reference;
connectable only to an `image` node or an interface input), and `surface` (only the surface
nodes produce it). The only implicit conversion is the broadcast `float` →
`color3`/`vector2`/`vector3`. All other conversions use `convert.*` nodes.

**Slopes:** a `float` wire that reaches a normal input carries its value and its slope along
the texcoord's u and v (per mm). Pattern nodes compute slopes analytically or by a fixed finite
difference in pattern space. Maths nodes propagate them by the chain rule (forward-mode
differentiation). A slope is computed only for wires that need one, so plain colour patterns
pay nothing.

**Anti-aliasing is built in:** every pattern node fades to its mean as its period approaches the
pixel footprint. An author cannot turn this off, so a graph cannot alias into moiré.

**Ranges:** a constant outside its input's range **refuses** on load, as `PbrParameters` does
today. Wires computed at run time can leave a range. The surface node clamps its inputs and maps
NaN to the input's default, identically in every back end and in the test-only reference.

**Fallbacks:** a connection may carry a constant `fallback` of its input's type, for example
`{"from": "rough", "fallback": 0.45}`; it defaults to the input's default. Fallbacks make up the
base response (§2): data, not shading.

### 3.1 Surface nodes

`standard_surface` has OpenPBR input names (a subset); its output is `surface`.

| Input | Type | Range | Default | Notes |
|---|---|---|---|---|
| `base_weight` | float | [0, 1] | 1 | |
| `base_color` | color3 | [0, 1] | 0.8 | Linear reflectance. |
| `base_metalness` | float | [0, 1] | 0 | |
| `specular_weight` | float | [0, 1] | 1 | |
| `specular_color` | color3 | [0, 1] | 1 | Edge tint of metals, scale of dielectric F0. |
| `specular_roughness` | float | [0.045, 1] | 0.3 | Perceptual; `OpaqueSurface`'s lower bound. |
| `specular_ior` | float | [1, 3] | 1.5 | As a look's `dielectric_ior`; 1 means no interface. |
| `specular_roughness_anisotropy` | float | [0, 1] | 0 | 0 is isotropic. |
| `specular_rotation` | float | [−π, π] | 0 | Turns the tangent about the normal. |
| `geometry_tangent` | vector3 | unit | the texcoord's tangent | Usually wired from a projection. |
| `coat_weight` | float | [0, 1] | 0 | The same weight as `ClearCoat`'s. |
| `coat_color` | color3 | [0, 1] | 1 | Tint of light through the coat. |
| `coat_roughness` | float | [0.045, 1] | 0.1 | |
| `coat_ior` | float | [1, 3] | 1.5 | |
| `geometry_coat_normal` | normal | unit | the geometric normal | For orange peel on the coat only. |
| `geometry_normal` | normal | unit | the geometric normal | Base relief. |
| `transmission_weight` | float | [0, 1] | 0 | Requires `base_metalness` = 0. |
| `transmission_color` | color3 | [0, 1] | 1 | Transmittance at `transmission_depth_mm`. |
| `transmission_depth_mm` | float | [0.001, 1000] | 1 | |
| `emission_luminance` | float | [0, 1000] | 0 | Relative; not calibrated. |
| `emission_color` | color3 | [0, 1] | 1 | |
| `geometry_opacity` | float | [0, 1] | 1 | Cut-out coverage, not glass. |

Not in v1, and **refused** rather than dropped: subsurface, sheen/fuzz, thin film, thin-walled,
displacement and specular IOR spectra. Each would be a later node-set version.

`surface.mix` takes `fg` and `bg` (surface) and `mix` (float [0, 1]). In v1 it blends the two
surfaces' **parameters** (colours and scalars linearly, normals by normalised lerp), so one BRDF
is evaluated per pixel. This is enough for blends with sharp or moderately soft edges: an etched
mark, printed ink, a stripe. A true weighted sum of two BRDF evaluations (MaterialX's `mix` of
BSDFs) is a later option.

### 3.2 Geometry and projection nodes

Bodies are B-rep tessellations without UVs, so every texcoord is **projected**. Each projection
has a `space` param, `object` (default: the pattern stays on the body as it moves or is
instanced) or `world`.

| Node | Inputs (type, range) | Params | Output |
|---|---|---|---|
| `geometry.position` | – | `space` | vector3, mm |
| `geometry.normal` | – | `space` | vector3, unit |
| `geometry.facing` | – | – | float, \|n·v\| in [0, 1] |
| `texcoord.planar` | `origin` vector3 (±1e7 mm), `u`, `v` vector3 (each 0.001–1e6 mm long, orthogonal) | `space` | texcoord. u and v span one repeat, as in the runtime planar maps ([runtime textures](render-runtime-textures.md)). |
| `texcoord.triplanar` | `origin`, `scale_mm` vector3 (0.001–1e6), `sharpness` float [1, 64] | `space` | texcoord. Its consumers up to the first non-pattern node are evaluated three times and blended. That cost counts three times. |
| `texcoord.cylindrical` | `origin`, `axis` (unit), `reference` (unit, ⟂ axis) vector3; `radius_mm` float (0.001–1e6) | `space`, `unwrap`: `arc_length` \| `angle` | texcoord: u around (mm of arc at `radius_mm`, or radians), v along the axis (mm). |
| `texcoord.transform` | `texcoord`, `offset` vector2 (±1e6 mm), `scale` vector2 (1e-3–1e3), `rotation` float [−π, π] | – | texcoord |

### 3.3 Pattern nodes

Every pattern takes a `texcoord`, or a `vector3` position for 3D noise, and a `seed` (integer
0–2³¹−1). Each has outputs `value` (float, in [0, 1], mean 0.5 unless stated) and, where
meaningful, `slope`.

| Node | Inputs | Params | Notes |
|---|---|---|---|
| `pattern.noise` | `scale_mm` (0.001–1e4), `lacunarity` [1, 4] = 2, `gain` [0, 1] = 0.5 | `kind`: `value` \| `gradient` \| `cellular`; `dimensions`: 2 \| 3; `octaves`: 1–8 | fBm with an unrolled octave count. |
| `pattern.brushed` | `density_per_mm` (0.1–1000), `streak_mm` (0.01–1000), `contrast` [0, 1], `jitter` [0, 1] | – | Fine lines along u: noise stretched by `streak_mm / (1/density)`. Its tangent output is the texcoord's u, which feeds `geometry_tangent`. |
| `pattern.weave` | `pitch_mm` vector2 (0.01–100), `crown` [0, 1] = 1, `fibres` (0–32) = 7, `fibre_contrast` [0, 1] = 0.3 | `kind`: `plain` \| `twill_2x2` | Woven cloth, for example glass fibre or carbon-fibre twill. |
| `pattern.checker` | `size_mm` vector2 (0.001–1e6) | – | Values 0 or 1, smoothed by the footprint. |
| `pattern.grid` | `size_mm` vector2, `line_mm` (0.0001–1e6) | – | 1 on lines. |
| `pattern.gradient` | `start`, `end` float (±1e6 mm along u) | `kind`: `linear` \| `radial` | Ramp clamped to [0, 1]; `radial` measures the uv distance from the origin. |
| `image` | `file` texture; `default` color3 | `space`: `srgb` \| `linear`; `wrap`: `repeat` \| `clamp` \| `mirror`; `channel`: `rgb` \| `r` \| `g` \| `b` \| `a` | Outputs `color` color3, `value` float (the luminance or the chosen channel), `alpha`. |
| `normal.from_height` | `height` float (with slope), `relief_mm` [0, 1] | – | Perturbs the geometric normal along the texcoord's tangent frame. |
| `normal.map` | `color` color3 (linear, from `image`), `strength` [0, 1] | `convention`: `opengl` \| `directx` | Tangent-space normal map, in the texcoord's frame. |
| `normal.blend` | `a`, `b` normal, `mix` [0, 1] | – | Normalised lerp. |

### 3.4 Maths nodes

Inputs default to a value with no effect, so a partly wired node is still valid. Types are
float or vector (component-wise) unless stated.

`constant`, `add`, `subtract`, `multiply`, `divide` (the denominator is held at least 1e-6 away
from zero), `min`, `max`, `clamp`, `abs`, `floor`, `fract`, `power` (the base is clamped to
≥ 0), `sqrt` (of max(x, 0)), `sin`, `cos`, `mix`, `step`, `smoothstep`, `remap` (in-range →
out-range, clamped), `invert` (1 − x), `dot`, `length`, `normalize` (zero → +Z), `combine3`,
`separate3`, `luminance`, `hsv_adjust` (colour only; hue ±π, saturation and value scales 0–4),
`convert.float_to_color3` and the other explicit conversions.

There are no loops, no conditionals that branch control flow (`step` and `smoothstep` are
select-free arithmetic), and no recursion. Generated code is therefore straight-line, with
bounded cost, and derivatives stay valid everywhere (§5.1).

### 3.5 Limits

| Limit | Value | Why |
|---|---|---|
| Graphs per design | 4096 | A design library, not per-face copies. |
| Nodes per graph | 128 | Larger than every worked example ×4. |
| Connections per graph | 512 | Each input has at most one connection. |
| Interface inputs | 32 | |
| Ids | 1–64 bytes `[a-z0-9_.-]`, unique within a graph | Never used as identifiers in generated code. |
| Names | 1–256 UTF-8 bytes, no control characters | As body names. |
| Cost after expansion (§3.6) | 100,000 units (safety bound) | Not a quality limit: it stops pathological or hostile graphs that would stall the GPU until the driver resets it. Above 4,000 units a graph is only reported as heavier than typical. |
| Texture samples after expansion | 8 | Triplanar images count three each. |
| Distinct textures per graph | 4 | WebGL2 has 16 sampled textures per stage. |
| Texture assets | ≤ 2048 px a side, ≤ 16 MiB encoded (the render job's texture-map bounds, #813), ≤ 256 per design, ≤ 1 GiB decoded per design | Safety only; a renderer chooses the resident size. |
| Uniform constants per graph | 256 floats (64 vec4) | With the 7-vec4 base response in front, a record is ≤ 71 vec4, so a 16 KiB uniform page holds ≥ 14 looks (§5.2). |

### 3.6 Cost model

Each node kind has a fixed cost in "units", calibrated against GPU time on the viewport back end
(the measurements come with that step): one unit is the fragment time with which 2000 units add
half of the viewport's PBR shading on top of a constant graph, so a graph's fragment time is
about 1 + cost / 4000 times that shading. `constant` 0, arithmetic 1, `smoothstep` 4,
`hsv_adjust` 40, `pattern.brushed` 100, `pattern.weave` 120, `image` 8 plus one sample, and
`standard_surface` 0 (the lighting is a fixed cost). `pattern.noise` costs 100 plus, per octave,
75 (2D value), 150 (3D value), 200 (2D gradient), 500 (3D gradient), 400 (2D cellular) or 1500
(3D cellular). Slope-carrying wires cost ×3 (×4 for 3D noise, one more evaluation per axis) and
triplanar subgraphs ×3.

The cost is **advisory**: there is no quality limit. Validation computes the expanded cost;
above 4,000 units (`limits::COST_TYPICAL`, about twice the viewport's PBR shading) a graph is
reported as heavier than typical, and it is still accepted. Only a graph over the **safety
bound** of 100,000 units (`limits::COST_UNITS`) is refused. Files are untrusted: at 100,000 units
a full-screen layer takes about 25× the PBR shading on a fast desktop GPU, and on a GPU twenty
times slower, at 4K with some overdraw, about a second a frame, near the point where drivers
reset the GPU (Windows' 2 s timeout) and browsers lose the WebGL context.

### 3.7 Existing looks, expressed

- **Opaque PBR look (`pbr`):** it stays in the file as it is and is never rewritten. A renderer
  can read it, in memory, as one `standard_surface` with `base_color` = the look's sRGB colour,
  `base_metalness`, `specular_roughness` and `specular_ior`. The fixture `pbr-steel.json` is the
  library's "Steel - Satin PBR" look in this form, and its base response equals the look's
  `OpaqueSurface` exactly.
- **Clear coat:** `coat_weight`, `coat_roughness` and `coat_ior` are exactly `ClearCoat`'s
  weight, roughness and IOR. A pigmented body under lacquer keeps the existing rule: base
  `specular_ior` = 1 (no second interface).

### 3.8 Worked examples

Each example is a complete graph (§4); `"srgb8"` marks an exact sRGB constant. Values are visual
estimates, not measured optical data. The same graphs are the test fixtures in
`crates/viewport/tests/fixtures/appearance_graph/`.

**Brushed stainless steel** (brushed along X):

```json
{"name": "Brushed stainless steel",
 "nodes": [
  {"id": "uv", "kind": "texcoord.planar", "params": {"space": "object"},
   "inputs": {"origin": [0,0,0], "u": [1,0,0], "v": [0,1,0]}},
  {"id": "lines", "kind": "pattern.brushed",
   "inputs": {"texcoord": {"from": "uv"}, "density_per_mm": 40, "streak_mm": 4, "contrast": 0.6, "jitter": 0.3, "seed": 7}},
  {"id": "rough", "kind": "remap", "inputs": {"in": {"from": "lines"}, "out_min": 0.25, "out_max": 0.31}},
  {"id": "tint", "kind": "remap", "inputs": {"in": {"from": "lines"}, "out_min": 0.97, "out_max": 1.03}},
  {"id": "base", "kind": "multiply", "inputs": {"a": [0.8,0.77,0.7], "b": {"from": "tint"}}},
  {"id": "surface", "kind": "standard_surface",
   "inputs": {"base_color": {"from": "base"}, "base_metalness": 1, "specular_roughness": {"from": "rough"},
              "specular_roughness_anisotropy": 0.6, "geometry_tangent": {"from": "lines.tangent"}}}],
 "output": "surface"}
```

**Laser-etched stainless steel** (a logo; the mark is a PNG asset, planar-projected over a
12 × 8 mm area): a `texcoord.planar` feeds an `image` (linear, clamped, alpha channel) and a 2D
cellular noise; the noise's slope goes through `normal.from_height` into an etched, darker and
rougher surface, and a `surface.mix` puts the etched surface where the image's alpha is 1.

**Candy coat over polished aluminium** (a tinted gloss coat over a bright metal, with orange
peel): two 2D value noises at 2.4 mm and 0.9 mm, weighted 1 and 1/6, feed `normal.from_height`
(6 µm) into `geometry_coat_normal`. The surface is metal, base colour (0.91, 0.92, 0.92),
roughness 0.35, coat weight 1, coat colour (0.6, 0.06, 0.05), coat roughness 0.12, coat IOR 1.52.
The peel tilts only the coat's normal.

**Gloss black plastic, clear coated** (an EDM-textured moulded body with a lacquered top): a 3D
cellular noise of the object position (two octaves at 0.04 mm) is remapped to a roughness of
0.4–0.52, under a clear coat of roughness 0.08 and IOR 1.5. The base has `specular_ior` 1.

**Printed label** (a blue film around a cylinder, with a stripe and printed text): a
`texcoord.cylindrical` (radius 4 mm, unwrapped by angle) drives a `smoothstep` stripe and an
`image` of the text; their `max` mixes a printed surface over the film, whose colour is the
interface input `label`.

### 3.9 Later extensions (not v1)

- Further OpenPBR lobes (sheen, thin film, subsurface), closure-level `surface.mix`, and an
  edge-following projection.
- `.mtlx` import and export of the subset, mapping node kinds 1:1 where MaterialX has the same
  node (`mix`, `noise3d`/`fractal3d`, `cellnoise3d`, `image`, `triplanarprojection`,
  `checkerboard`, `ramplr`, `normalmap`, `heighttonormal`, `open_pbr_surface`).

## 4. Saved form

### 4.1 A graph

A graph is JSON (`appearance_graph::AppearanceGraph`):

```json
{"name": "Anodised aluminium",
 "interface": [{"name": "color", "type": "color3", "default": [0.1, 0.3, 0.6]}],
 "nodes": [
  {"id": "surface", "kind": "standard_surface",
   "inputs": {"base_color": {"interface": "color"}, "base_metalness": 1, "specular_roughness": 0.35}}],
 "output": "surface"}
```

- `nodes` hold `id`, `kind`, `params` and `inputs`. An input is a constant, a connection
  (`{"from": "node"}` or `{"from": "node.output"}`, with an optional `fallback`) or an interface
  reference (`{"interface": "name"}`).
- Optional: `interface`, `source` (library id, local entry id and revision) and
  `layout` (editor-only canvas positions, which nothing in this model reads).
- Unknown fields, kinds, params and inputs are refused rather than dropped.

A folder entry’s revision is a lowercase SHA-256 digest. Earlier numeric revisions keep their
saved form. Provenance describes the copy in the design and does not affect shading.

Reading a graph only parses it; validation checks it.

### 4.2 In a design

Graphs live in a design-level table under `appearances`, and looks refer to them by id. Texture
assets live beside them.

```json
"appearances": {
  "graphs": {
    "brushed-stainless": {"name": "Brushed stainless steel", "nodes": [ … ], "output": "surface"}
  },
  "textures": {
    "9b2e…sha256": {"name": "logo.png", "format": "png", "width": 512, "height": 256, "data": "…base64…"}
  },
  "bodies": {
    "Housing": {"name": "Brushed stainless steel", "color": [231, 227, 218], "opacity": 1,
                "graph": "brushed-stainless", "values": {}}
  }
}
```

- `Look` gains `graph` (an id) and `values` (interface values by name), both omitted when
  empty. `graph` and `pbr` exclude each other. `pbr` stays a valid, unchanged form; looks with
  neither keep scalar shading.
- `appearances.graphs` and `appearances.textures` are omitted when empty, so a design without
  graphs saves the same JSON as before.
- A look's `color` stays required. With a graph it is the colour a plain view, STEP/3MF colour
  export and older readers show; it is the graph's base colour (§5.3) when a look takes a
  graph.
- A graph that can be see-through (`geometry_opacity` < 1 or `transmission_weight` > 0) needs
  the look's `opacity` to be 1, as `pbr` looks do today.
- **Textures:** a texture is keyed by the SHA-256 of its exact encoded bytes (PNG or JPEG). In a
  container it is a `blobs/<sha256>.png|jpeg` part, as canvases are, so the container format does
  not change; in a plain JSON design `data` holds base64. Textures nothing references are dropped
  on save. Graphs are design data and stay until deleted, even when no look uses them.
- Reading checks everything: each texture's hash, header and size, each graph's bounds,
  validation and texture references, the design's decoded-texture budget, and every look's
  graph, values and opacity rule. Unknown fields are refused rather than dropped.
- The change is one pending format file, `appearance-graphs.rs`. It has no step: older designs
  are not rewritten. The last release warns about the newer format number, shows each look's
  `color`, and keeps the tables it cannot read when it saves.

### 4.3 In a library folder

Beside the library’s `*.sclibrary.json` manifest, put one entry descriptor in `graphs/` for
each finish. Use an id such as `studio/oak`, where `studio` is the manifest’s id. The file
name may change without changing the entry id. The descriptor holds four fields:

| Field | Contents |
|---|---|
| `id` | Qualified library and entry id |
| `provenance` | Licence and optional author and source URL, as for other folder entries |
| `definition` | `{"kind":"graph","value": …}`, with the graph from §4.1 as the value |
| `revision` | Lowercase SHA-256 of the compact serialized pair `[provenance, definition]`. Rust authors can use `solidcraft_io::library::GraphEntry::content_revision`. |

Put the original encoded bytes for each referenced image in `textures/<sha256>.png` or
`textures/<sha256>.jpeg`. The digest is over those file bytes. A missing image, a mismatched
digest or an invalid graph is reported alongside the entry when browsing. Import copies the
graph and its images into the design. Later folder edits do not change the imported finish.

## 5. What the GPU back ends read

### 5.1 Lowered form

Validation produces a typed, topologically ordered **IR** (`appearance_graph::Ir`): one value per
output, types resolved, triplanar and slope expansions applied, and constants assigned to
uniform slots. It is a pure Rust type with no GPU dependency, shared by both back ends (§5.4). A
back end turns it into one straight-line WGSL function that only calls a fixed library of
pattern and shading functions. Identifiers are generated, so no user string ever reaches WGSL,
and constants are uniforms, not literals; only shape params (octaves, enum kinds) change code.
Straight-line code keeps `fwidth` and texture derivatives in uniform control flow.

### 5.2 Shape key and uniform words

The **shape key** is a 128-bit hash of the canonical IR without values. Graphs that differ only
in values share one pipeline; a value edit changes uniforms and never recompiles.
`uniform_words` fills a look's uniform slots from its constants and a look's interface
values, in the order the generated code reads them; the base response's 7 vec4 come first, so a
fallback pipeline can read them at the same offset while a shape compiles.

### 5.3 Base response and fallbacks

`base_response` and `base_sample` read the output node statically: constants as they are,
driven inputs at their fallbacks, and for a `surface.mix` its `bg` surface. `look_base_sample`
does the same with a look's interface values in place of the defaults, so a look never shows the
graph's own default colours. The base response is what a renderer shows while a shape compiles
or when a back end cannot draw the graph, and its colour is a look's plain colour.

### 5.4 One appearance system, two GPU targets

The surface node's output type is `SurfaceSample`: `OpaqueSurface` (base colour, metallic,
roughness, dielectric IOR, optional `ClearCoat`) plus the inputs `OpaqueSurface` lacks
(specular weight and colour, anisotropy, coat colour, transmission, emission, opacity). A
constant `SurfaceSample` without those extensions converts losslessly to `OpaqueSurface`.

Both GPU back ends, the viewport and the GPU path tracer, will evaluate the same IR. The
test-only reference (`appearance_graph::reference`, `cfg(test)`) is an f64 interpreter of the IR
and the oracle both are checked against: product code never evaluates a graph on the CPU.

## 6. Commands

Every command has input and output schemas, never opens a dialog, and is the same for the UI,
the CLI and MCP. A command that changes the design is one undo step and atomic: it builds the
whole change, checks it, and leaves the design as it was when anything is refused.

| Command | Input | Output |
|---|---|---|
| `appearance.graph.nodes` (read-only) | – | The node catalogue: kinds, ports, types, ranges, defaults, params, costs and its version |
| `appearance.graph.validate` (read-only) | `{graph}` | `{ok, cost, samples, textures, shape, errors: [{node, port, code, message}], note?}` |
| `appearance.graph.create` | `{name, id?, graph? \| from?: "standard" \| a saved look}` | `{id, cost}` |
| `appearance.graph.convert` | `{look, id?, name?, scope?}`: the look's one-node graph (§3.7), and every override that is exactly that look names it | `{id, cost, count}` |
| `appearance.graph.set` | `{id, graph}` (a whole replacement) | `{id, cost}` |
| `appearance.graph.edit` | `{id, ops: [add_node \| remove_node \| connect \| disconnect \| set_input \| set_param \| set_interface]}`, all applied or none | `{id, cost}` |
| `appearance.graph.get` / `.list` (read-only) | `{id}` / `{}` | the graph with its cost and uses / `[{id, name, uses, source?}]` |
| `appearance.graph.library.list` (read-only) | `{folder?}` | The library id, its graph entries and any refused entries with reasons |
| `appearance.graph.library.import` | `{folder?, entry, revision?, id?}` | The design graph id and cost. The graph and its referenced images are copied in one undo step. |
| `appearance.graph.duplicate` | `{id, new_id?, name?}` | `{id, cost}` |
| `appearance.graph.delete` | `{id, replace_with?}`; refused while looks use the graph, unless `replace_with` is given | `{replaced}` |
| `appearance.texture.add` | `{path \| data_base64, name?}` (PNG or JPEG) | `{texture, width, height}` |
| `appearance.texture.list` (read-only) | – | `[{texture, name, width, height, bytes, uses}]` |

`appearance.assign`, `appearance.paint`, `appearance.replace` and `material.create`/`edit`
also take `graph` and `values`: a look that names a graph and sets its interface inputs. Its
`color` becomes the graph's base colour with those values (§5.3), and every command that changes
a graph recolours the looks that use it in the same step. A heavier-than-typical graph is
accepted with a `note` (§3.6). Render scenes show a graph look by its base response until the GPU
back ends draw it.

Example (MCP or `solidcraft-cli run`):

```json
{"command": "appearance.graph.create", "params": {"id": "candy-red", "name": "Candy red", "graph": { … }}}
{"command": "appearance.graph.edit", "params": {"id": "candy-red", "ops": [
  {"op": "set_input", "node": "surface", "port": "coat_roughness", "value": 0.08}]}}
{"command": "appearance.assign", "params": {"bodies": ["Housing"], "graph": "candy-red"}}
```

## 7. Later steps

Each step lands on its own and changes no default:

1. A shared WGSL shading library for the viewport and the path tracer, frames unchanged.
2. Graph looks drawn in the viewport, in steps (constant graphs; patterns and projections;
   images; coat, anisotropy and transmission).
3. Graph looks in the GPU path tracer.
4. A graph section in the Appearance editor.
5. Graphs as entries in appearance libraries.
