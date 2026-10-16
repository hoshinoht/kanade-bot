//! Content hashes: SHA-256 over a canonical JSON form whose field order is
//! fixed by the structs below (never by map iteration). Any change to these
//! forms marks every published slot "Out of date" once, so keep them stable.

use serde::Serialize;

use super::plan::{Confirmations, PlanOp};
use super::render::{GuideEmbed, Upload};
pub use crate::domain::history::sha256_hex;

/// Every create is sent with Discord's `SILENT` flag (decision 15).
const FLAGS: &str = "silent";

#[derive(Serialize)]
struct SlotForm<'a> {
    content: &'a str,
    embeds: &'a [GuideEmbed],
    flags: &'static str,
    uploads: Vec<[&'a str; 2]>,
}

/// The hash of one rendered message: content, embeds, flags and the
/// uploads' names and digests (bytes enter only through their digest).
pub fn slot_hash(content: &str, embeds: &[GuideEmbed], uploads: &[Upload]) -> String {
    let form = SlotForm {
        content,
        embeds,
        flags: FLAGS,
        uploads: uploads
            .iter()
            .map(|upload| [upload.file_name.as_str(), upload.sha256.as_str()])
            .collect(),
    };
    sha256_hex(&serde_json::to_vec(&form).unwrap_or_default())
}

#[derive(Serialize)]
struct OpForm<'a> {
    kind: &'static str,
    position: u32,
    channel: &'a str,
    message_id: Option<&'a str>,
    hash: Option<&'a str>,
}

#[derive(Serialize)]
struct PlanForm<'a> {
    channel: &'a str,
    ops: Vec<OpForm<'a>>,
    confirm: &'a Confirmations,
}

/// The identity of a plan: its target channel, every op in order and the
/// confirmations it requires. A publish whose recomputed plan hashes
/// differently stops `plan_changed` before sending anything.
pub fn plan_hash(channel_id: &str, ops: &[PlanOp], needs: &Confirmations) -> String {
    let form = PlanForm {
        channel: channel_id,
        ops: ops
            .iter()
            .map(|op| OpForm {
                kind: op.kind().as_str(),
                position: op.position(),
                channel: op.channel_id(),
                message_id: op.message_id(),
                hash: op.hash(),
            })
            .collect(),
        confirm: needs,
    };
    sha256_hex(&serde_json::to_vec(&form).unwrap_or_default())
}
