//! Character data (module `character_data`, behind `modules.character_data`,
//! which needs `modules.roster`): NEXON Open API data linked to roster
//! characters by opt-in. Links are kanade's; snapshots are Nexon's, kept
//! at most 30 days, deleted at once on unlink, never in the change history,
//! settings audit values, logs, backups or a model prompt. Chat and
//! extraction never import this module (pinned below).

mod model;
mod port;
mod readiness;
mod rules;

pub use model::{
    ATTRIBUTION, CALL_WINDOW, DAILY_CALLS, FUTURE_SKEW, LINK_REGION, Link, LinkStatus,
    MAX_TEXT_CHARS, MAX_URL_CHARS, NOTICE_VERSION, OCID_RENEW_AFTER, PENDING_TTL, Post, PostState,
    REFRESH_AGE, RefreshState, SNAPSHOT_TTL, STRIP_AFTER, STRIP_RETRY_AGAIN, STRIP_RETRY_FIRST,
    Snapshot, date_text, parse_date,
};
pub use port::{CharDataStore, LinkPut, StoreFuture};
pub use readiness::{
    Level, LevelSource, LevelVerdict, Power, Readiness, Requirement, Subject, judge, readiness,
    usable_nexon,
};
pub use rules::{
    BATCH_TIME, Failure, MAINTENANCE_AGAIN, MAINTENANCE_FIRST, Orphan, PREPARING_RETRY,
    RATE_LIMIT_TRIES, UnlinkReason, has_departed, link_change, next_batch, orphan, pending_expired,
    rate_limit_delay, same_identity,
};

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn naming(dir: &Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                naming(&path, hits);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if [
                    "chardata",
                    "character_data",
                    "CharDataStore",
                    "CharacterData",
                    "nexon",
                    "Nexon",
                ]
                .iter()
                .any(|name| text.contains(name))
                {
                    hits.push(path.display().to_string());
                }
            }
        }
    }

    /// Nexon data never reaches a model: the chat and extraction code that
    /// builds prompts names none of the character-data paths.
    #[test]
    fn chat_and_extraction_never_read_character_data() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        for dir in ["chat", "extract"] {
            naming(&src.join(dir), &mut hits);
        }
        assert!(hits.is_empty(), "character data named in {hits:?}");
    }
}
