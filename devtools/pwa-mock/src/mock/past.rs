//! The Inbox's Past tab (admin-api "Inbox (A6)", `GET /api/admin/inbox/past`):
//! invented closed proposals and member requests covering every outcome,
//! newest closed first, paged by the last id shown.

use super::clock::iso_z;
use super::dto::Named;
use super::inbox::Evidence;
use super::seed;
use super::{MoveError, Store};
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Decider {
    pub kind: &'static str,
    pub id: &'static str,
    pub name: String,
}

#[derive(Clone, Serialize)]
pub struct PastItem {
    pub id: &'static str,
    pub short_id: &'static str,
    pub kind: &'static str,
    pub kind_label: &'static str,
    pub tab: &'static str,
    pub source: &'static str,
    pub source_id: Option<&'static str>,
    pub summary: &'static str,
    pub channel: Option<&'static str>,
    pub requester: Option<Named>,
    pub outcome: &'static str,
    pub decided_by: Option<Decider>,
    pub decided_at: String,
    pub reason: Option<&'static str>,
    pub created_at: String,
    pub history_seq: Option<u64>,
    pub evidence: Vec<Evidence>,
    pub card_url: Option<String>,
}

#[derive(Serialize)]
pub struct PastPage {
    pub items: Vec<PastItem>,
    pub next_before: Option<String>,
}

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;

/// A seeded closed item; hours count from this boss week's start.
struct Closed {
    id: &'static str,
    kind: &'static str,
    /// `extraction` or `chat` for proposals; `None` for member requests.
    source: Option<&'static str>,
    summary: &'static str,
    channel: Option<&'static str>,
    requester: Option<&'static str>,
    outcome: &'static str,
    /// `(kind, id)` as the server's history actors.
    decider: (&'static str, &'static str),
    reason: Option<&'static str>,
    created_hour: i64,
    closed_hour: i64,
    history_seq: Option<u64>,
    /// `(author, text, hour)`; empty text is a message no longer cached.
    evidence: Vec<(&'static str, &'static str, i64)>,
    card_posted: bool,
}

