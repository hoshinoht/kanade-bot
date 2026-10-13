//! Every word `/character` says, as the approved copy boards draw it
//! (`D_LinkPick`, `D_LinkNotice`, `D_LinkReplies`, `D_Data`, `D_Share`,
//! `D_ShareStripped`), and the Components V2 layouts that carry it. Cards
//! showing NEXON data end with [`ATTRIBUTION`]; the stub carries no data and
//! no attribution. Mentions never notify (replies are ephemeral or sent
//! with no allowed mentions).

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Asia::Singapore;
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::ButtonStyle;

use crate::bot::delivery::cards::redesign::{
    action_row, button, container, section_with_thumbnail, text, within_budget,
};
use crate::domain::chardata::{ATTRIBUTION, NOTICE_VERSION, Snapshot};

/// The blossom edge of cards with data; the stub's grey one.
const BLOSSOM: u32 = 0xa3_4e_68;
const GREY: u32 = 0x4e_50_58;

pub const STUB: &str = "Character data removed (expired or unlinked).";
pub const NOT_CONNECTED: &str = "Character data isn’t connected yet. Nothing was fetched or kept.";
pub const BUSY: &str =
    "Kanade has used today’s NEXON lookups. Try again tomorrow; nothing was linked.";
pub const UNAVAILABLE: &str =
    "NEXON isn’t answering right now. Try again later; nothing was linked.";
pub const OTHER_REGION: &str = "Only MapleSEA characters can be linked.";
pub const EXPIRED: &str = "This notice expired. Run /character link again.";
pub const NOTHING_LINKED: &str = "Nothing was linked.";
pub const NOTHING_KEPT: &str = "Nothing from NEXON is kept for you.";
/// The public privacy page, on the public origin.
pub const PRIVACY_PATH: &str = "/privacy";
/// `/character unlink character:` value that unlinks everything (“All”).
pub const ALL: &str = "all";

/// Markdown-inert text (names come from Discord, the roster or NEXON).
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '*' | '_' | '~' | '`' | '|' | '>' | '#' | '[' | ']' | '(' | ')' | '<'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn bold(text: &str) -> String {
    format!("**{}**", plain(text))
}

/// `48,203,114`.
pub fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `29 Sep 2026 (SGT)`: the response's reference date, else the fetch
/// time, in Singapore time (the Open API's zone).
pub fn as_of(snapshot: &Snapshot) -> String {
    let date = snapshot
        .data_date
        .unwrap_or_else(|| snapshot.fetched_at.with_timezone(&Singapore).date_naive());
    format!(
        "{} {} {} (SGT)",
        date.day(),
        MONTHS[date.month0() as usize],
        date.year()
    )
}

/// A Discord timestamp: each reader sees it in their own time zone.
/// `f` date and time, `D` date.
fn stamp(at: DateTime<Utc>, style: char) -> String {
    format!("<t:{}:{style}>", at.timestamp())
}

fn channel(channel_id: &str) -> String {
    format!("<#{channel_id}>")
}

/// `**Asahi**`, or "An admin" when the requester has no name here.
fn who(requester: Option<&str>) -> String {
    requester.map_or_else(|| "An admin".to_owned(), bold)
}

fn privacy(url: Option<&str>) -> String {
    url.map(|url| format!(" [Privacy notice]({url})"))
        .unwrap_or_default()
}

fn footer(extra: Option<&str>) -> Component {
    text(match extra {
        Some(extra) => format!("-# {ATTRIBUTION} · {extra}"),
        None => format!("-# {ATTRIBUTION}"),
    })
}

// ── /character link ──

/// The autocomplete label of a pending request.
pub fn pick_pending(ign: &str, class: Option<&str>, requester: Option<&str>) -> String {
    let by = requester.unwrap_or("an admin");
    match class {
        Some(class) => format!("{ign} · {class} · requested by {by} · waiting for you"),
        None => format!("{ign} · requested by {by} · waiting for you"),
    }
}

/// The autocomplete label of another character, with its state
/// (`not linked`, `linked`, `stale, link again`).
pub fn pick_other(ign: &str, class: Option<&str>, state: &str) -> String {
    match class {
        Some(class) => format!("{ign} · {class} · {state}"),
        None => format!("{ign} · {state}"),
    }
}

pub fn not_on_roster(typed: &str) -> String {
    let typed: String = typed.trim().chars().take(40).collect();
    format!(
        "{} isn’t one of your characters on the roster. An admin adds characters on Members.",
        plain(&typed)
    )
}

