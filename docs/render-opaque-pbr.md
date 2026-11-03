# Opaque product surfaces

This is the first integrated consumer slice of #1004, under #813/#620. It uses the
existing Render workspace, product geometry, appearance assignment and PNG job host.
There is no second renderer or external material dataset.

## Apply and preserve a response

```json
{"command":"appearance.assign","params":{"bodies":["Specimen"],"color":"#b47846","pbr":{"metallic":1,"roughness":0.3,"dielectric_ior":1.5}}}
```

The Appearance editor has an **Opaque PBR** checkbox and separate Metallic,
Roughness and Dielectric IOR fields. Enabling it explicitly sets opacity to one.
View the response in Render or export a product PNG; Design retains its simple
color preview. Apply/OK, swatch reuse and component/face targeting use the existing
assignment command. A later scalar face override replaces a body's PBR response
on that face; it does not inherit the optical response implicitly.

Parameters are finite and bounded: metallic [0,1], perceptual roughness [0.045,1],
relative dielectric IOR [1,3]. For PBR, unknown optical fields and
nonnumeric/out-of-range opacity refuse before changing the design. PBR requires
opacity exactly one.
IOR controls the nonmetal fraction's reflection; it does not enable refraction.
A library appearance selected by name keeps its complete preset response when
`pbr` is omitted. An explicit `color`, including a custom look named like a
library entry, starts with scalar shading unless a `pbr` block is supplied.
Supplying `pbr: null` explicitly selects scalar shading. Assignment replaces a
look; it is not a partial parameter patch.

Use `name` with an explicit `color` to copy an inert look label, including labels
that happen to equal a `starter.*` ID. `name` and `appearance` are mutually
exclusive. The Appearance editor uses `name` for owned looks and `appearance`
only for an explicitly selected recipe, so reopening or reusing a copied look
does not change its optical values. Explicit unavailable glass recipe requests
still refuse; an inert label never enables refraction.

`material.create/edit` accept the same optional `pbr` block alongside a default
appearance/color. A named PBR default also requires opacity exactly one, even
when the command omits `pbr`; invalid combinations refuse before creating or
editing the material. A density-only edit retains the entire existing look. Visual
response changes do not modify density, geometry, placement or engineering axes.
No grade-specific optical or mechanical values are inferred from a name or color.

Since format **21**, `pbr` is saved inside existing body/face/component and
material-default looks. Missing fields retain scalar shading. Format-20 render setups and old looks
upgrade without reinterpretation; older readers refuse newly saved files instead
of dropping their optical response. Undo/redo, copying and cold reopening use the
existing design-owned values, with no external file dependency.

The portable catalog v2 preserves PBR looks and physical-material defaults, alongside
validated engineering cards. Explicit scalar v1 export refuses any loss of those
records before writing. V1 imports and v2 looks without PBR explicitly select scalar
shading and clear a prior PBR response. See [local material libraries](book/src/dev/local-libraries.md).
In-design swatches retain and reuse the response.

## Inspect, replace and inherit a look

`appearance.chain` is a read-only command. Supply exactly one target: a body,
a face index from `model.faces` with its body, or a component name/ID:

```json
{"command":"appearance.chain","params":{"target":{"body":"Specimen","face":0}}}
{"command":"appearance.chain","params":{"target":{"component":"Housing"}}}
```

The result contains four `steps`, ordered `face`, `body`, `component`, `material`.
Each step contains `step` and `look`: either null or the full saved look
(`name`, hex `color`, `opacity`, and optional `pbr`). `active` names the first
nonempty step, or is null. Body reads have no face step. Component reads have
no face/body steps. Physical materials belong to bodies; a component's material
step shows the common material look of its directly owned bodies, or null for
mixed/empty components. Root is not an assignable component. Imported paint
remains a display fallback outside these four steps, so an all-null chain does
not imply a particular viewport color.

`appearance.replace` rewrites every equal body, face and component override in
one undo step. `from` accepts a library name or a complete look copied from
`appearance.list`'s `in_design` entries. Equality includes the label, color,
opacity and all PBR values, so looks with identical names but different responses
remain distinct. `to` accepts a library name or an object with the same look
fields as `appearance.assign` (including inert `name` with `color`).

```json
{"command":"appearance.replace","params":{"from":"ABS - Red","to":"Copper - Clean PBR"}}
{"command":"appearance.replace","params":{"from":{"name":"Custom","color":"#abcdef","opacity":0.5},"to":{"color":"#123456","opacity":0.4},"scope":{"bodies":["Specimen"]}}}
```

