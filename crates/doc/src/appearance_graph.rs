//! The design-level records of material graphs (docs/material-graph.md §4.1): a table of named
//! graphs that looks reference by id (decision O1), and the texture assets they read.
//!
//! `Appearances::tables` holds them (`appearances.graphs` and `appearances.textures`, both
//! omitted when empty) and a [`Look`] names a graph with `graph` and sets its interface with
//! `values`. The format change is `format/pending/appearance-graphs.rs`.
//!
//! What is checked where:
//! - reading a look: `graph` and `pbr` exclude each other, `values` need a graph and are bounded
//!   ([`check_look_shape`]);
//! - reading the tables: their counts ([`GraphTables::check_counts`]);
//! - reading a design: everything else, in [`crate::Document::check_appearance_graphs`] — each
//!   texture's hash, header and size, each graph's bounds and texture references, the design's
//!   decoded-texture budget, and every look's graph, values and opacity rule.
//!
//! Saving drops textures nothing references ([`crate::Document::without_unreferenced_textures`]);
//! graphs stay until deleted.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solidcraft_render::material_graph::graph::TextureRef;
pub use solidcraft_render::material_graph::limits;
pub use solidcraft_render::material_graph::{Constant, MaterialGraph};
use solidcraft_render::material_graph::{GraphError, InputValue};

use crate::appearance::Look;
use crate::{DocError, Document, Result};

fn invalid(message: impl Into<String>) -> DocError {
    DocError::Invalid(message.into())
}

/// A name a person gave: 1–`limits::NAME_BYTES` bytes, no control characters.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= limits::NAME_BYTES && !name.chars().any(char::is_control)
}