pub fn retired(ign: &str) -> String {
    format!(
        "{} is retired on the roster, so it can’t be linked.",
        bold(ign)
    )
}

pub fn already_linked(ign: &str) -> String {
    format!(
        "{} is already linked. See it with `/character data`.",
        bold(ign)
    )
}

pub fn not_found(ign: &str) -> String {
    format!(
        "NEXON couldn’t find {} in MapleSEA. Check the IGN on the roster with an admin, then try again. Nothing was kept.",
        bold(ign)
    )
}

/// Cancel with a pending admin request (`requester`: its admin's name).
pub fn cancelled(ign: &str, pending: bool, requester: Option<&str>) -> String {
    if !pending {
        return NOTHING_LINKED.to_owned();
    }
    let whose = requester.map_or_else(
        || "An admin’s".to_owned(),
        |name| format!("{}’s", plain(name)),
    );
    format!(
        "Nothing was linked. {whose} request for {} is still waiting; run `/character link` when you’re ready.",
        bold(ign)
    )
}

pub fn linked_later(ign: &str) -> String {
    format!(
        "Linked {}. NEXON didn’t answer just now, so nothing is kept yet; Kanade tries again later.",
        bold(ign)
    )
}

/// The ephemeral notice (version [`NOTICE_VERSION`]) with Link / Cancel.
/// `request`: an admin's pending request, with its admin's name if known.
pub fn notice(
    ign: &str,
    request: Option<Option<&str>>,
    privacy_url: Option<&str>,
    link_id: String,
    cancel_id: String,
) -> Vec<Component> {
    debug_assert_eq!(NOTICE_VERSION, 1, "a new notice needs new copy");
    let name = plain(ign);
    let lead = match request {
        Some(requester) => format!(
            "{} asked to link **{name}** (MapleSEA). Nothing is fetched until you press Link.",
            who(requester)
        ),
        None => "Nothing is fetched until you press Link.".to_owned(),
    };
    vec![
        container(
            BLOSSOM,
            vec![
                text(format!("### Link {name} to its NEXON Open API data?")),
                text(lead),
                text(format!(
                    "**What Kanade fetches**\nName, world, class, level, combat power and the character image link, for {name} only."
                )),
                text(
                    "**Why**\nSo guild admins can check boss readiness (level and combat power) when they plan runs.",
                ),
                text(
                    "**How long**\nRefreshed once a day and kept 30 days at most. `/character unlink` deletes it at once.",
                ),
                text(
                    "**Who sees it**\nGuild admins in Kanade’s admin app, and the channel when you post a `/character share` card. It is never shared with anyone else and never sent to chat models.",
                ),
                text(format!(
                    "-# The data is shown as is and may be out of date. Kanade is not affiliated with or endorsed by NEXON.{}",
                    privacy(privacy_url)
                )),
                footer(None),
            ],
        ),
        action_row(vec![
            button(ButtonStyle::Success, &format!("Link {ign}"), link_id, false),
            button(ButtonStyle::Secondary, "Cancel", cancel_id, false),
        ]),
    ]
}

/// `**Class** Bishop · **Level** 283 · **Combat power** 48,203,114`, the
/// pairs NEXON answered; `None` with none.
fn pairs(items: &[(&str, Option<String>)]) -> Option<String> {
    let line: Vec<String> = items
        .iter()
        .filter_map(|(label, value)| value.as_ref().map(|value| format!("**{label}** {value}")))
        .collect();
    (!line.is_empty()).then(|| line.join(" · "))
}

fn stats(snapshot: &Snapshot) -> Option<String> {
    pairs(&[
        ("Class", snapshot.character_class.as_deref().map(plain)),
        ("Level", snapshot.level.map(|level| level.to_string())),
        ("Combat power", snapshot.combat_power.map(thousands)),
    ])
}

/// Texts beside the hotlinked image (never downloaded), else plain.
fn with_image(texts: Vec<String>, snapshot: &Snapshot) -> Vec<Component> {
    match &snapshot.image_url {
        Some(url) => vec![section_with_thumbnail(texts, url)],
        None => texts.into_iter().map(text).collect(),
    }
}

/// The reply that replaces the notice once linked: what is kept now.
pub fn linked(ign: &str, snapshot: &Snapshot) -> Vec<Component> {
    let mut facts = vec![format!("-# Linked · {}", plain(ign))];
    facts.extend(stats(snapshot));
    facts.push(
        pairs(&[
            ("World", snapshot.world_name.as_deref().map(plain)),
            ("As of", Some(as_of(snapshot))),
        ])
        .unwrap_or_default(),
    );
    let mut body = with_image(facts, snapshot);
    body.push(text(
        "-# Refreshed daily, kept 30 days at most. See all of it with `/character data`; remove it with `/character unlink`.",
    ));
    body.push(footer(None));
    vec![container(BLOSSOM, body)]
}

