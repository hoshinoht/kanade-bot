//! The member's own characters on the portal (modules `roster` and
//! `character_data`): the roster, the NEXON notice and link outcomes, what
//! is kept (My data), and the party's characters on one of the member's
//! runs. No doc comments on the structs (see `public.rs`); `public.json`
//! documents each field.

use std::collections::BTreeMap;

use serde::Serialize;

use super::super::{
    character_data::{CharacterFacts, CharacterLink},
    characters::CharacterRow,
    history::SettingsActor,
    readiness::ScouterReading,
};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MyCharacters {
    pub characters: Vec<CharacterRow>,
    pub links: BTreeMap<String, CharacterLink>,
    pub pending: Vec<PendingLink>,
    pub character_data: bool,
    pub connected: bool,
    pub estimates: bool,
    pub notice_version: u32,
    pub privacy_url: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PendingLink {
    pub character_id: String,
    pub ign: String,
    pub class: Option<String>,
    pub requested_by: SettingsActor,
    pub requested_by_name: Option<String>,
    pub requested_at: String,
    pub expires_at: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MyCharacterCreated {
    #[serde(flatten)]
    pub row: CharacterRow,
    pub linkable: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct NoticeSection {
    pub heading: &'static str,
    pub text: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LinkNotice {
    pub character_id: String,
    pub ign: String,
    pub version: u32,
    pub title: String,
    pub lead: String,
    pub what: NoticeSection,
    pub why: NoticeSection,
    pub how_long: NoticeSection,
    pub who: NoticeSection,
    pub as_is: &'static str,
    pub link_label: String,
    pub cancel_label: &'static str,
    pub request: Option<PendingLink>,
    pub privacy_url: Option<String>,
    pub attribution: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LinkOutcome {
    #[cfg_attr(
        test,
        ts(
            type = "'linked' | 'linked_later' | 'busy' | 'unavailable' | 'not_found' | 'other_region'"
        )
    )]
    pub state: &'static str,
    pub link: Option<CharacterLink>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LinkCancelled {
    pub request_kept: bool,
    pub requested_by_name: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Unlinked {
    pub unlinked: String,
    pub cards: u32,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct UnlinkedAll {
    pub unlinked: Vec<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SharedCard {
    pub channel: Option<String>,
    pub posted_at: String,
    pub cleared_by: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MyDataEntry {
    pub character_id: String,
    pub ign: String,
    #[cfg_attr(
        test,
        ts(type = "'pending' | 'linked' | 'stale' | 'expired' | 'not_linked'")
    )]
    pub state: &'static str,
    pub retired: bool,
    pub data: Option<CharacterFacts>,
    pub requested_by: Option<SettingsActor>,
    pub requested_by_name: Option<String>,
    pub requested_at: Option<String>,
    pub request_expires_at: Option<String>,
    pub linked_at: Option<String>,
    pub notice: Option<u32>,
    pub consented_at: Option<String>,
    #[cfg_attr(test, ts(type = "'discord' | 'portal' | null"))]
    pub consent_surface: Option<&'static str>,
    pub cards: Vec<SharedCard>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MyData {
    pub entries: Vec<MyDataEntry>,
    pub notice_version: u32,
    pub privacy_url: Option<String>,
    pub attribution: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberEstimateLimit {
    #[cfg_attr(test, ts(type = "'member_limited'"))]
    pub error: &'static str,
    pub message: String,
    pub next_at: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyMember {
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyCharacter {
    pub ign: String,
    pub class: Option<String>,
    pub level: Option<u16>,
    #[cfg_attr(test, ts(type = "'roster' | 'nexon'"))]
    pub level_source: &'static str,
    pub as_of: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyEstimate {
    pub boss: String,
    #[cfg_attr(test, ts(type = "Difficulty"))]
    pub difficulty: String,
    #[serde(flatten)]
    pub reading: ScouterReading,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyCharacterSlot {
    pub member: PartyMember,
    pub you: bool,
    pub character: Option<PartyCharacter>,
    pub scouter: Option<Vec<PartyEstimate>>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyTotal {
    pub boss: String,
    #[cfg_attr(test, ts(type = "Difficulty"))]
    pub difficulty: String,
    pub combined_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub technically_possible: Option<bool>,
    pub complete: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PartyTotals {
    pub fetched_at: String,
    pub stale: bool,
    pub bosses: Vec<PartyTotal>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunPartyCharacters {
    pub run_id: String,
    pub slots: Vec<PartyCharacterSlot>,
    pub character_data: bool,
    pub estimates: bool,
    pub fresh_days: Option<u32>,
    pub totals: Option<PartyTotals>,
    pub nexon_data: bool,
    pub attribution: &'static str,
    pub estimate_label: &'static str,
    pub estimate_note: &'static str,
}
