//! Character data in Discord (module `character_data`): the share cards'
//! Discord side (`posts.rs`): posting, finding a card by its ref and
//! stripping it to the stub for the `CharacterData` task. The approved copy
//! and its Components V2 layouts are `crate::cards::chardata`; the command
//! itself is `commands::character`.

mod posts;

pub use posts::{DiscordPosts, SelfId, Sent};
