//! Built-in procedural finishes. Each file defines its own name, inputs and surface.
pub(crate) const GRAPHS: &[(&str, &str)] = &[
    ("anodised-aluminium", include_str!("anodised-aluminium.json")),
    ("clear-glass", include_str!("clear-glass.json")),
    ("matte-rubber", include_str!("matte-rubber.json")),
    ("oak-wood", include_str!("oak-wood.json")),
    ("orange-peel-gloss", include_str!("orange-peel-gloss.json")),
    ("textured-abs", include_str!("textured-abs.json")),
];