Omit `scope` to replace all stored overrides, including hidden or suppressed
ones. Optional `scope.bodies` selects body overrides and face overrides on those
bodies; `scope.components` selects only those component overrides. These lists
combine as a union. A component scope does not expand to its bodies or descendants.
Names and numeric component IDs are accepted; duplicate targets count once.
Scope targets must exist in the evaluated model/design. An empty target list
selects nothing. At most 10,000 scope entries and 100,000 stored overrides are
processed; larger requests/designs refuse. The chain read uses the same override
budget and at most 100,000 bodies for a component read.

The replacement result is `{"count": N}`, counting stored overrides rewritten.
Invalid values or targets refuse before mutation. A missing source look, empty
scope or identical replacement returns count zero without adding undo history or
clearing redo. Geometry, placements, density, mass, physical material defaults
and engineering records are unchanged. No command opens a dialog. Existing look
records support saving/reopening these edits without a format change.

**From Material** uses the existing clear form of `appearance.assign`:

```json
{"command":"appearance.assign","params":{"bodies":["Specimen"],"color":null}}
{"command":"appearance.assign","params":{"components":["Housing"]}}
{"command":"appearance.assign","params":{"body":"Specimen","faces":[[5,10,5]],"color":null}}
```

Clear removes only the selected override. A face then follows its body, a body
its component, and finally the physical material's default look. Clear remaining
higher-priority overrides when the material should be active. This changes the
look without changing density or mass.

## Original opaque finish presets

The existing Appearance library includes six additional opaque finishes. Its
20 scalar entries keep their names, colors, opacity and shading behavior. The
new names are distinct, and the Appearance editor uses its existing categories,
search, swatches and optical fields to select and edit them.

| Appearance | sRGB color | Metallic | Roughness |
|---|---|---:|---:|
| Steel - Satin PBR | `#9ea4ac` | 1 | 0.40 |
| Aluminum - Satin PBR | `#ccd0d6` | 1 | 0.30 |
| Copper - Clean PBR | `#cc7a50` | 1 | 0.25 |
| ABS - Molded PBR | `#deded8` | 0 | 0.40 |
| PLA - Smooth PBR | `#6ea0de` | 0 | 0.35 |
| Wood - Oak Uniform PBR Preview | `#be925c` | 0 | 0.65 |

Each preset has opacity 1 and an illustrative dielectric IOR of 1.5. The values
are original visual studies, not measured optical constants or engineering-grade
material properties. Oak is a uniform preview without grain; PLA has no printed
layers; satin metals use an isotropic response. A physical material's density and
engineering axes remain independent of these visual choices.

The source values live in `assets/appearances/opaque_finishes.rs`. Assigning a
preset saves its values inside the existing look, so a later library update does
not change a saved assignment. This addition does not change the design format.

`examples/material-finishes.json` creates six original radius-10 mm spheres,
assigns generic physical materials and applies these finishes through the same
commands used by the editor:

```sh
solidcraft-cli run examples/material-finishes.json --save target/material-finishes.sc
solidcraft-cli render target/material-finishes.sc --out target/material-finishes.png --width 1600 --height 1000
```

## Actual consumers and color

The installed integrated `gpu-viewport` and `cpu-product` image hosts advertise the
`pbr` feature. Requests require it when the captured product scene contains PBR.
The static legacy `cpu-raster` descriptor retains its previous capabilities.
No host gains HDRI, refraction, progressive/final rendering or EXR support.

Look colors are decoded from sRGB8 to linear sRGB once in the shared document/scene
adapter. PBR GPU vertices (48 bytes) carry linear reflectance and optical parameters;
a coated body previews uncoated, as below. The CPU
rasterizer interpolates world position and normals with perspective correction;
the GPU uses the corresponding fragment interpolation. Both use the original
[opaque BRDF](render-surface.md) and three fixed neutral directional lights in Z-up
world coordinates. They sum BRDF × relative incident radiance × light cosine,
apply per-channel Reinhard preview tone mapping, then encode exactly once.
Linear and sRGB GPU attachments have distinct output transfer paths.

This fixed studio has no cast shadows, environment illumination, indirect bounces,
emission, anisotropy, coating, absorption or transmission. Lighting magnitudes are
relative preview values, not calibrated lux or spectral data. The single-scattering
model can darken rough metals. Transparent legacy looks retain approximate sorted
alpha blending, including when seen in front of an opaque PBR surface; that is not
glass optics. Nondefault exposure/environment controls remain explicitly unsupported;
the opt-in product look below lights only the real-time preview.

