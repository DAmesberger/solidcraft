# Materials and the Realistic view

To see how a surface catches the light, open **Display** in the navigation bar and turn on
**Realistic**. The selected studio lights the faces. An appearance graph can give them a
brushed metal finish, a glossy coat or a pattern. Edges, highlights, dimensions and sketches
keep their modelling colours. In Wireframe, the finish becomes visible when you show faces again.

In a Model view, **Material Finish** beside **Realistic** selects the neutral studio finish.
Each Model view and Render preview has its own Realistic setting. New views start with
Realistic off. Changes apply to the current view for this session.

The first time you turn Realistic on, the GPU prepares the studio and surfaces in the
background. The option is available when the GPU viewport is using full graphics.
Scripts can set it with `view.realistic` and `{"on": true}` or `{"on": false}`.

Appearances without an appearance graph look sensible in Realistic too: a plain colour is lit
as a satin surface (roughness 0.5, refractive index 1.5), and an appearance with metalness
and roughness (PBR) keeps exactly its values.

## Making a material

Materials are made in the **Appearance** dialog (**A**), in its **Appearance graph** section.
An appearance graph belongs to the design, has a name, and can be used by many appearances.

1. Pick the objects to give the material (bodies, faces or components), then open the
   Appearance dialog.
2. In **Appearance graph**, type a name and press **Create**. A new graph starts as one plain
   surface. To start from an existing appearance instead, choose it and press **Convert to
   appearance graph**: the graph draws exactly what the appearance drew.
3. Set the surface's inputs. They are grouped as **Base** (colour, metalness), **Specular**
   (roughness, refractive index, anisotropy), **Coat** (a clear or tinted layer on top),
   **Transmission**, **Emission** and **Geometry** (surface and coat normals, opacity). Each
   input is a fixed value, or driven by a pattern.
4. To drive an input, open its **Drive with a pattern** menu and choose **Noise**,
   **Brushed**, **Weave**, **Checker**, **Grid**, **Gradient** or **Image**. The row then
   shows the pattern's settings, its projection (how it is laid on the part: planar,
   triplanar or cylindrical, in millimetres) and the range its values are remapped to.
   **Use a fixed value** goes back to a constant.
5. To lay a second surface over the first, press **Add layer** under **Layers**: a printed
   mark, an etched logo, a stripe. Each layer has a **Mask** that says where it shows,
   usually an image or a pattern. **Move up** and **Move down** reorder layers; the base layer
   stays at the bottom.
6. Press **Use on picked objects** to give the picked objects the graph, then **Apply** or
   **OK**.

A graph's **appearance inputs** (its interface) are values each appearance can set for
itself, such as a colour. One graph can then serve every colour of the same finish: set the
values in the Appearance dialog, and **Use the graph's default** returns an input to the
graph's own value.

The live swatch shows the material on a sphere and a flat coupon, lit as in Realistic. Below
it, **Cost** shows how much of the graph's budget it uses (2000 units, and 8 texture samples
from at most 4 images). A change that would exceed the budget, or make the graph invalid in
any other way, is not made, and the rows at fault are marked.

Every change is one undo step; dragging a slider is one step when you release it.

A graph made by a script or a library that is not a simple stack of layers opens read-only,
with a summary of its nodes, cost and images. **Edit as JSON** shows its full text, which you
can change and apply with **Replace graph**.

### The built-in materials

SolidCraft ships a small library of ready-made appearance graphs. In the Appearance dialog's
**Local library** section, press **Built-in materials**, choose an entry and apply it to the
picked objects. The graph is copied into the design, so the design never depends on the
library. The library has:

| Material | Appearance inputs | What it shows |
|---|---|---|
| Woven glass fibre | colour, strength | A fine plain glass-cloth weave in the colour and relief; fainter on cut edges |
| Gloss coat over woven glass fibre | colour, coat colour, strength | The weave seen through a tinted gloss coat with a slight orange peel |
| Gloss paint, orange peel | colour | Pigment under a clear gloss coat whose reflections ripple gently |
| Printed ink | colour, strength | Matte ink with a fine grain in its colour and relief |
| Brushed stainless steel | – | Brushed metal with stretched, directional highlights |
| Laser-etched stainless steel | mark (an image) | Satin metal with a dark, pitted mark where the image is opaque, on a 12 × 8 mm lid |
| Candy coat over polished aluminium | coat colour | Polished aluminium seen through a tinted gloss coat |
| Gloss black plastic, clear coated | – | A finely textured black moulding under a clear lacquer |
| Printed label with stripe | label colour, print (an image) | A plastic label with a stripe and printed text, wrapped around the Z axis |

The values are visual estimates, not measured optical data. Details, and how these graphs
are built: [built-in appearance graphs](https://github.com/tangency-labs/solidcraft/blob/main/docs/render-appearance-graphs.md).

## Textures

An **Image** pattern reads a picture: a label, a logo, a mask, or a photographed surface.
Choose **Add texture…** in its row, or pick one the design already holds.

- PNG and JPEG are accepted, 8-bit, and 16-bit PNG for data such as height. Colour images
  are read as sRGB; data (roughness, masks, normal maps) as linear values.
- An image may be up to 16384 pixels a side and 256 MiB as a file. A design holds at most
  256 images and 1 GiB of them decoded. These limits only protect against damaged or hostile
  files; they never reduce quality.
- The file keeps every image at full quality. The viewport chooses how much of it to keep on
  the graphics card: usually everything on a desktop, at most 2048 pixels a side in a
  browser. Images are filtered smoothly at every distance.
- Images are part of the design: in a `.sc` design they are stored inside the file, in a
  plain JSON design as text. An image nothing uses any more is dropped when you save.
- Until an image has loaded, its pattern shows its default colour and its mask shows nothing.
- Parts have no texture coordinates of their own, so every image is projected: planar
  (a frame with an origin and two axes spanning one copy of the image), triplanar (from the
  three axis directions, blended) or cylindrical (wrapped around an axis).

A library that holds images is saved as a `.sclib` file rather than `.json`.

## Compatibility

- **Designs without appearance graphs** are saved exactly as before: the same file, byte for
  byte, readable by the releases that read them before. Adding a graph and deleting it again
  returns the design to that form.
- **Designs with appearance graphs** need a release that knows them. An older release refuses
  to open such a design and asks you to update SolidCraft; it never opens it partly or drops
  the materials.
- **Appearances with metalness and roughness (PBR)** are never rewritten into graphs. Only
  **Convert to appearance graph** makes one, when you ask for it.
- Views with Realistic off, exports (STEP and 3MF colours) and older tools show each appearance's
  colour. For an appearance with a graph, that colour is the graph's base colour, kept up to
  date whenever the graph changes.

A design is written in the oldest file format that holds everything it contains. A design
without graphs therefore never needs the appearance-graph change: it stays readable by the
release that brings the file format changes now pending, and by any later one, even when a
release with appearance graphs saved it.

## Scripts and agents

Every step above is a command, and the same commands run from the CLI and MCP:
[appearance graph commands](https://github.com/tangency-labs/solidcraft/blob/main/docs/appearance-graph-commands.md).
The example design `examples/appearance-graphs.json` builds several parts with built-in
materials from commands.