/// Whether `key` is a SHA-256 as texture keys spell it: 64 lowercase hex digits.
pub fn is_texture_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The key of a texture with these encoded bytes: their SHA-256, lowercase hex.
pub fn texture_key(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A graph's id in the design's table: 1–64 bytes of `[a-z0-9_.-]`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GraphId(String);

impl GraphId {
    pub fn new(id: &str) -> crate::Result<Self> {
        let ok = !id.is_empty()
            && id.len() <= limits::ID_BYTES
            && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'));
        if ok {
            Ok(GraphId(id.to_string()))
        } else {
            Err(crate::DocError::Invalid(format!("a graph id is 1–{} characters of a–z, 0–9, _ . -", limits::ID_BYTES)))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GraphId {
    type Error = crate::DocError;
    fn try_from(id: String) -> crate::Result<Self> {
        GraphId::new(&id)
    }
}

impl From<GraphId> for String {
    fn from(id: GraphId) -> String {
        id.0
    }
}

/// A texture asset, keyed by the SHA-256 of its exact encoded bytes. In a container its bytes
/// are `image_blob` (`blobs/<sha256>.png|jpeg`, the canvas blob kinds); in plain JSON, `data`
/// holds base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureAsset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// `png` or `jpeg`.
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_blob: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
}

/// The design's graphs and textures, as `Appearances` will hold them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphTables {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub graphs: Graphs,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub textures: Textures,
}

impl TextureAsset {
    /// A texture asset from encoded PNG or JPEG bytes, with its key. `name` is for people (the
    /// file name it came from, say). Checked as a design's textures are on reading.
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<(String, TextureAsset)> {
        let (format, [width, height]) = crate::canvas::image_info(bytes).ok_or_else(|| invalid("a texture is a PNG or JPEG image"))?;
        let asset = TextureAsset {
            name: name.to_string(),
            width,
            height,
            format: format.to_string(),
            image_blob: None,
            data: crate::canvas::base64_encode(bytes),
        };
        let key = texture_key(bytes);
        asset.checked(&key)?;
        Ok((key, asset))
    }

    /// The encoded bytes, after checking them against `key` and the record ([`Self::checked`]).
    pub fn bytes(&self, key: &str) -> Result<Vec<u8>> {
        Ok(self.checked(key)?.0)
    }

    /// Check the asset as the design's texture `key`: its name and format, bytes present (base64
    /// `data`, never an unresolved `image_blob`), within the file's safety limits (§4.3), hashing
    /// to `key`, and a header that agrees with `format`, `width` and `height`. Returns the bytes
    /// and the decoded size the header promises (RGBA8 4 B/px, 16-bit grey data 2 B/px).
    pub fn checked(&self, key: &str) -> Result<(Vec<u8>, usize)> {
        let what = || format!("texture `{}`", key.get(..12).unwrap_or(key));
        if !is_texture_key(key) {
            return Err(invalid("a texture's key is the SHA-256 of its bytes: 64 lowercase hex digits"));
        }
        if !plain_name(&self.name) || !matches!(self.format.as_str(), "png" | "jpeg") {
            return Err(invalid(format!("{} needs a name of 1–{} bytes and the format png or jpeg", what(), limits::NAME_BYTES)));
        }
        if self.image_blob.is_some() {
            return Err(invalid(format!("{}: `image_blob` names a container part, so it cannot appear in a plain design", what())));
        }
        if self.data.len() > limits::TEXTURE_BYTES.div_ceil(3).saturating_mul(4) {
            return Err(invalid(format!("{} is over {} MiB", what(), limits::TEXTURE_BYTES >> 20)));
        }
        let bytes = crate::canvas::base64_decode(&self.data).ok_or_else(|| invalid(format!("{}: invalid base64 data", what())))?;
        if bytes.is_empty() || bytes.len() > limits::TEXTURE_BYTES {
            return Err(invalid(format!("{} must hold 1 byte to {} MiB", what(), limits::TEXTURE_BYTES >> 20)));
        }
        if texture_key(&bytes) != key {
            return Err(invalid(format!("{}: its bytes do not hash to its key", what())));
        }
        let side = 1..=limits::TEXTURE_SIDE;
        match crate::canvas::image_info(&bytes) {
            Some((format, [w, h])) if format == self.format && w == self.width && h == self.height && side.contains(&w) && side.contains(&h) => {}
            _ => {
                return Err(invalid(format!(
                    "{}: its header must be a {} image of {}×{} px, 1–{} px a side",
                    what(),
                    self.format,
                    self.width,
                    self.height,
                    limits::TEXTURE_SIDE
                )));
            }
        }
        // PNG: bit depth at byte 24, colour type at 25. Grey 16-bit is a data map (height).
        let per_pixel = if self.format == "png" && bytes.get(24) == Some(&16) && bytes.get(25) == Some(&0) { 2 } else { 4 };
        let decoded = (self.width as usize).saturating_mul(self.height as usize).saturating_mul(per_pixel);
        Ok((bytes, decoded))
    }
}

impl GraphTables {
    pub fn is_empty(&self) -> bool {
        self.graphs.is_empty() && self.textures.is_empty()
    }

    /// The table sizes, checked as a file is read, before anything looks inside.
    pub fn check_counts(&self) -> Result<()> {
        if self.graphs.len() > limits::GRAPHS_PER_DESIGN || self.textures.len() > limits::TEXTURES_PER_DESIGN {
            return Err(invalid(format!(
                "a design holds at most {} material graphs and {} textures",
                limits::GRAPHS_PER_DESIGN,
                limits::TEXTURES_PER_DESIGN
            )));
        }
        Ok(())
    }

    /// Check the tables on their own ([`check_tables`]).
    pub fn check(&self) -> Result<()> {
        check_tables(&self.graphs, &self.textures)
    }

    /// Check a look against these tables ([`check_look_in`]).
    pub fn check_look(&self, look: &Look) -> Result<()> {
        check_look_in(look, &self.graphs, &self.textures)
    }

    /// Every texture key the graphs name (constants, connection fallbacks, interface defaults).
    pub fn graph_textures(&self) -> impl Iterator<Item = &str> {
        self.graphs.values().flat_map(graph_textures)
    }
}

/// A design's or a library's graph table.
pub type Graphs = BTreeMap<GraphId, MaterialGraph>;
/// A design's or a library's texture table, by SHA-256.
pub type Textures = BTreeMap<String, TextureAsset>;

/// Check graph and texture tables: their counts, every texture ([`TextureAsset::checked`]) and
/// their decoded budget, and every graph ([`check_graph_in`]).
pub fn check_tables(graphs: &Graphs, textures: &Textures) -> Result<()> {
    if graphs.len() > limits::GRAPHS_PER_DESIGN || textures.len() > limits::TEXTURES_PER_DESIGN {
        return Err(invalid(format!("at most {} material graphs and {} textures", limits::GRAPHS_PER_DESIGN, limits::TEXTURES_PER_DESIGN)));
    }
    let mut decoded = 0usize;
    for (key, texture) in textures {
        decoded = decoded.saturating_add(texture.checked(key)?.1);
        if decoded > limits::DECODED_TEXTURE_BYTES {
            return Err(invalid(format!("textures decode to at most {} MiB together", limits::DECODED_TEXTURE_BYTES >> 20)));
        }
    }
    for (id, graph) in graphs {
        check_graph_in(id, graph, textures)?;
    }
    Ok(())
}

/// One graph against the limits and the textures it may name, then [`validate`]d. Until
/// validation lands (track A), its "not built yet" refusal admits the graph.
///
/// [`validate`]: solidcraft_render::material_graph::validate
pub fn check_graph_in(id: &GraphId, graph: &MaterialGraph, textures: &Textures) -> Result<()> {
    let what = || format!("material graph `{}`", id.as_str());
    if !plain_name(&graph.name) {
        return Err(invalid(format!("{} needs a name of 1–{} bytes without control characters", what(), limits::NAME_BYTES)));
    }
    if graph.nodes.len() > limits::NODES || graph.interface.len() > limits::INTERFACE_INPUTS || graph.layout.len() > limits::NODES {
        return Err(invalid(format!("{} has more than {} nodes or {} interface inputs", what(), limits::NODES, limits::INTERFACE_INPUTS)));
    }
    if graph.layout.values().flatten().any(|c| !c.is_finite() || c.abs() > limits::LAYOUT_COORD) {
        return Err(invalid(format!("{}: layout positions are finite and within ±{}", what(), limits::LAYOUT_COORD)));
    }
    if let Some(missing) = graph_textures(graph).find(|key| !textures.contains_key(*key)) {
        return Err(invalid(format!("{} names texture `{missing}`, which is not held beside it", what())));
    }
    match solidcraft_render::material_graph::validate(graph) {
        Ok(_) | Err(GraphError::NotBuilt(_)) => Ok(()),
        Err(e) => Err(invalid(format!("{}: {e}", what()))),
    }
}

/// Check a look against graph and texture tables: its shape ([`check_look_shape`]), the graph it
/// names, its values (each an interface input of that graph; textures it names are held) and the
/// opacity rule: a see-through graph ([`see_through`]) needs opacity 1, as `pbr` does.
pub fn check_look_in(look: &Look, graphs: &Graphs, textures: &Textures) -> Result<()> {
    check_look_shape(look)?;
    let Some(id) = &look.graph else { return Ok(()) };
    let graph =
        graphs.get(id).ok_or_else(|| invalid(format!("look `{}` names material graph `{}`, which is not held beside it", look.name, id.as_str())))?;
    for (name, value) in &look.values {
        if !graph.interface.iter().any(|input| &input.name == name) {
            return Err(invalid(format!("look `{}` sets `{name}`, which material graph `{}` does not expose", look.name, id.as_str())));
        }
        if let Constant::Texture(TextureRef { texture }) = value
            && !textures.contains_key(texture)
        {
            return Err(invalid(format!("look `{}` names texture `{texture}`, which is not held beside it", look.name)));
        }
    }
    if look.opacity != 1.0 && see_through(graph, look) {
        return Err(invalid(format!(
            "look `{}`: material graph `{}` sets its own opacity or transmission, so the look's opacity must be 1",
            look.name,
            id.as_str()
        )));
    }
    Ok(())
}

/// The texture keys one graph names: constant inputs, connection fallbacks and interface defaults.
pub fn graph_textures(graph: &MaterialGraph) -> impl Iterator<Item = &str> {
    fn texture(c: &Constant) -> Option<&str> {
        match c {
            Constant::Texture(TextureRef { texture }) => Some(texture.as_str()),
            _ => None,
        }
    }
    let inputs = graph.nodes.iter().flat_map(|n| n.inputs.values()).filter_map(|v| match v {
        InputValue::Constant(c) => texture(c),
        InputValue::Connection(c) => c.fallback.as_ref().and_then(texture),
        InputValue::Interface(_) => None,
    });
    let defaults = graph.interface.iter().filter_map(|i| i.default.as_ref().and_then(texture));
    inputs.chain(defaults)
}

/// The texture keys a look's values name.
pub fn look_textures(look: &Look) -> impl Iterator<Item = &str> {
    look.values.values().filter_map(|c| match c {
        Constant::Texture(TextureRef { texture }) => Some(texture.as_str()),
        _ => None,
    })
}

/// Whether a graph, as `look` sets it, draws its own see-through: a standard surface whose
/// `geometry_opacity` is below 1 or `transmission_weight` above 0. A constant is read as it is,
/// an interface input from the look's value or its default; a connected input counts as
/// see-through, since it can vary over the surface (docs/material-graph.md §4.1).
pub fn see_through(graph: &MaterialGraph, look: &Look) -> bool {
    let number = |value: &InputValue| -> Option<f64> {
        let constant = match value {
            InputValue::Constant(c) => Some(c),
            InputValue::Interface(r) => {
                look.values.get(&r.interface).or_else(|| graph.interface.iter().find(|i| i.name == r.interface).and_then(|i| i.default.as_ref()))
            }
            InputValue::Connection(_) => return Some(f64::NAN),
        };
        match constant {
            Some(Constant::Number(x)) => Some(*x),
            _ => None,
        }
    };
    graph.nodes.iter().filter(|n| n.kind == "standard_surface").any(|n| {
        let opacity = n.inputs.get("geometry_opacity").and_then(number);
        let transmission = n.inputs.get("transmission_weight").and_then(number);
        opacity.is_some_and(|x| !(x >= 1.0)) || transmission.is_some_and(|x| !(x <= 0.0))
    })
}

/// A look on its own: `graph` and `pbr` exclude each other, `values` need a graph, at most
/// `limits::INTERFACE_INPUTS` of them, named as interface inputs are, with finite numbers,
/// two- or three-component vectors and texture keys spelled as SHA-256s.
pub fn check_look_shape(look: &Look) -> Result<()> {
    if look.graph.is_some() && look.pbr.is_some() {
        return Err(invalid("a look has either `pbr` or a material `graph`, not both"));
    }
    if look.graph.is_none() && !look.values.is_empty() {
        return Err(invalid("a look's `values` set a material graph's inputs, so they need a `graph`"));
    }
    if look.values.len() > limits::INTERFACE_INPUTS {
        return Err(invalid(format!("a look sets at most {} graph inputs", limits::INTERFACE_INPUTS)));
    }
    for (name, value) in &look.values {
        if name.is_empty() || name.len() > limits::ID_BYTES {
            return Err(invalid(format!("a graph input's name is 1–{} bytes", limits::ID_BYTES)));
        }
        let ok = match value {
            Constant::Number(x) => x.is_finite(),
            Constant::Vector(v) => matches!(v.len(), 2 | 3) && v.iter().all(|x| x.is_finite()),
            Constant::Srgb8(_) => true,
            Constant::Texture(TextureRef { texture }) => is_texture_key(texture),
        };
        if !ok {
            return Err(invalid(format!("graph input `{name}`: a finite number, a 2- or 3-component vector, an sRGB8 colour or a texture key")));
        }
    }
    Ok(())
}

impl Document {
    /// Call `f` with every look the design holds: body, face and component appearances,
    /// physical materials' looks, what linked designs last gave, and the looks configuration
    /// cells spell out.
    pub fn for_each_look(&self, f: &mut dyn FnMut(&Look) -> Result<()>) -> Result<()> {
        let a = &self.appearances;
        for look in a.bodies.values().chain(a.components.values()).chain(a.faces.iter().map(|face| &face.look)) {
            f(look)?;
        }
        for look in self.user_materials.iter().filter_map(|m| m.appearance.as_ref()) {
            f(look)?;
        }
        for inherited in self.links.values().flat_map(|link| &link.inherited) {
            for look in inherited.appearance.iter().chain(inherited.faces.iter().flatten().map(|face| &face.look)) {
                f(look)?;
            }
        }
        use crate::config::Column;
        let columns: Vec<usize> = self
            .configs
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, Column::Appearance(_) | Column::ComponentAppearance(_) | Column::FaceAppearance(_)))
            .map(|(i, _)| i)
            .collect();
        for row in &self.configs.rows {
            for cell in columns.iter().filter_map(|&i| row.cells.get(i)) {
                // Only a look written out as JSON can name a graph; library names and other
                // text are the configuration's own business.
                if cell.trim_start().starts_with('{')
                    && cell.contains("\"graph\"")
                    && let Ok(look) = serde_json::from_str::<Look>(cell)
                {
                    f(&look)?;
                }
            }
        }
        Ok(())
    }

    /// Check the design's material graphs and textures ([`GraphTables::check`]) and every look
    /// against them ([`GraphTables::check_look`]). Reading a design runs this; so must every
    /// command that changes the tables or looks, before it commits.
    pub fn check_appearance_graphs(&self) -> Result<()> {
        let tables = &self.appearances.tables;
        tables.check()?;
        self.for_each_look(&mut |look| tables.check_look(look))
    }

    /// The textures something references: the graphs (all of them: graphs stay until deleted)
    /// and the looks' values.
    pub fn referenced_textures(&self) -> BTreeSet<String> {
        let mut used: BTreeSet<String> = self.appearances.tables.graph_textures().map(str::to_string).collect();
        let _ = self.for_each_look(&mut |look| {
            used.extend(look_textures(look).map(str::to_string));
            Ok(())
        });
        used
    }

    /// The design as it is saved: without textures nothing references. Borrowed, so unchanged
    /// byte for byte, when every texture is used (always, for a design without textures).
    pub fn without_unreferenced_textures(&self) -> Cow<'_, Document> {
        if self.appearances.tables.textures.is_empty() {
            return Cow::Borrowed(self);
        }
        let used = self.referenced_textures();
        if self.appearances.tables.textures.keys().all(|key| used.contains(key)) {
            return Cow::Borrowed(self);
        }
        let mut doc = self.clone();
        doc.appearances.tables.textures.retain(|key, _| used.contains(key));
        Cow::Owned(doc)
    }
}