Scene uploads keep the 128 MiB bound, accounting for the larger PBR vertices.
CPU face-map admission accounts for retained optical descriptors. Existing immutable
capture, ownership, cancellation, stale-result rejection and PNG export policies
remain in effect. Invalid/nonfinite geometry or zero input normals refuse.

## Opt-in product look

The GPU viewport has a second, opt-in shading path, the product look. It is the
renderer underneath the **Realistic** view mode ([material graphs](material-graph.md) §6):
`view.look` (Shift+P, or **Realistic** in the navigation bar's visual-style menu) sets it
for the modelling viewport or, with `view: "render_preview"`, for the Render preview. Each
view's look is a preference remembered like the visual style, Standard by default and never
saved in the design; without a GPU viewport the command refuses. In the Realistic look
opaque faces with a scalar look are lit as a dielectric (their colour, roughness 0.5,
IOR 1.5) through the same shading as PBR surfaces; selection and hover highlights, edges,
sketches, faded and see-through surfaces keep the standard drawing, in theme colours. Until
the studio is installed a frame with the look and no product lights draws the standard
picture. Without the look, everything above, the `cpu-product` parity and the viewport BRDF
probe stay as they are. Progressive and CPU renders never
use it. Memory and upload bounds do not change: PBR vertices stay 48 bytes. A program
that embeds the viewport (`solidcraft-gpu`) turns the look on with
`Frame::product_look`, and bakes and installs the studio itself:

```rust
use solidcraft_gpu::view::{ProductLook, StudioEnvironment, StudioPreset};
// Off the window thread (or step by step with `StudioBake` where there are no threads):
let studio = StudioEnvironment::bake(StudioPreset::Product);
renderer.set_studio_environment(&device, &queue, &studio);
frame.product_look = Some(ProductLook { exposure: 1.0, studio: StudioPreset::Product, ..ProductLook::default() });
```

The look takes none of the 21 product-light slots; the frame's product lights, if any,
still light the scene, and the fixed three-light studio of the default preview does not.
The uniform carries the look in one `vec4` after the lights, (exposure, intensity,
rotation, 1), all zero without a look. Out-of-range values are clamped (exposure and
intensity to [0, 1024], rotation wrapped into [0, 2π)) and non-finite ones replaced by
the defaults before upload (`ProductLook::sanitized`). The next `vec4` carries the
scene's world units per millimetre (`Frame::units_per_mm`, with or without the look,
clamped to [10⁻⁶, 10⁶] and 1 when not positive and finite), the scale at which material
lengths given in millimetres are drawn: a constant of the scene, not of a material. The
frame cache key hashes the drawn look and the units, so any change to them draws a new
picture.

