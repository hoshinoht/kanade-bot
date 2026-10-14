//! Discord snowflakes as the domain's text ids.

use twilight_model::id::Id;

/// A canonical nonzero decimal snowflake; padded, signed, zero or non-numeric
/// text is refused so aliases cannot name a second identity.
pub fn parse_id<M>(text: &str) -> Option<Id<M>> {
    let canonical =
        !text.is_empty() && !text.starts_with('0') && text.bytes().all(|b| b.is_ascii_digit());
    if !canonical {
        return None;
    }
    text.parse::<u64>().ok().and_then(Id::new_checked)
}

/// The domain's text form of a snowflake.
pub fn id_text<M>(id: Id<M>) -> String {
    id.get().to_string()
}