/// Whether a design's saved JSON has material graphs: what the format change
/// `appearance-graphs` adds (its `used` predicate, docs/material-graph.md §4.4).
pub fn graphs_used(v: &serde_json::Value) -> bool {
    v.pointer("/appearances/graphs").and_then(serde_json::Value::as_object).is_some_and(|g| !g.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_ids_are_bounded_and_plain() {
        assert!(GraphId::new("brushed-nickel-silver").is_ok());
        assert!(GraphId::new("pbr.1_a").is_ok());
        for bad in ["", "Upper", "with space", "../x", &"a".repeat(65)] {
            assert!(GraphId::new(bad).is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<GraphId>("\"Bad Id\"").is_err());
    }

    #[test]
    fn empty_tables_serialise_to_nothing() {
        assert_eq!(serde_json::to_value(GraphTables::default()).ok(), Some(serde_json::json!({})));
    }

    use serde_json::{Value, json};

    /// A real 2×1 RGBA PNG (original test data).
    pub(crate) const PNG_2X1: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0, 244, 34, 127, 138, 0, 0, 0, 14, 73, 68,
        65, 84, 120, 156, 99, 56, 161, 161, 241, 31, 132, 1, 18, 51, 4, 47, 169, 127, 204, 104, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    const ETCHED: &str = include_str!("../../render/tests/fixtures/material_graph/laser-etched-nickel-silver.json");
    const COPPER: &str = include_str!("../../render/tests/fixtures/material_graph/pbr-copper.json");

    /// Only the header: enough for the size checks, never decoded.
    fn png_header(width: u32, height: u32, depth: u8, colour: u8) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(width.to_be_bytes());
        png.extend(height.to_be_bytes());
        png.extend([depth, colour, 0, 0, 0]);
        png
    }

    fn id(s: &str) -> GraphId {
        GraphId::new(s).unwrap()
    }

    /// A design with two graphs (one textured, one unused by any look), a texture and a look.
    fn design() -> (Document, String) {
        let mut doc = Document::new("Graphs");
        let (key, texture) = TextureAsset::from_bytes("mark.png", PNG_2X1).unwrap();
        let tables = &mut doc.appearances.tables;
        tables.graphs.insert(id("etched"), serde_json::from_str(ETCHED).unwrap());
        tables.graphs.insert(id("copper"), serde_json::from_str(COPPER).unwrap());
        tables.textures.insert(key.clone(), texture);
        let mut look = Look::custom([200, 198, 190], 1.0);
        look.name = "Etched lid".into();
        look.graph = Some(id("etched"));
        look.values.insert("mark".into(), Constant::Texture(TextureRef { texture: key.clone() }));
        doc.appearances.bodies.insert("Can".into(), look);
        (doc, key)
    }

    #[test]
    fn designs_with_graphs_and_textures_round_trip() {
        let (doc, key) = design();
        doc.check_appearance_graphs().unwrap();
        let text = doc.to_json();
        let back = Document::from_json(&text).unwrap();
        assert_eq!(back.appearances, doc.appearances);
        assert_eq!(back.to_json(), text, "a second save writes the same bytes");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["appearances"]["bodies"]["Can"]["graph"], "etched");
        assert_eq!(v["appearances"]["bodies"]["Can"]["values"]["mark"], json!({ "texture": key }));
        assert_eq!(v["appearances"]["textures"][&key]["data"], crate::canvas::base64_encode(PNG_2X1));
        assert!(v["appearances"]["textures"][&key].get("image_blob").is_none());
        assert_eq!(v["appearances"]["graphs"]["copper"]["name"], "Copper - Clean PBR");
        assert!(graphs_used(&v));
        // Connection fallbacks are data: a texture one names counts as used, and round-trips.
        let mut fallback = doc.clone();
        let graph = fallback.appearances.tables.graphs.get_mut(&id("copper")).unwrap();
        let fb = json!({ "from": "noise", "fallback": { "texture": key } });
        graph.nodes[0].inputs.insert("base_color".into(), serde_json::from_value(fb.clone()).unwrap());
        fallback.appearances.bodies.clear();
        let text = fallback.to_json();
        let back = Document::from_json(&text).unwrap();
        assert_eq!(serde_json::to_value(&back.appearances.tables.graphs[&id("copper")].nodes[0].inputs["base_color"]).unwrap(), fb);
        assert!(back.referenced_textures().contains(&key));
    }

    #[test]
    fn designs_without_graphs_write_none_of_the_new_fields() {
        let mut doc = Document::new("Plain");
        doc.appearances.bodies.insert("A".into(), Look::custom([1, 2, 3], 0.5));
        doc.appearances.bodies.insert("B".into(), crate::appearance::library("Steel - Satin PBR").unwrap());
        let text = doc.to_json();
        for key in ["\"graphs\"", "\"textures\"", "\"graph\"", "\"values\""] {
            assert!(!text.contains(key), "{key} in {text}");
        }
        assert_eq!(serde_json::to_string(&Look::custom([1, 2, 3], 0.5)).unwrap(), r#"{"name":"Custom","color":[1,2,3],"opacity":0.5}"#);
        assert!(!graphs_used(&serde_json::from_str(&text).unwrap()));
        assert!(matches!(doc.without_unreferenced_textures(), Cow::Borrowed(_)));
        // Numeric component keys still read: the tables are not a serde flatten on reading.
        doc.appearances.components.insert(7, Look::custom([9, 9, 9], 1.0));
        assert_eq!(Document::from_json(&doc.to_json()).unwrap().appearances, doc.appearances);
    }

    #[test]
    fn graph_and_pbr_exclude_each_other_and_values_need_a_graph() {
        let both = r#"{"name":"x","color":[1,2,3],"pbr":{"metallic":1,"roughness":0.3,"dielectric_ior":1.5},"graph":"copper"}"#;
        assert!(serde_json::from_str::<Look>(both).is_err());
        assert!(serde_json::from_str::<Look>(&both.replace(r#","graph":"copper""#, "")).is_ok(), "the same pbr alone is fine");
        assert!(serde_json::from_str::<Look>(r#"{"name":"x","color":[1,2,3],"values":{"a":1}}"#).is_err());
        assert!(serde_json::from_str::<Look>(r#"{"name":"x","color":[1,2,3],"graph":"Bad Id"}"#).is_err());
        assert!(serde_json::from_str::<Look>(r#"{"name":"x","color":[1,2,3],"graph":"g","values":{"a":[1,2,3,4]}}"#).is_err());
        assert!(serde_json::from_str::<Look>(r#"{"name":"x","color":[1,2,3],"graph":"g","values":{"a":{"texture":"nothex"}}}"#).is_err());
        let ok = serde_json::from_str::<Look>(r#"{"name":"x","color":[1,2,3],"graph":"g","values":{"a":0.5,"b":{"srgb8":[1,2,3]}}}"#).unwrap();
        assert_eq!(ok.graph, Some(id("g")));
    }

    /// Every hostile change to a good design is refused on reading.
    #[test]
    fn hostile_designs_are_refused_on_reading() {
        let (doc, key) = design();
        let good: Value = serde_json::from_str(&doc.to_json()).unwrap();
        let other = "a".repeat(64);
        let header = |w, h| {
            let bytes = png_header(w, h, 8, 6);
            json!({ "name": "big", "width": w, "height": h, "format": "png", "data": crate::canvas::base64_encode(&bytes) })
        };
        let big = |w: u32, h: u32| (texture_key(&png_header(w, h, 8, 6)), header(w, h));
        let cases: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
            (
                "texture key is not its hash",
                Box::new(|v| {
                    let t = v["appearances"]["textures"].as_object_mut().unwrap();
                    let entry = t.remove(&key).unwrap();
                    t.insert(other.clone(), entry);
                }),
            ),
            (
                "texture key is not hex",
                Box::new(|v| {
                    let t = v["appearances"]["textures"].as_object_mut().unwrap();
                    let entry = t.remove(&key).unwrap();
                    t.insert("NOT-A-HASH".into(), entry);
                }),
            ),
            ("data is not base64", Box::new(|v| v["appearances"]["textures"][&key]["data"] = json!("!!!"))),
            ("width disagrees with the header", Box::new(|v| v["appearances"]["textures"][&key]["width"] = json!(3))),
            ("format disagrees with the header", Box::new(|v| v["appearances"]["textures"][&key]["format"] = json!("jpeg"))),
            ("unknown format", Box::new(|v| v["appearances"]["textures"][&key]["format"] = json!("gif"))),
            ("a blob reference in plain JSON", Box::new(|v| v["appearances"]["textures"][&key]["image_blob"] = json!(format!("blobs/{key}.png")))),
            ("unknown texture field", Box::new(|v| v["appearances"]["textures"][&key]["url"] = json!("http://x"))),
            ("a look names a missing graph", Box::new(|v| v["appearances"]["bodies"]["Can"]["graph"] = json!("missing"))),
            ("a look sets an input the graph lacks", Box::new(|v| v["appearances"]["bodies"]["Can"]["values"]["nope"] = json!(1))),
            ("a look names a missing texture", Box::new(|v| v["appearances"]["bodies"]["Can"]["values"]["mark"] = json!({ "texture": other }))),
            (
                "graph and pbr together",
                Box::new(|v| {
                    v["appearances"]["bodies"]["Can"]["pbr"] = serde_json::to_value(crate::pbr_appearance::PbrParameters::default()).unwrap();
                }),
            ),
            (
                "an interface default names a missing texture",
                Box::new(|v| {
                    v["appearances"]["graphs"]["etched"]["interface"][0]["default"] = json!({ "texture": other });
                }),
            ),
            (
                "a connection fallback names a missing texture",
                Box::new(|v| {
                    v["appearances"]["graphs"]["copper"]["nodes"][0]["inputs"]["base_color"] =
                        json!({ "from": "x", "fallback": { "texture": other } });
                }),
            ),
            ("an unknown graph field", Box::new(|v| v["appearances"]["graphs"]["copper"]["shader"] = json!("code"))),
            ("an empty graph name", Box::new(|v| v["appearances"]["graphs"]["copper"]["name"] = json!(""))),
            ("a layout far away", Box::new(|v| v["appearances"]["graphs"]["copper"]["layout"] = json!({ "surface": [1e7, 0] }))),
            (
                "a bad graph id",
                Box::new(|v| {
                    let g = v["appearances"]["graphs"].as_object_mut().unwrap();
                    let entry = g.remove("copper").unwrap();
                    g.insert("Copper Graph".into(), entry);
                }),
            ),
            (
                "too many nodes",
                Box::new(|v| {
                    let node = v["appearances"]["graphs"]["copper"]["nodes"][0].clone();
                    v["appearances"]["graphs"]["copper"]["nodes"] = Value::Array(vec![node; limits::NODES + 1]);
                }),
            ),
            (
                "too many graphs",
                Box::new(|v| {
                    let copper = v["appearances"]["graphs"]["copper"].clone();
                    let g = v["appearances"]["graphs"].as_object_mut().unwrap();
                    for i in 0..limits::GRAPHS_PER_DESIGN {
                        g.insert(format!("g{i}"), copper.clone());
                    }
                }),
            ),
            (
                "too many textures",
                Box::new(|v| {
                    let t = v["appearances"]["textures"].as_object_mut().unwrap();
                    for i in 0..limits::TEXTURES_PER_DESIGN {
                        t.insert(format!("{i:064x}"), json!({ "name": "t", "width": 1, "height": 1, "format": "png" }));
                    }
                }),
            ),
            (
                "a side over the limit",
                Box::new(|v| {
                    let (k, t) = big(limits::TEXTURE_SIDE + 1, 1);
                    v["appearances"]["textures"][k] = t;
                }),
            ),
            (
                "over the decoded budget",
                Box::new(|v| {
                    for w in [limits::TEXTURE_SIDE, limits::TEXTURE_SIDE - 1] {
                        let (k, t) = big(w, limits::TEXTURE_SIDE);
                        v["appearances"]["textures"][k] = t;
                    }
                }),
            ),
            (
                "a see-through graph on a translucent look",
                Box::new(|v| {
                    v["appearances"]["graphs"]["copper"]["nodes"][0]["inputs"]["geometry_opacity"] = json!(0.5);
                    v["appearances"]["bodies"]["Copper"] = json!({ "name": "c", "color": [1, 2, 3], "opacity": 0.5, "graph": "copper" });
                }),
            ),
            (
                "a material's look names a missing graph",
                Box::new(|v| {
                    v["user_materials"] =
                        json!([{ "name": "M", "density": 1.0, "appearance": { "name": "m", "color": [1, 2, 3], "graph": "missing" } }]);
                }),
            ),
            (
                "a configuration cell names a missing graph",
                Box::new(|v| {
                    v["configs"] = json!({
                        "columns": [{ "appearance": "Can" }],
                        "rows": [{ "name": "R", "cells": [r#"{"name":"m","color":[1,2,3],"graph":"missing"}"#] }]
                    });
                }),
            ),
        ];
        for (what, change) in cases {
            let mut bad = good.clone();
            change(&mut bad);
            assert!(Document::from_json(&bad.to_string()).is_err(), "{what} was read");
        }
        // The budget holds exactly one maximum-size image, and a large grey 16-bit data map
        // counts two bytes a pixel.
        let mut fits = good.clone();
        let (k, t) = big(limits::TEXTURE_SIDE, limits::TEXTURE_SIDE);
        fits["appearances"]["textures"][k] = t;
        fits["appearances"]["textures"].as_object_mut().unwrap().remove(&key);
        fits["appearances"]["bodies"].as_object_mut().unwrap().clear();
        Document::from_json(&fits.to_string()).unwrap();
        let grey = png_header(limits::TEXTURE_SIDE, limits::TEXTURE_SIDE / 2, 16, 0);
        let asset = TextureAsset {
            name: "height".into(),
            width: limits::TEXTURE_SIDE,
            height: limits::TEXTURE_SIDE / 2,
            format: "png".into(),
            image_blob: None,
            data: crate::canvas::base64_encode(&grey),
        };
        assert_eq!(asset.checked(&texture_key(&grey)).unwrap().1, (limits::TEXTURE_SIDE as usize).pow(2));
        // A translucent look on an opaque graph is the legacy blend, as today.
        let mut blend = good.clone();
        blend["appearances"]["bodies"]["Copper"] = json!({ "name": "c", "color": [1, 2, 3], "opacity": 0.5, "graph": "copper" });
        Document::from_json(&blend.to_string()).unwrap();
    }

    #[test]
    fn see_through_reads_constants_interface_values_and_connections() {
        let mut graph: MaterialGraph = serde_json::from_str(COPPER).unwrap();
        let mut look = Look::custom([1, 2, 3], 1.0);
        assert!(!see_through(&graph, &look));
        let set = |g: &mut MaterialGraph, input: &str, v: Value| {
            g.nodes[0].inputs.insert(input.into(), serde_json::from_value(v).unwrap());
        };
        set(&mut graph, "transmission_weight", json!(0.2));
        assert!(see_through(&graph, &look));
        set(&mut graph, "transmission_weight", json!(0));
        set(&mut graph, "geometry_opacity", json!({ "from": "mask" }));
        assert!(see_through(&graph, &look), "a driven opacity can be below 1");
        set(&mut graph, "geometry_opacity", json!({ "interface": "alpha" }));
        graph.interface = serde_json::from_value(json!([{ "name": "alpha", "type": "float", "default": 1.0 }])).unwrap();
        assert!(!see_through(&graph, &look));
        look.values.insert("alpha".into(), Constant::Number(0.4));
        assert!(see_through(&graph, &look));
    }

    #[test]
    fn unreferenced_textures_are_dropped_on_save_and_graphs_stay() {
        let (mut doc, key) = design();
        assert!(matches!(doc.without_unreferenced_textures(), Cow::Borrowed(_)), "every texture used: nothing to drop");
        let (spare, asset) = TextureAsset::from_bytes("spare.png", &png_header(4, 4, 8, 6)).unwrap();
        doc.appearances.tables.textures.insert(spare.clone(), asset);
        let saved = doc.without_unreferenced_textures();
        assert!(saved.appearances.tables.textures.contains_key(&key));
        assert!(!saved.appearances.tables.textures.contains_key(&spare));
        assert_eq!(saved.appearances.tables.graphs.len(), 2, "graphs stay until deleted, used or not");
        // A material's look keeps its texture too.
        let mut look = doc.appearances.bodies.remove("Can").unwrap();
        look.values.insert("mark".into(), Constant::Texture(TextureRef { texture: spare.clone() }));
        doc.user_materials.push(crate::UserMaterial { name: "M".into(), density: 1.0, appearance: Some(look), physical_card: None });
        let saved = doc.without_unreferenced_textures();
        assert_eq!(saved.appearances.tables.textures.keys().collect::<Vec<_>>(), vec![&spare]);
        Document::from_json(&saved.to_json()).unwrap();
    }
}
