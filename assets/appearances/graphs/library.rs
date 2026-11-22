//! The built-in material library (docs/render-material-graphs.md): material graphs, each offered
//! as one look. Rows: the graph's id, its look's standard-view colour (sRGB8, the graph's base
//! response, checked by a test) and the graph's JSON. Values are original visual estimates, not
//! measured optical data.
pub const GRAPHS: &[(&str, [u8; 3], &str)] = &[
    ("brushed-stainless", [231, 227, 218], include_str!("brushed-stainless.json")),
    ("candy-coat", [245, 246, 246], include_str!("candy-coat.json")),
    ("clear-glass", [247, 252, 249], include_str!("clear-glass.json")),
    ("coated-glass-fibre-weave", [173, 162, 115], include_str!("coated-glass-fibre-weave.json")),
    ("glass-fibre-weave", [173, 162, 115], include_str!("glass-fibre-weave.json")),
    ("gloss-black-clear-coat", [41, 41, 43], include_str!("gloss-black-clear-coat.json")),
    ("laser-etched-stainless", [231, 227, 218], include_str!("laser-etched-stainless.json")),
    ("matte-rubber", [48, 48, 48], include_str!("matte-rubber.json")),
    ("oak-wood", [194, 162, 121], include_str!("oak-wood.json")),
    ("orange-peel-gloss", [203, 63, 56], include_str!("orange-peel-gloss.json")),
    ("printed-ink", [239, 239, 236], include_str!("printed-ink.json")),
    ("printed-label", [39, 60, 111], include_str!("printed-label.json")),
    ("textured-abs", [63, 63, 66], include_str!("textured-abs.json")),
];
