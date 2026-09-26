//! Startup loaders for tracked and operator files: the boss catalog, boss
//! knowledge documents and the persona layout; boss art for cards.

mod art;
mod catalog;
mod error;
mod knowledge;
mod personas;
mod read;

pub use art::BossArt;
pub use catalog::load_catalog;
pub use error::LoadError;
pub(crate) use knowledge::read_document;
pub use knowledge::{KnowledgeDir, load_knowledge_dir};
pub use personas::{PersonaLoad, load_personas};

#[cfg(test)]
mod tests;