// ── /character data ──

/// One character in the `/character data` view.
pub enum DataEntry<'a> {
    Linked {
        ign: &'a str,
        /// Fresh data, if any.
        snapshot: Option<&'a Snapshot>,
        linked_at: Option<DateTime<Utc>>,
        notice: Option<u32>,
        /// Tracked share cards, by channel.
        cards: Vec<&'a str>,
        retired: bool,
    },
    Pending {
        ign: &'a str,
        requester: Option<&'a str>,
        requested_at: DateTime<Utc>,
    },
    Stale {
        ign: &'a str,
    },
    NotLinked {
        ign: &'a str,
    },
}

fn cards_line(cards: &[&str]) -> Option<String> {
    let mut channels: Vec<&str> = Vec::new();
    for card in cards {
        if !channels.contains(card) {
            channels.push(card);
        }
    }
    let places = channels
        .iter()
        .map(|id| channel(id))
        .collect::<Vec<_>>()
        .join(", ");
    match cards.len() {
        0 => None,
        1 => Some(format!("1 shared card in {places}")),
        n => Some(format!("{n} shared cards in {places}")),
    }
}

impl DataEntry<'_> {
    fn render(&self) -> String {
        match self {
            Self::Linked {
                ign,
                snapshot,
                linked_at,
                notice,
                cards,
                retired,
            } => {
                let state = if *retired {
                    "linked, retired"
                } else {
                    "linked"
                };
                let mut lines = vec![format!("**{} · {state}**", plain(ign))];
                match snapshot {
                    Some(snapshot) => {
                        let facts: Vec<String> = [
                            snapshot.character_class.as_deref().map(plain),
                            snapshot.level.map(|level| format!("Lv. {level}")),
                            snapshot
                                .combat_power
                                .map(|power| format!("CP {}", thousands(power))),
                            snapshot.world_name.as_deref().map(plain),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        if !facts.is_empty() {
                            lines.push(facts.join(" · "));
                        }
                        lines.push(format!(
                            "As of {} · fetched {} · deleted by {} unless refreshed",
                            as_of(snapshot),
                            stamp(snapshot.fetched_at, 'f'),
                            stamp(snapshot.expires_at, 'D')
                        ));
                    }
                    None if *retired => lines.push(
                        "Refresh is paused while it is retired; what is kept is deleted within 30 days of its last fetch."
                            .to_owned(),
                    ),
                    None => lines.push(
                        "No current data yet; Kanade refreshes it once a day.".to_owned(),
                    ),
                }
                let mut tail = Vec::new();
                if let Some(at) = linked_at {
                    tail.push(match notice {
                        Some(version) => format!("Linked {}, notice {version}", stamp(*at, 'f')),
                        None => format!("Linked {}", stamp(*at, 'f')),
                    });
                }
                tail.extend(cards_line(cards));
                if !tail.is_empty() {
                    lines.push(tail.join(" · "));
                }
                lines.join("\n")
            }
            Self::Pending {
                ign,
                requester,
                requested_at,
            } => format!(
                "**{} · pending**\nRequested by {} {}. Nothing is fetched until you confirm with `/character link`.",
                plain(ign),
                requester.map_or_else(|| "an admin".to_owned(), plain),
                stamp(*requested_at, 'D')
            ),
            Self::Stale { ign } => format!(
                "**{} · stale**\nNo current data: NEXON no longer finds it under that name. Link it again with `/character link`.",
                plain(ign)
            ),
            Self::NotLinked { ign } => {
                format!("**{} · not linked**\nNothing fetched or kept.", plain(ign))
            }
        }
    }
}

