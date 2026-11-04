# Built-in material graphs

SolidCraft's built-in material library and the bindings of the built-in render recipes, both
as material graphs ([design note](material-graph.md), §3.7, §3.8 and §4.5). User guide:
[materials and the Realistic view](book/src/guide/materials.md). Commands:
[material graph commands](material-graph-commands.md).

All values here are original visual estimates, not measured optical data.

## The built-in library

The library is the catalog v3 `solidcraft-materials` ("SolidCraft materials"). Every place that
reads a library reads it from the path `builtin:materials`: `library.inspect`,
`library.apply`, `appearance.graph.update_from_library`, and the Appearance dialog's **Built-in
materials** button. It is assembled from `assets/appearances/graphs/` when it is read:
`library.rs` lists each graph's id, the colour its look shows in the standard view, and its
JSON file. Entry ids and revisions follow from the content (`Catalog::builtin`), as for any
exported library, so a changed graph is a new revision that `library.apply` asks for
explicitly and `update_from_library` previews. A test applies every entry through
`library.apply`, checks that each graph is valid within the budget, and that each listed
colour is exactly the graph's base response.

`library.apply` copies the graph into the design with `source: {library:
"solidcraft-materials", entry: <graph id>, revision}`; the design never depends on the
library afterwards.

| Graph id | Name | Interface | Cost (units of 2000) |
|---|---|---|---|
| `woven-laminate` | Woven laminate | `color` (color3), `strength` (float, 1) | 585 |
| `coated-woven-laminate` | Gloss coat over woven laminate | `color`, `coat_color` (color3), `strength` (0.5) | 572 |
| `orange-peel-gloss` | Gloss paint, orange peel | `color` | 378 |
| `printed-ink` | Printed ink | `color`, `strength` (1) | 384 |
| `brushed-nickel-silver` | Brushed nickel silver | – | 39 |
| `laser-etched-nickel-silver` | Laser-etched nickel silver | `mark` (texture) | 208 (1 sample) |
| `tinted-coat-over-copper` | Tinted gloss coat over copper | `coat_color` | 136 |
| `black-epoxy-clear-coat` | Black epoxy, clear coated | – | 242 |
| `printed-sleeve` | Printed sleeve with stripe | `sleeve` (color3), `print` (texture) | 31 (1 sample) |

A texture input has no default: until a look binds an image (`values: {"mark": {"texture":
sha256}}`, or a runtime override, §7.3), the image node reads its default colour with alpha 0,
so the mark or print does not show.

### The four surface patterns

The renderer branch had four fixed surface patterns (`SurfacePattern`). They are now these
graphs, built from catalogue nodes only, so nothing in the renderer knows them.

- **Woven laminate** (was `Weave`). A plain `pattern.weave`, pitch 0.577 × 0.819 mm (a coarse
  plain-weave glass cloth of 44 × 31 yarns per inch), seven fibres per yarn at contrast 0.3,
  on a `texcoord.triplanar` in object space with `scale_mm` (1, 1, 0.173), so the weave's
  period up through an edge is a 0.173 mm ply. The weave's value is remapped to an albedo
  factor of 1 ± 0.2·k and multiplies `color`; its height drives `normal.from_height` with
  6 µm relief, also scaled by k. k is `strength` × `mix(0.25, 1, smoothstep(0.65, 0.75,
  |n.z|))` from `geometry.normal`: full on faces (|n.z| ≥ 0.75), a quarter on edges. This
  equals the old look on faces and axis-aligned edges; across a diagonal edge triplanar blends
  two planes where the old look ran along the edge (§3.7).
- **Gloss paint, orange peel** (was `OrangePeel`). Two `pattern.noise` (value, 2D) at 2.4 mm
  and 0.9 mm on a triplanar projection, summed with weights 1 and 1/6 into
  `normal.from_height` at 6 µm, so the relief is 6 µm and 1 µm. It feeds
  `geometry_coat_normal` of a clear coat (weight 1, roughness 0.08, IOR 1.5) over the
  pigment, whose `specular_ior` is 1: the coat is the only interface.
- **Gloss coat over woven laminate** (was `PeelOverWeave`). The weave of the first graph
  drives only the albedo (no base relief), under a tinted coat (`coat_color`, roughness 0.12,
  IOR 1.52) that carries the orange peel. `strength` is the weave's show-through, 0.5 by
  default.