The PBR shader is split in two (`pbr::SOURCE` = the shared shading, `pbr::SHADE`, then the vertex stream). A
fragment entry builds a `PbrFragment` (pixel, world position, interpolated normal, view
direction and the pixel's footprint, `fwidth` of the world position, taken before any
branch), asks a material for the `Surface` there (`material_eval`), and returns
`pbr_shade(surface, fragment)`. `Surface` is the WGSL mirror of
`material_graph::SurfaceSample`: the `OpaqueSurface` core, its clear coat and the inputs the
core lacks, with base and coat normals and a tangent. `pbr_shade` holds all of the lighting
above and below, for both looks, and reads the core, the coat and the normal; the other inputs
wait for the shading that reads them. For PBR vertices `material_eval` is the built-in
"vertex constant" material (`surface_from_opaque` of the vertex's values, uncoated); material
graphs generate their own. The split changes no frame of the default preview: of 900 test
frames (five surfaces, a dome and a block, three lightings, the default preview and four
looks, with and without occlusion and supersampling, linear and sRGB targets) every
default-preview frame is byte-identical, and 7 look frames differ by one 8-bit step in one
pixel each (Metal compiles with fast math, so restructured code may round differently).

SolidCraft's paint callback bakes a look's studio on another thread natively and in
`StudioBake::FRAME_WORK` steps a frame on the web, and installs it when done
(`gpu::studio_ready` tells a view to repaint until then).

With the look, the PBR shader:

- adds a procedural **studio** to the product lights: `Product` (four softboxes over a
  light grey cyclorama and a dark floor), `Soft` (two large softboxes, bright cyclorama)
  or `Contrast` (two strips and an overhead panel in a dark studio), turned by
  `rotation` about +Z and scaled by `intensity`. No HDR file is read. Radiance is
  relative, like the product lights; the presets are original visual studies;
- reflects it with split-sum specular: a prefiltered Rgba16Float cube map, 128 × 128
  texels a face at perceptual roughness 0 and five more levels to roughness 1, sampled
  at level `roughness × 5`, times an analytic environment-BRDF fit, with
  single-to-multiple-scattering compensation `1 + F0 (1 / (A + B) − 1)`;
- lights the diffuse layer from nine spherical-harmonic irradiance terms, weighted by
  what the specular lobe does not reflect, so a white lossless surface reflects exactly
  the light it receives;
- scales the radiance by `exposure`, tone maps it with Khronos PBR Neutral instead of
  per-channel Reinhard, encodes it once and adds a ±1 LSB triangular dither (two
  interleaved-gradient noise values) before the 8-bit store. For an sRGB attachment
  (`u.back.w`), the dithered encoded value is decoded again so the attachment's own
  encode quantises it. The 2 × 2 supersample filter and the cached still picture use a
  look variant of the filter that dithers the mean again, so averaging does not cancel
  the dither into bands.

Non-PBR faces get no studio (their pipelines have no studio group): under the look they
are lit by the frame's product lights alone, through the same exposure, PBR Neutral and
dither. A look frame with no product lights therefore draws non-PBR faces black; a client
that wants them lit gives them PBR surfaces or product lights.

The studio is baked on the CPU, so it also works on WebGL2 (no compute shaders): 2 × 2
radiance samples per texel of the sharpest level, a box pyramid of it, and 64 GGX
samples per texel of each rougher level with filtered importance sampling; the
irradiance terms are projected from the 32 × 32 level. A bake takes about 75–110 ms in
an optimised build on an Apple M-series laptop (measured, three presets) and about
1 MB of texels. The renderer never bakes (that would stall the window thread): until a
client installs a preset's studio with `Renderer::set_studio_environment`, the look
draws without it (a stand-in black cube). `StudioEnvironment::bake(preset)` is plain
CPU work for another thread; where there are no threads (the web), `StudioBake` bakes
in bounded steps, a frame's share at a time: `StudioBake::FRAME_WORK` is 152 steps a
studio, each at most 1.8 ms in an optimised native build (measured, three presets),
with the same result as the whole bake. The PBR pipelines gain bind group 3 (the cube,
its sampler and the irradiance uniform); the stand-in is bound without a look. Groups 1
and 2 of their layout are the solid pipelines' (accessibility, occlusion). Group 3 is
the last group WebGL2's limit of four allows.

The environment-BRDF fit is SolidCraft's own: polynomials in perceptual roughness and
`1 − n·v` plus a Smith-masking term, least-squares fitted to the split-sum scale and bias
integrated numerically from the viewport's own GGX/separable-Smith/Schlick BRDF. Its
largest error over n·v in [0.02, 1] and roughness in [0.045, 1] is 0.046 (scale) and
0.023 (bias); RMS 0.006 and 0.004. The sum is kept at or below one.

Mathematics only, from public sources: split-sum prefiltering (Karis 2013, *Real Shading
in Unreal Engine 4*), filtered importance sampling (Křivánek and Colbert 2008, *GPU Gems
3*, ch. 20), nine-term irradiance (Ramamoorthi and Hanrahan 2001), multiple-scattering
compensation (Fdez-Agüera 2019; Filament documentation). The PBR Neutral curve is
adapted from the Khronos Group's reference (Apache-2.0; ATTRIBUTION.md).

The look also draws things the default preview leaves out:

- **Clear coat.** A surface's `ClearCoat` (weight, roughness, IOR) reaches the shader
  through a material graph's surface, not through PBR vertices, which carry none; until
  graphs are drawn, the vertex path draws the uncoated surface. For direct light
  `pbr_coated` is the shared
  coat of [`clear_coat.rs`](render-clear-coat.md) and the path tracer: a single-scattering
  GGX dielectric lobe over the base, which it attenuates at entry and exit by an upper bound
  of its directional reflectance. In the studio the coat reflects its single-scattering
  albedo w·(F0·A + B) at its own roughness and passes the rest to the base; the ambient
  product light reaches the base through the same factor. The interface Fresnel is counted
  once: a layer under a lacquer (a pigmented paint body) takes base IOR 1, which has
  no interface of its own (no specular, no Fresnel attenuation of its diffuse), so only the
  coat reflects at the air boundary. A client that keeps a base IOR above 1 under a coat
  models two interfaces on purpose.
- **Ambient occlusion.** With a frame's `ao_radius` above zero, PBR surfaces join the
  occlusion passes (the depth prepass and the term pass, drawn only with the look) and `fs_pbr` reads the solid pipelines' blurred term through
  `occlusion()`. Occlusion scales only indirect light: the studio's diffuse
  term and the ambient product light by the occlusion, the studio's specular (base and coat)
  by specular occlusion `clamp((n·v + ao)^exp2(−16α − 1) − 1 + ao, 0, 1)` at each lobe's α
  (Lagarde and de Rousiers 2014, *Moving Frostbite to PBR*; mathematics only). Direct product
  lights are never occluded. Product-preview frames (`Frame::product`) draw no prepass, as
  before, so they stay unoccluded.
- **Softbox direct lights.** With the look, a direct product light is treated as a small
  area light for a clear coat: the coat's GGX α for it grows by 0.1 (a light of about 0.2
  rad half-angle; the sphere-light roughness widening of Karis 2013, *Real Shading in Unreal
  Engine 4*, mathematics only). GGX stays normalised, so the light's energy is unchanged and
  a gloss coat shows a soft highlight instead of a clipped dot. The studio's reflection is
  unaffected.