/// `/character data`: everything kept, per character, with dates. Every
/// entry is shown: entries go into as many containers as Discord's
/// per-message budget needs (one message each; see `split_reply`), each
/// footed by the attribution; the first has the title, the last the unlink
/// hint.
pub fn data(entries: &[DataEntry<'_>], privacy_url: Option<&str>) -> Vec<Component> {
    let hint = text(format!(
        "-# Remove one or all with `/character unlink`.{}",
        privacy(privacy_url)
    ));
    let mut pages: Vec<Vec<Component>> = vec![vec![
        text("### What Kanade keeps for you"),
        text("Linked characters, from the NEXON Open API. Nothing else from NEXON is kept."),
    ]];
    // How many components a page holds before its first entry.
    let mut head = 2;
    for entry in entries {
        let entry = text(entry.render());
        let page = pages.last_mut().expect("a first page");
        // Room is kept for the hint and the attribution on every page.
        let mut trial = page.clone();
        trial.extend([entry.clone(), hint.clone(), footer(None)]);
        if page.len() > head && !within_budget(&[container(BLOSSOM, trial)]) {
            pages.push(vec![entry]);
            head = 0;
        } else {
            page.push(entry);
        }
    }
    let last = pages.len() - 1;
    pages
        .into_iter()
        .enumerate()
        .map(|(index, mut body)| {
            if index == last {
                body.push(hint.clone());
            }
            body.push(footer(None));
            container(BLOSSOM, body)
        })
        .collect()
}

// ── /character unlink ──

/// One character unlinked; `cards` its tracked share cards' channels.
pub fn unlinked(ign: &str, cards: &[&str]) -> String {
    match cards_line(cards) {
        None => format!(
            "Unlinked {}. Its NEXON data is deleted. Your roster character is unchanged.",
            bold(ign)
        ),
        Some(line) => {
            let verb = if cards.len() == 1 { "reads" } else { "read" };
            format!(
                "Unlinked {}. Its NEXON data is deleted, and your {line} now {verb} “Character data removed”. Your roster character is unchanged.",
                bold(ign)
            )
        }
    }
}

pub fn unlinked_all(igns: &[&str]) -> String {
    let names = igns
        .iter()
        .map(|ign| bold(ign))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Unlinked everything left: {names}. Nothing from NEXON is kept for you now.")
}

pub fn not_linked(ign: &str) -> String {
    format!(
        "{} isn’t linked. Nothing from NEXON is kept for it.",
        bold(ign)
    )
}

// ── /character share ──

pub fn share_not_linked(ign: &str) -> String {
    format!(
        "{} isn’t linked. Link it first with `/character link`.",
        plain(ign)
    )
}

pub fn share_no_data(ign: &str) -> String {
    format!(
        "There is no current NEXON data for {} to share yet.",
        bold(ign)
    )
}

pub fn posted(channel_id: &str) -> String {
    format!(
        "Posted in {}. It turns into “Character data removed” after 29 days, or at once if you unlink.",
        channel(channel_id)
    )
}

pub fn cant_post(channel_id: &str) -> String {
    format!(
        "Kanade can’t post in {}; nothing was shared.",
        channel(channel_id)
    )
}

pub fn unconfirmed(channel_id: &str) -> String {
    format!(
        "Kanade couldn’t confirm the card in {}. If it was posted, the admins will remove it; nothing else was shared.",
        channel(channel_id)
    )
}

/// The footer a share card carries: the attribution, then its ref.
pub fn share_footer(reference: &str) -> String {
    format!("{ATTRIBUTION} · ref {reference}")
}

/// The public card: name, class, level, CP, as-of and the hotlinked image
/// (no world, no guild), footed by the attribution and its ref.
pub fn share_card(owner: &str, ign: &str, snapshot: &Snapshot, reference: &str) -> Vec<Component> {
    let name = snapshot.character_name.as_deref().unwrap_or(ign);
    let mut facts = vec![format!("### {}", plain(name))];
    facts.extend(stats(snapshot));
    facts.push(format!("**As of** {}", as_of(snapshot)));
    let mut body = vec![text(format!("-# {}’s character", plain(owner)))];
    body.extend(with_image(facts, snapshot));
    body.push(text(format!("-# {}", share_footer(reference))));
    vec![container(BLOSSOM, body)]
}

/// What a share card becomes at 29 days or on unlink: no data, no
/// attribution, a grey edge.
pub fn stub() -> Vec<Component> {
    vec![container(GREY, vec![text(STUB)])]
}

// ── the portal's notice ──

/// One titled part of the notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticePart {
    pub heading: &'static str,
    pub text: String,
}

/// The notice (version [`NOTICE_VERSION`]) as plain text for the public
/// portal: word for word [`notice`], without Discord's markdown. Consent on
/// either surface is consent to these words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeText {
    pub title: String,
    pub lead: String,
    pub what: NoticePart,
    pub why: NoticePart,
    pub how_long: NoticePart,
    pub who: NoticePart,
    /// "The data is shown as is …"; the privacy link follows it.
    pub as_is: &'static str,
    pub link_label: String,
    pub cancel_label: &'static str,
}

