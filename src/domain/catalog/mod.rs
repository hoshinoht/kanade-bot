//! Boss catalog validation and boss-name parsing.
//!
//! Tokens are a difficulty letter plus a boss alias (`nstar`, `HFA`, `xkalos`);
//! the stored canonical form is the uppercased letter plus the boss's short name
//! (`HStar`). Alias matching ignores spacing and punctuation, and a spelled-out
//! difficulty word (`Hard Gatekeeper Kalos`) folds into the prefixed form.
//!
//! A missing difficulty and a difficulty the boss lacks are refused rather than
//! guessed, listing the forms that would have worked. Loading the YAML file is an
//! infrastructure concern; this module validates already-decoded [`CatalogSpec`]s.

mod build;
mod error;
mod model;
mod parse;
mod reference;
mod spec;
mod table;
mod text;

pub use error::{BossParseError, BossTableError};
pub use model::{Boss, BossDetail, BossReference, Difficulty};
pub use spec::{BossSpec, CatalogSpec, DifficultySpec, GuideSpec};
pub use table::BossTable;