Procedural patterns (a glass weave, orange peel, ink grain) are no longer fixed fields of
the renderer: they become pattern nodes of [material graphs](material-graph.md) (§3.3).

Still out of reach with the look: cast shadows, image textures,
true subsurface or absorption in a coat, crisp rectangular highlights (the cube's
resolution limits the sharpest reflections), and any parity with the CPU hosts, which keep
the fixed studio and draw no coat in the viewport sense above.

Tests (`crates/gpu/src/view/ibl/tests.rs`, `view.rs`): white furnace (every level,
irradiance π, white metal, white diffuse and white dielectric reflect 1 at every
roughness and angle), energy conservation of each prefiltered level (mean within 5 %),
irradiance against a direct cosine integral, the fit against the numerical split sum
and the shader's coefficients against the fit, the actual WGSL environment term and cube
lookups on a native GPU against the CPU reference, the tone curve, the dither's
distribution, the look's sanitising, its uniform, the frame key, a stepped bake against
the whole bake, and GPU frames
showing that a frame without the look draws the documented Reinhard preview with no
dither (against the CPU BRDF) while the look dithers, follows PBR Neutral and adds the
studio once installed (and never bakes it in `prepare`: a look naming a studio that is
not installed draws the lights alone), supersampled and cached too. For the coat and
occlusion: the actual `pbr_coated` against the f64 `OpaqueSurface::with_clear_coat` oracle
on any native GPU, a coated white furnace and the coated studio term against its CPU
reference, the 48-byte vertex, and GPU frames showing that without the look a coated
surface draws uncoated and an occlusion radius changes no pixel, while with it a block
darkens the floor beside it only in the studio's light. The derivatives guard scans
`pbr::SOURCE` and also stops at unparenthesised branches, loops and `continue`. The PBR
entry points (`vs_pbr`, `fs_pbr` and the prepass's) translate to WebGL2's GLSL ES 3.00
through naga's GL backend, with the studio's cube; the GPU frames themselves run on a
native adapter only (Metal here), and no browser has drawn a look frame yet. In
`solidcraft-ui-egui`, a GPU test checks that the paint callback bakes the studio off the
window thread and installs it on a later frame.

## Validation and original example

`examples/render-opaque-studio.json` builds four original spheres and a base,
varying metallic fraction and roughness independently while keeping base color.
Its values are an illustrative optical study, not engineering data for real grades.

The product offscreen test uses the actual installed callback/pipelines on Vulkan,
including both linear and sRGB attachments. For four metal/roughness combinations,
each comparison checks 12,454 interior pixels against CPU PNG output; observed mean
channel error is below 0.2/255, with no pixels beyond the 12/255 outlier threshold.
Silhouettes are excluded because CPU images use 2x supersampling and that test's GPU
target uses 1x. This is a scoped parity result, not browser or universal GPU certification.

Command tests check unchanged geometry/mass, atomic refusal, undo/redo, cold reopen,
physical default/face precedence, retained scalar format-20 looks upgraded to the current
format and explicit v1 catalog refusal. Preset regressions also cover named PBR
inheritance, `pbr: null`, custom looks with matching names, material create/edit
refusal without changing history, density-only edits and all six scene responses.
A clicked editor regression checks one transaction and reopening
the same selected body. Native media and exact current-head gate results belong
in the PR. wasm compilation and actual browser interaction are separate evidence.