- **Printed ink** (was `InkGrain`). Noise at 0.05 mm and 0.16 mm, summed: an albedo of ±18 %
  per noise (factor 1 ± 0.36·`strength` over the sum's range) and 2 µm of relief, on a
  matte dielectric (roughness 0.6, IOR 1.5).

Every pattern fades to its mean as its period approaches the pixel footprint, as the old
`pat_fade` and `pat_weave_fade` did; that is built into the pattern nodes (§3).

### The worked examples

The other five are the worked examples of §3.8, under generic names and with an interface
where one is natural:

- **Brushed nickel silver**: `pattern.brushed` along object X (40 lines per mm, 4 mm
  streaks) drives roughness 0.41–0.49 and a ±3 % tint; anisotropy 0.6 along the brushing
  tangent.
- **Laser-etched nickel silver**: a `surface.mix` of plain metal and a dark, rough, pitted
  metal (cellular noise at 0.03 mm, 3 µm relief), masked by the alpha of the `mark` image on
  a 12 × 8 mm planar frame centred on the object origin.
- **Tinted gloss coat over copper**: copper (metal, roughness 0.35) under a tinted gloss coat
  with orange peel; `coat_color` sets the tint (green by default).
- **Black epoxy, clear coated**: 3D cellular noise at 0.04 mm (an EDM texture) varies the
  roughness 0.40–0.52 under a clear coat (roughness 0.08).
- **Printed sleeve with stripe**: a cylindrical projection about object Z (radius 4 mm,
  unwrapped by angle) lays a stripe between 2.6 and 2.62 rad and the `print` image's alpha
  as a light ink over the `sleeve` colour.

## Starter and Selene bindings

`render.library.appearance.apply` with `variant: "graph"` turns a built-in render recipe into
a graph (§4.5): a starter surface (`SURF-*`, `assets/render/starter/materials/`) binds its
base colour and OpenGL normal maps, and with a `frame` its roughness and metallic maps; a
Selene appearance (`SELENE-AP-*`) becomes the one-node graph of its baseline.

A recipe's **direction** is bound from
`assets/appearances/graphs/render-library-bindings.json`. A listed entry gets
`specular_roughness_anisotropy` and `specular_rotation` (its lines' angle from the
projection's u axis) and takes `geometry_tangent` from the projection's tangent; a Selene
entry with a direction gets a tangent-only projection (triplanar in object space, or planar in
`frame`). An entry not listed is isotropic.

**The amounts are authored estimates, not measurements.** They come from the recipe's
`pattern` and the direction in which its roughness map varies least (for example, a brushed
recipe's map varies along v, so its lines run along u). The table says so (`"basis":
"authored_estimate"`), and so does each reply (`anisotropy: {value, rotation_rad, basis,
note}`).

| Entries | Pattern | Anisotropy | Rotation | Note |
|---|---|---|---|---|
| SURF-001, 008, 009 | brushed | 0.6 | 0 | lines along u |
| SURF-011 | satin brushed | 0.5 | 0 | |
| SURF-018 | brushed under an anodic layer | 0.4 | 0 | softened |
| SURF-006 | turned | 0.5 | 0 | straight along u in the tile; around a part's axis only with a frame that lays u along the circumference |
| SURF-005, 010, 021, 022 | milled | 0.3 | π/2 | arcs, mostly along v |
| SURF-027 to 030 | wood | 0.15 | π/2 | grain along v |
| SURF-040, 042, 043 | printed layer lines | 0.3 | 0 | |
| SURF-041 | printed top skin | 0.3 | −π/4 | along the u–v diagonal |
| SURF-002 | cross-brushed | 0 | – | two crossing directions, which one tangent cannot express |
| SELENE-AP-007 | linear brushing | 0.6 | 0 | |
| SELENE-AP-009, 013, 028 | directional machining, finish, extrusion lines | 0.3 | 0 | |
| SELENE-AP-005, 011, 032 | concentric marks, threads | 0 | – | need an axis, which a planar or triplanar tangent lacks |

With the binding, the graph variant's `omitted` list no longer names
`true_anisotropic_brdf`. It always names `height_displacement`, because graphs never move
geometry (height feeds only the shading normal), and under the default triplanar projection
also `roughness_metallic_maps_under_triplanar`.

## The example design

`examples/material-graphs.json` is a command script (`solidcraft-cli run
examples/material-graphs.json --out material-graphs.sc`): a row of primitive parts given
eight of the built-in graphs by `library.apply` from `builtin:materials`, two of them with
their own interface colour through `appearance.assign`, and a brushed Selene housing
(`SELENE-AP-007`) through `render.library.appearance.apply`, which shows the binding table's
anisotropy. The sleeve stands on the origin, the axis of its cylindrical projection. Open the
result and press Shift+P to see it in the Realistic view.

The script names each entry by its id and revision, which follow from the graph's content: a
changed built-in graph changes them. `the_material_graphs_example_builds` (engine tests) runs
the script, and when an entry has changed it lists the current ids and revisions to put in.
