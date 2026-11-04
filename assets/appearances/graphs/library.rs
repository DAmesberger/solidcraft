//! The built-in material library (docs/render-material-graphs.md): material graphs, each offered
//! as one look. Rows: the graph's id, its look's standard-view colour (sRGB8, the graph's base
//! response, checked by a test) and the graph's JSON. Values are original visual estimates, not
//! measured optical data.
pub const GRAPHS: &[(&str, [u8; 3], &str)] = &[
    ("black-epoxy-clear-coat", [41, 41, 43], include_str!("black-epoxy-clear-coat.json")),
    ("brushed-nickel-silver", [231, 227, 218], include_str!("brushed-nickel-silver.json")),
    ("coated-woven-laminate", [173, 162, 115], include_str!("coated-woven-laminate.json")),
    ("laser-etched-nickel-silver", [231, 227, 218], include_str!("laser-etched-nickel-silver.json")),
    ("orange-peel-gloss", [203, 63, 56], include_str!("orange-peel-gloss.json")),
    ("printed-ink", [239, 239, 236], include_str!("printed-ink.json")),
    ("printed-sleeve", [39, 60, 111], include_str!("printed-sleeve.json")),
    ("tinted-coat-over-copper", [250, 209, 194], include_str!("tinted-coat-over-copper.json")),
    ("woven-laminate", [173, 162, 115], include_str!("woven-laminate.json")),
];
