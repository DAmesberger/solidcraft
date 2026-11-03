# Material graph commands (CLI and MCP)

Material graphs are design-owned surface definitions that looks refer to by id
([design note](material-graph.md), §4 and §7). This page is the automation reference: the
commands, what they take and return, and how to run them from `solidcraft-cli run` or the
MCP server. Every command has input and output schemas (`solidcraft-cli commands`, or the
MCP `describe_command` tool), and none opens a dialog.

**Every change is atomic and one undo step.** A command builds the whole change on a copy
of the design, checks it (validation, the texture limits and every look that uses the
graph), and only then replaces the design. A refused command leaves the design, its history
and its revision exactly as they were.

## Reading

| Command | Params | Returns |
|---|---|---|
| `appearance.graph.nodes` | – | `{version, nodes}`: every node kind with typed inputs, ranges, defaults, params, outputs and cost |
| `appearance.graph.validate` | `{graph}` | `{ok, cost, samples, textures, shape, errors: [{node?, port?, code, message}]}`; the design is not consulted |
| `appearance.graph.list` | – | `{graphs: [{id, name, uses, source?}]}` |
| `appearance.graph.get` | `{id}` | `{id, graph, cost, uses}` |
| `appearance.texture.list` | – | `{textures: [{texture, name, width, height, bytes, uses}]}` |

