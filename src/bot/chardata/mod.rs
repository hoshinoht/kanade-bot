//! Character data in Discord (module `character_data`): the approved copy
//! of `/character link|unlink|data|share` and its Components V2 layouts
//! (`copy.rs`), and the share cards' Discord side (`posts.rs`): posting,
//! finding a card by its ref and stripping it to the stub for the
//! `CharacterData` task. The command itself is `commands::character`.

pub mod copy;
mod posts;

pub use posts::{DiscordPosts, SelfId, Sent};
