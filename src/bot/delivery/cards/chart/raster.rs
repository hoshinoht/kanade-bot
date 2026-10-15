//! SVG → PNG with resvg, at [`SCALE`]× and with the bundled fonts only
//! (Zilla Slab and Maple Mono, OFL-1.1, unmodified; licences beside them in
//! `fonts/`). Never reads a system font or a file at run time. CPU-bound:
//! callers run it on the blocking pool.

use std::sync::{Arc, LazyLock};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb};

use super::svg::Drawn;
use super::theme::SPOKEN;

/// Device pixels per CSS px: the 560 px drawing is a 1120 px image.
pub const SCALE: u32 = 2;

const FONTS: [&[u8]; 4] = [
    include_bytes!("fonts/ZillaSlab-Regular.ttf"),
    include_bytes!("fonts/ZillaSlab-Bold.ttf"),
    include_bytes!("fonts/MapleMono-Regular.ttf"),
    include_bytes!("fonts/MapleMono-Bold.ttf"),
];

/// The bundled fonts, parsed once per process (shared, never copied).
static FONT_DB: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    for bytes in FONTS {
        db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
    }
    Arc::new(db)
});

/// Why no PNG came out.
#[derive(Debug, thiserror::Error)]
pub enum RasterError {
    #[error("svg: {0}")]
    Svg(#[from] usvg::Error),
    #[error("size {0}x{1}")]
    Size(u32, u32),
    #[error("png: {0}")]
    Png(String),
}

/// `drawn` as a PNG at [`SCALE`]×.
pub fn png(drawn: &Drawn) -> Result<Vec<u8>, RasterError> {
    let options = usvg::Options {
        font_family: SPOKEN.to_owned(),
        fontdb: Arc::clone(&FONT_DB),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&drawn.svg, &options)?;
    let (width, height) = (drawn.width * SCALE, drawn.height * SCALE);
    let mut pixmap = Pixmap::new(width, height).ok_or(RasterError::Size(width, height))?;
    let scale = SCALE as f32;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|error| RasterError::Png(error.to_string()))
}