fn seeded() -> Vec<Closed> {
    let proposal = |id, kind, source, summary, channel| Closed {
        id,
        kind,
        source: Some(source),
        summary,
        channel: Some(channel),
        requester: None,
        outcome: "approved",
        decider: ("system", "delivery"),
        reason: None,
        created_hour: 0,
        closed_hour: 0,
        history_seq: None,
        evidence: Vec::new(),
        card_posted: true,
    };
    let request = |id, kind, member, summary| Closed {
        source: None,
        channel: None,
        requester: Some(member),
        card_posted: false,
        ..proposal(id, kind, "extraction", summary, "")
    };
    vec![
        // The seeded History record 3 (Asahi's approval of Kalos to 22:00).
        Closed {
            decider: ("member", "1001"),
            created_hour: 39,
            closed_hour: 41,
            history_seq: Some(3),
            evidence: vec![
                ("1005", "kalos at 10 instead? 9:30 is tight", 38),
                ("1003", "", 38),
            ],
            ..proposal(
                "c-kalos-move",
                "move",
                "extraction",
                "Tsubame asks to start Kalos at 22:00",
                "kalos-four",
            )
        },
        Closed {
            outcome: "rejected",
            decider: ("member", "1003"),
            created_hour: 30,
            closed_hour: 33,
            evidence: vec![("1003", "can we add a lotus run sat?", 29)],
            ..proposal(
                "c-lotus-add",
                "add",
                "chat",
                "Mika asks the chatbot for a Saturday Lotus run",
                "hstar-party",
            )
        },
        Closed {
            outcome: "superseded",
            decider: ("system", "extraction"),
            created_hour: 26,
            closed_hour: 28,
            evidence: vec![("1006", "limbo 9pm?", 25)],
            ..proposal(
                "c-limbo-move",
                "move",
                "extraction",
                "Hinata suggests Limbo at 21:00",
                "limbo-trio",
            )
        },
        Closed {
            outcome: "expired",
            created_hour: 2,
            closed_hour: 26,
            card_posted: false,
            ..proposal(
                "c-seren-cancel",
                "cancel",
                "extraction",
                "Seren might be off this week",
                "seren-trio",
            )
        },
        Closed {
            outcome: "discarded",
            decider: ("admin", "token"),
            reason: Some("Duplicate of the Limbo move."),
            created_hour: 20,
            closed_hour: 24,
            ..proposal(
                "c-bellona-otot",
                "otot",
                "extraction",
                "Bellona as own time",
                "bellona-otot",
            )
        },
        Closed {
            outcome: "withdrawn",
            decider: ("member", "1002"),
            created_hour: 10,
            closed_hour: 14,
            ..request("c-ren-leave", "leave", "1002", "busy on Tuesday")
        },
        Closed {
            outcome: "rejected",
            decider: ("admin", "tailscale:ops"),
            reason: Some("One weekly run per party."),
            created_hour: 4,
            closed_hour: 9,
            ..request(
                "c-yuzu-fixed",
                "new_fixed",
                "1004",
                "weekly star on Saturdays",
            )
        },
        Closed {
            outcome: "expired",
            created_hour: -100,
            closed_hour: -2,
            ..request("c-sora-swap", "swap", "1008", "Nagi takes my spot")
        },
        Closed {
            outcome: "rejected",
            decider: ("admin", "discord:1003"),
            reason: Some("Wednesday clashes with Kalos."),
            created_hour: -60,
            closed_hour: -40,
            ..request("c-tsu-change", "change_fixed", "1005", "Wednesdays suit us")
        },
        Closed {
            outcome: "rejected",
            decider: ("admin", "discord:9999"),
            reason: Some("Asked twice."),
            created_hour: -70,
            closed_hour: -50,
            ..request("c-nagi-join", "join", "1007", "can I come to Kalos?")
        },
    ]
}

fn named(id: &str) -> Named {
    seed::member_name(id).map_or_else(
        || Named {
            id: id.into(),
            name: format!("user {id}"),
        },
        |(id, name)| Named {
            id: id.into(),
            name: name.into(),
        },
    )
}

/// As the server (and the History page) names actors.
fn decider(kind: &'static str, id: &'static str) -> Decider {
    let name = match (kind, id.split_once(':')) {
        ("member", _) => named(id).name,
        ("admin", _) if id == "token" => "Admin (token)".into(),
        ("admin", Some(("discord", user))) => seed::member_name(user)
            .map_or_else(|| format!("Admin (Discord {user})"), |m| m.1.into()),
        ("admin", Some(("tailscale", login))) => format!("Admin ({login})"),
        ("admin", _) => format!("Admin ({id})"),
        _ => "Kanade".into(),
    };
    Decider { kind, id, name }
}

impl Store {
    fn past_iso(hour: i64) -> String {
        iso_z(Self::start(false) * 1440 + hour * 60)
    }