`uses` counts every look that names the graph (bodies, faces, components, physical
materials, linked designs' baselines and configuration cells). A texture's `uses` counts
the graphs and looks that name it.

## Graphs

| Command | Params | Returns |
|---|---|---|
| `appearance.graph.create` | `name`, `id?`, and `graph?` or `from?` | `{id, cost}` |
| `appearance.graph.set` | `id`, `graph` | `{id, cost}` |
| `appearance.graph.edit` | `id`, `ops` (1–512) | `{id, cost}` |
| `appearance.graph.duplicate` | `id`, `new_id?`, `name?` | `{id, cost}` |
| `appearance.graph.delete` | `id`, `replace_with?` | `{replaced}` |
| `appearance.graph.convert` | `look`, `id?`, `name?`, `scope?` | `{id, cost, count}` |
| `appearance.graph.update_from_library` | `id`, `library`, `preview?` | `{applied, changes, looks}` |

- **Ids** are 1–64 characters of `a–z 0–9 _ . -`. Without `id`, one is made from the name
  (`"Brushed Nickel/Silver"` → `brushed-nickel-silver`, then `-2`, `-3` …). A given id that
  is taken is refused.
- **`create`** starts from `graph`, or `from: "standard"` (one `standard_surface` with its
  defaults, also the default), or `from: {name, color, opacity?, pbr?}`: the one-node graph
  that draws that scalar or PBR look exactly. The graph's `name` becomes `name`.
- **`set`** replaces the graph as a whole, its `layout` and `source` included.
- **`edit`** applies its ops to a copy in order; if any op or the result is invalid, none
  applies. Ops:
  - `{op: "add_node", node, kind}`: a node with its defaults;
  - `{op: "remove_node", node}`: also drops every connection from it (those inputs return to
    their defaults); the output node cannot be removed;
  - `{op: "connect", from, node, port, fallback?}`: `from` is `node` or `node.output`;
    `fallback` is the constant the base response (the standard view's colour) reads instead;
  - `{op: "disconnect", node, port}`: the input returns to its default;
  - `{op: "set_input", node, port, value}`: a constant (a number, a 2- or 3-vector,
    `{"srgb8": [r, g, b]}`, `{"texture": sha256}`) or `{"interface": name}`;
  - `{op: "set_param", node, param, value}`: an option name or an integer;
  - `{op: "set_interface", input: {name, type, default?}}`: adds or replaces by name.
- **Looks follow their graph.** A look that names a graph shows the graph's base response in
  the standard view, so its `color` is computed from it. `set`, `edit`, `delete` with
  `replace_with` and `update_from_library` recompute the colour of every look that uses the
  graph, in the same undo step. A change that would break a look (an interface input it sets
  removed, a value out of the new range, see-through optics on a translucent look) is refused.
- **`delete`** is refused while looks use the graph, unless `replace_with` names the graph they
  use instead. Their values the new graph does not expose are dropped.
- **`convert`** ("Convert to material graph") takes a saved look as `appearance.list` shows it,
  makes the one-node graph that draws it, and points every override that is exactly that look
  (within `scope: {bodies?, components?}`, as `appearance.replace` takes it) at the graph.
  The picture does not change. Physical materials' default looks are not rewritten.
- **`update_from_library`** compares the graph with the one its `source` names in the catalog
  file `library` (whose id must be `source.library`). `preview: true` reports the
  `changes` (`"<node>.<input>: old → new"`, nodes added or removed, interface changes) and the
  looks affected, changing nothing. Without it the library's graph and its textures replace the
  design's, keeping the design's layout if the library has none.

## Textures

`appearance.texture.add {path | data_base64, name?}` adds a PNG or JPEG of at most 16384 px a
side and 256 MiB, kept at full quality, and returns `{texture, width, height}`; `texture` is
the SHA-256 of the bytes, which graphs and look values use (`{"texture": "<sha256>"}`). Adding
the same bytes again changes nothing. A design holds at most 256 textures, decoding to at
most 1 GiB together. A texture nothing references is dropped when the design is saved, so add
one and use it in the same session. `data_base64` also works in the browser, where `path`
reads only files the page was given.

## Looks with graphs

`appearance.assign` (and `appearance.replace`'s `to`, `appearance.paint`, `appearance.paste`,
`material.create` and `material.edit`) take a graph look:

```json
{"bodies": ["Can"], "graph": "anodised", "values": {"tint": {"srgb8": [200, 30, 30]}}, "name": "Red anodised"}
```

- `graph` names a graph of the design; `values` sets its interface inputs (a texture input
  takes `{"texture": sha256}` of a texture the design holds); `name` defaults to the graph's
  id and `opacity` to 1.
- `graph` excludes `pbr` and a library `appearance`. A given `color` is replaced by the
  graph's base colour.
- `appearance.list`, `appearance.copy` and `appearance.pick_up` return `graph` and `values`, so
  a copied look pastes with its graph.

`library.apply` copies an entry's graph, with its source and its textures, into the design.
The same graph already under that id is reused; a different one is refused unless
`graph_id` names a new id for the copy.

`render.library.appearance.apply` with `variant: "graph"` adds a built-in recipe as a graph
and assigns it:

- a `SURF-*` recipe (in builds with the bundled render library) binds its base colour and
  OpenGL normal maps, triplanar at the recipe's tile size; with `frame: {origin?, u, v}` it is
  planar and binds the roughness and metallic maps too;
- a `SELENE-AP-*` entry, which has no maps, becomes the one-node graph of its baseline;
- `graph_id` chooses the id (default: the entry's id in lower case);
- the reply's `omitted` always lists `height_displacement`, because graphs never move geometry.

`variant: "uniform_opaque"` is unchanged.

## Runtime overrides (`render.start`)

A progressive GPU path job can override graph interface values for that job only. The
design, its history and its file do not change.

```json
{"command": "render.start", "params": {"progressive": true, "raytrace": {
  "graph_values": [
    {"graph": "anodised", "values": {"tint": [0.9, 0.1, 0.1]}},
    {"body_index": 2, "body_name": "Label", "values": {"mark": {"texture": "<sha256 of the image below>"}}}
  ],
  "graph_textures": ["<base64 PNG>"]
}}}
```

- An override names `graph` (wherever it is bound) or `body_index` with `body_name` (as
  `render.scene` lists the body), and merges `values` over the looks' own. At most 256.
- A texture value names a design texture or one of `graph_textures` (at most 16, each at most
  16 MiB), by SHA-256. A runtime texture no override reads is refused.
- `render.preflight` and `render.start` report the captured `graphs`: per material its graph,
  values, cost and texture count, and the number of bindings.

## Examples

A script for `solidcraft-cli run script.json --out can.sc`:

```json
{"commands": [
  {"command": "solid.box", "params": {"body_name": "Can", "length": 20, "width": 20, "height": 30}},
  {"command": "appearance.graph.create", "params": {"id": "nickel", "name": "Nickel silver"}},
  {"command": "appearance.graph.edit", "params": {"id": "nickel", "ops": [
    {"op": "set_input", "node": "surface", "port": "base_color", "value": {"srgb8": [227, 224, 218]}},
    {"op": "set_input", "node": "surface", "port": "base_metalness", "value": 1},
    {"op": "set_input", "node": "surface", "port": "coat_weight", "value": 1},
    {"op": "set_input", "node": "surface", "port": "coat_roughness", "value": 0.1}]}},
  {"command": "appearance.assign", "params": {"bodies": ["Can"], "graph": "nickel"}}
]}
```

Over MCP the same steps go through `execute` (or `batch`), e.g.
`{"command": "appearance.graph.validate", "params": {"graph": {…}}}` before a `set`, to point
each error at its node and port.
