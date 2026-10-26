//! The chart's fixed colours, from the approved board
//! (`docs/research/2026-10-15-estimate-share-chart`): Discord's dark embed
//! grey as an opaque ground, and the six member colours (Okabe–Ito without
//! the yellow, the dark face), each with the ink drawn on it.

/// Ground: a step off Discord's dark message background, opaque.
pub const GROUND: &str = "#2b2d31";
/// Body text.
pub const INK: &str = "#dbdee1";
/// Headings, IGNs and boss tokens.
pub const STRONG: &str = "#f2f3f5";
/// Secondary text and the no-estimate swatch.
pub const DIM: &str = "#b5bac1";
/// The dashed outline of a bar nobody has an estimate for.
pub const LINE: &str = "#4e5058";
/// Rules between runs and above the footer.
pub const RULE: &str = "#3f4147";

/// `(fill, ink)` per member number, in party order.
pub const WHO: [(&str, &str); 6] = [
    ("#3a9ad9", "#00182b"),
    ("#e69f00", "#2a1d00"),
    ("#14b386", "#00241a"),
    ("#d98bb6", "#2b0f20"),
    ("#7cc6f0", "#0b2638"),
    ("#e36d14", "#2b1300"),
];

/// Member `index`'s colours; a seventh member reuses the first (parties
/// are looked up six at most, and the number still tells them apart).
pub fn who(index: usize) -> (&'static str, &'static str) {
    WHO[index % WHO.len()]
}

/// The families named in the SVG, loaded from the bundled fonts.
pub const SPOKEN: &str = "Zilla Slab";
pub const COUNTED: &str = "Maple Mono";