/// [`NoticeText`] for `ign`; `request` as for [`notice`].
pub fn notice_text(ign: &str, request: Option<Option<&str>>) -> NoticeText {
    debug_assert_eq!(NOTICE_VERSION, 1, "a new notice needs new copy");
    let lead = match request {
        Some(requester) => format!(
            "{} asked to link {ign} (MapleSEA). Nothing is fetched until you press Link.",
            requester.unwrap_or("An admin")
        ),
        None => "Nothing is fetched until you press Link.".to_owned(),
    };
    NoticeText {
        title: format!("Link {ign} to its NEXON Open API data?"),
        lead,
        what: NoticePart {
            heading: "What Kanade fetches",
            text: format!(
                "Name, world, class, level, combat power and the character image link, for {ign} only."
            ),
        },
        why: NoticePart {
            heading: "Why",
            text: "So guild admins can check boss readiness (level and combat power) when they plan runs."
                .to_owned(),
        },
        how_long: NoticePart {
            heading: "How long",
            text: "Refreshed once a day and kept 30 days at most. /character unlink deletes it at once."
                .to_owned(),
        },
        who: NoticePart {
            heading: "Who sees it",
            text: "Guild admins in Kanade’s admin app, and the channel when you post a /character share card. It is never shared with anyone else and never sent to chat models."
                .to_owned(),
        },
        as_is: "The data is shown as is and may be out of date. Kanade is not affiliated with or endorsed by NEXON.",
        link_label: format!("Link {ign}"),
        cancel_label: "Cancel",
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone};

    use super::*;

    #[test]
    fn numbers_and_dates_read_as_drawn() {
        assert_eq!(thousands(48_203_114), "48,203,114");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        let fetched = Utc.with_ymd_and_hms(2026, 9, 28, 17, 0, 0).unwrap();
        let mut snapshot = Snapshot {
            character_id: "c".into(),
            ocid: "o".into(),
            ocid_fetched_at: fetched,
            character_name: None,
            world_name: None,
            character_class: None,
            level: None,
            combat_power: None,
            image_url: None,
            data_date: None,
            fetched_at: fetched,
            expires_at: Snapshot::expiry(fetched, fetched),
        };
        // 01:00 SGT on 29 Sep.
        assert_eq!(as_of(&snapshot), "29 Sep 2026 (SGT)");
        snapshot.data_date = NaiveDate::from_ymd_opt(2026, 9, 27);
        assert_eq!(as_of(&snapshot), "27 Sep 2026 (SGT)");
    }

    #[test]
    fn names_never_become_markdown() {
        assert_eq!(plain("a*b_c"), "a\\*b\\_c");
        assert_eq!(
            unlinked_all(&["MikaBish", "Mikadere"]),
            "Unlinked everything left: **MikaBish**, **Mikadere**. Nothing from NEXON is kept for you now."
        );
    }

    /// Every text of `components`, with Discord's markdown taken out.
    fn bare_texts(components: &[Component]) -> Vec<String> {
        fn walk(value: &serde_json::Value, out: &mut Vec<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    for (key, inner) in map {
                        match (key.as_str(), inner) {
                            ("content" | "label", serde_json::Value::String(text)) => out.push(
                                text.replace("**", "")
                                    .replace('`', "")
                                    .replace("### ", "")
                                    .replace("-# ", "")
                                    .replace('\n', " "),
                            ),
                            _ => walk(inner, out),
                        }
                    }
                }
                serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&serde_json::to_value(components).unwrap(), &mut out);
        out
    }

    /// The portal's notice is the Discord notice word for word.
    #[test]
    fn the_portal_notice_is_the_discord_notice() {
        for request in [None, Some(None), Some(Some("Ren"))] {
            let portal = notice_text("AsahiDK", request);
            let discord = bare_texts(&notice(
                "AsahiDK",
                request,
                Some("https://pub.example/privacy"),
                "l".into(),
                "c".into(),
            ));
            let mut expected = vec![portal.title.clone(), portal.lead.clone()];
            for part in [&portal.what, &portal.why, &portal.how_long, &portal.who] {
                expected.push(format!("{} {}", part.heading, part.text));
            }
            expected.push(format!(
                "{} [Privacy notice](https://pub.example/privacy)",
                portal.as_is
            ));
            expected.push(ATTRIBUTION.to_owned());
            expected.push(portal.link_label.clone());
            expected.push(portal.cancel_label.to_owned());
            assert_eq!(discord, expected, "{request:?}");
        }
    }
}
