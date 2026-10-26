//! Startup loaders for tracked and operator files: the boss catalog and boss
//! knowledge documents; boss art for cards and the files guides render from.

mod art;
mod catalog;
mod error;
mod guide_sources;
mod knowledge;
mod read;

pub use art::BossArt;
pub use catalog::load_catalog;
pub use error::LoadError;
pub use guide_sources::GuideFiles;
pub(crate) use knowledge::read_document;
pub use knowledge::{KnowledgeDir, KnowledgeEvent, load_knowledge_dir};
pub(crate) use read::{Refusal, open_regular, read_capped};

#[cfg(test)]
mod tests;