    fn past_items() -> Vec<PastItem> {
        let mut closed = seeded();
        closed.sort_by(|a, b| (b.closed_hour, b.id).cmp(&(a.closed_hour, a.id)));
        closed
            .into_iter()
            .map(|c| {
                let short_id = &c.id[2..];
                let channel = c.channel.and_then(seed::channel);
                PastItem {
                    id: c.id,
                    short_id,
                    kind: c.kind,
                    kind_label: super::inbox::label(c.kind),
                    tab: if c.source.is_some() {
                        "extractor"
                    } else {
                        "self_service"
                    },
                    source: c.source.unwrap_or("self_service"),
                    source_id: c.source.map(|_| short_id),
                    summary: c.summary,
                    channel: channel.map(|c| c.1),
                    requester: c.requester.map(named),
                    outcome: c.outcome,
                    decided_by: Some(decider(c.decider.0, c.decider.1)),
                    decided_at: Self::past_iso(c.closed_hour),
                    reason: c.reason,
                    created_at: Self::past_iso(c.created_hour),
                    history_seq: c.history_seq,
                    evidence: c
                        .evidence
                        .iter()
                        .enumerate()
                        .map(|(i, &(who, text, hour))| {
                            let id = format!("{short_id}-{i}");
                            Evidence {
                                author: if text.is_empty() {
                                    "someone".into()
                                } else {
                                    named(who).name
                                },
                                author_id: (!text.is_empty()).then(|| who.to_owned()),
                                at: Self::when(Self::start(false) * 1440 + hour * 60),
                                content: (!text.is_empty()).then(|| text.to_owned()),
                                // Closed items keep the link even once uncached.
                                url: Some(format!("https://discord.com/channels/0/0/{id}")),
                                missing: text.is_empty(),
                                id,
                            }
                        })
                        .collect(),
                    card_url: c
                        .card_posted
                        .then(|| format!("https://discord.com/channels/0/0/card-{short_id}")),
                }
            })
            .collect()
    }

    /// One page after `before` (the last id shown); an unknown id is refused.
    pub fn inbox_past(&self, before: Option<&str>, limit: usize) -> Result<PastPage, MoveError> {
        let all = Self::past_items();
        let start = match before {
            None => 0,
            Some(id) => {
                all.iter().position(|item| item.id == id).ok_or_else(|| {
                    MoveError::Coded(422, "invalid_query", "No such closed item.".into())
                })? + 1
            }
        };
        let end = all.len().min(start + limit);
        let next_before = (end < all.len()).then(|| all[end - 1].id.to_owned());
        Ok(PastPage {
            items: all[start..end].to_vec(),
            next_before,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::mock::catalog::Catalog;

    fn store() -> crate::mock::Store {
        crate::mock::Store::new(Catalog::new(std::path::PathBuf::from(
            "../../web/e2e/fixtures/boss",
        )))
    }

    fn page(s: &crate::mock::Store, before: Option<&str>, limit: usize) -> super::PastPage {
        s.inbox_past(before, limit)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn every_outcome_and_both_kinds_newest_first() {
        let s = store();
        let page = page(&s, None, super::MAX_LIMIT);
        assert_eq!(page.next_before, None);
        let outcomes: std::collections::BTreeSet<&str> =
            page.items.iter().map(|i| i.outcome).collect();
        assert_eq!(
            outcomes.into_iter().collect::<Vec<_>>(),
            [
                "approved",
                "discarded",
                "expired",
                "rejected",
                "superseded",
                "withdrawn"
            ]
        );
        for tab in ["extractor", "self_service"] {
            assert!(page.items.iter().any(|i| i.tab == tab), "{tab}");
        }
        let at: Vec<&str> = page.items.iter().map(|i| i.decided_at.as_str()).collect();
        assert!(at.windows(2).all(|w| w[0] >= w[1]), "{at:?}");
        let approved = page.items.iter().find(|i| i.outcome == "approved").unwrap();
        assert_eq!(approved.history_seq, Some(3));
        assert!(
            s.record(3).is_some(),
            "the History link names a seeded record"
        );
        assert!(
            page.items
                .iter()
                .filter(|i| i.outcome != "approved")
                .all(|i| i.history_seq.is_none())
        );
    }

    #[test]
    fn pages_follow_the_last_id_shown() {
        let s = store();
        let all = page(&s, None, super::MAX_LIMIT).items;
        let mut seen = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let page = page(&s, before.as_deref(), 3);
            seen.extend(page.items.iter().map(|i| i.id));
            match page.next_before {
                Some(id) => before = Some(id),
                None => break,
            }
        }
        assert_eq!(seen, all.iter().map(|i| i.id).collect::<Vec<_>>());
        assert!(s.inbox_past(Some("nope"), 3).is_err());
    }
}
