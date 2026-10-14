//! Character data (module `character_data`), as the server's admin API
//! answers it: one character's link and NEXON data, the admin request
//! (pending until the member confirms in Discord, which the mock does not
//! have), unlink, refresh (an admin's is always allowed; only the 24-hour
//! call budget, held here by the `alert`/`refused` health, stops it), and
//! Config's health block. Seeded with wholly invented data (the chardata
//! boards' states): MikaBish, MikaNL, YuzuPala and TsubaKain linked,
//! MikaHero linked but retired, Mikadere stale, Hotaru7 linked with its
//! data expired, AsahiDK pending, everyone else not linked.
//!
//! On with the roster module (the server refuses it without);
//! `POST /__mock/modules {"character_data": false}` turns it off until the
//! next reset (its routes then answer the generic 404 and Config has no
//! health block). `POST /__mock/character-data {"health": …}` picks the
//! health the boards draw: `ok` (default), `alert` (Nexon backoff, scouter
//! verification, a stuck and an unknown card), `setup` (no Nexon key: links
//! are `not_connected`; no scouter URL: estimates off) or `refused` (key
//! refused, scouter unreachable).

use serde::Deserialize;
use serde_json::{Value, json};

use super::clock::{TZ_OFFSET_SECS, iso_date, iso_secs, now_secs};
use super::{MoveError, Store, coded};

pub const ATTRIBUTION: &str = "Data based on NEXON Open API";
/// The public `/privacy` page (invented host).
pub const PRIVACY_URL: &str = "https://portal.example.invalid/privacy";
pub(super) const NOTICE_VERSION: u32 = 1;
pub(super) const DAY: i64 = 86_400;
const BUDGET: u32 = 800;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    #[default]
    Ok,
    Alert,
    Setup,
    Refused,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthRequest {
    pub health: Health,
}

#[derive(Clone)]
pub struct Link {
    pub(super) character_id: String,
    /// `pending`, `linked` or `stale`; a linked one with no fresh data 30
    /// days after linking reads as `expired`.
    pub(super) state: &'static str,
    pub(super) version: u32,
    pub(super) requested_by: (&'static str, String),
    pub(super) requested_at: i64,
    pub(super) linked_at: Option<i64>,
    /// Where the member consented: `discord` or `portal`.
    pub(super) consent_surface: Option<&'static str>,
    /// Invented data and when it was fetched.
    pub(super) data: Option<(Value, i64)>,
    /// Tracked share cards: channel name and when each was posted.
    pub(super) cards: Vec<(&'static str, i64)>,
}

pub(super) fn facts(
    name: &str,
    class: &str,
    level: i64,
    power: i64,
    fetched: i64,
) -> Option<(Value, i64)> {
    Some((
        json!({
            "name": name,
            "world": "Aquila",
            "class": class,
            "level": level,
            "combat_power": power,
            "image_url": null,
        }),
        fetched,
    ))
}

pub(super) fn linked(id: &str, member: &str, linked_at: i64, data: Option<(Value, i64)>) -> Link {
    Link {
        character_id: id.into(),
        state: "linked",
        version: 2,
        requested_by: ("member", member.into()),
        requested_at: linked_at - 600,
        linked_at: Some(linked_at),
        consent_surface: Some("discord"),
        data,
        cards: Vec::new(),
    }
}

/// The boards' states at the mock's now (Tue 29 Sep 12:00): today's batch
/// ran at 04:01, 7 h 59 min ago. `r-carling`'s readiness board: YuzuPala
/// (`ch-s8`) and TsubaKain (`ch-s9`) linked, Hotaru7 (`ch-s10`) expired.
pub fn seed() -> Vec<Link> {
    let now = now_secs();
    let batch = now - (7 * 3_600 + 59 * 60);
    vec![
        // MikaHero (retired): refresh paused, lapses 30 days after 19 Sep.
        linked(
            "ch-s1",
            "1003",
            now - 56 * DAY,
            facts("MikaHero", "Hero", 261, 22_913_402, batch - 10 * DAY),
        ),
        linked(
            "ch-s2",
            "1003",
            now - 30 * DAY,
            facts("MikaNL", "Night Lord", 287, 52_640_118, now - DAY),
        ),
        linked(
            "ch-s3",
            "1003",
            now - 8 * DAY,
            facts("MikaBish", "Bishop", 283, 48_204_551, batch),
        ),
        Link {
            state: "stale",
            ..linked("ch-s4", "1003", now - 20 * DAY, None)
        },
        Link {
            character_id: "ch-s5".into(),
            state: "pending",
            version: 1,
            requested_by: ("admin", "discord:1001".into()),
            requested_at: now - 20 * 60,
            linked_at: None,
            consent_surface: None,
            data: None,
            cards: Vec::new(),
        },
        linked(
            "ch-s8",
            "1004",
            now - 9 * DAY,
            facts("YuzuPala", "Paladin", 276, 52_104_311, batch),
        ),
        linked(
            "ch-s9",
            "1005",
            now - 14 * DAY,
            facts("TsubaKain", "Kain", 281, 38_412_906, batch),
        ),
        // Hotaru7: no refresh has reached Nexon for 31 days.
        linked(
            "ch-s10",
            "1011",
            now - 40 * DAY,
            facts("Hotaru7", "Adele", 279, 44_870_215, now - 31 * DAY),
        ),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    #[serde(default)]
    version: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupRequest {
    ign: String,
    #[serde(default)]
    world: Option<String>,
}

impl LookupRequest {
    /// A member's lookup: always MapleSEA.
    pub(super) fn msea(ign: String) -> Self {
        Self { ign, world: None }
    }
}

/// The data a reader may show: kept at most 30 days, never stamped in the future.
pub(super) fn fresh(link: Option<&Link>, now: i64) -> Option<&(Value, i64)> {
    link.and_then(|link| link.data.as_ref())
        .filter(|(_, fetched)| fetched + 30 * DAY > now && *fetched <= now + 300)
}

/// The strip's state of an existing link: `expired` is linked with no fresh
/// data 30 days after linking, as the server derives it.
pub(super) fn state_of(link: &Link, now: i64) -> &'static str {
    match link.state {
        "linked"
            if fresh(Some(link), now).is_none()
                && link.linked_at.is_some_and(|at| at + 30 * DAY <= now) =>
        {
            "expired"
        }
        state => state,
    }
}

pub(super) fn view(id: &str, link: Option<&Link>, connected: bool, now: i64) -> Value {
    let iso = iso_secs;
    let data = fresh(link, now).map(|(data, fetched)| {
        let mut data = data.clone();
        // The response date in Singapore time (the mock's data has none: the fetch day).
        data["as_of"] = json!(iso_date((fetched + TZ_OFFSET_SECS).div_euclid(DAY)));
        data["fetched_at"] = json!(iso(*fetched));
        data["expires_at"] = json!(iso(fetched + 30 * DAY));
        data
    });
    let state = match link {
        Some(link) => state_of(link, now),
        None if connected => "not_linked",
        None => "not_connected",
    };
    json!({
        "character_id": id,
        "state": state,
        "version": link.map_or(0, |link| link.version),
        "connected": connected,
        "requested_by": link.map(|link| json!({"kind": link.requested_by.0, "id": link.requested_by.1})),
        "requested_at": link.map(|link| iso(link.requested_at)),
        "consent_version": link.and_then(|link| link.linked_at).map(|_| NOTICE_VERSION),
        "consented_at": link.and_then(|link| link.linked_at).map(iso),
        "linked_at": link.and_then(|link| link.linked_at).map(iso),
        "consent_surface": link.and_then(|link| link.consent_surface),
        "notice_version": NOTICE_VERSION,
        "data": data,
        "refresh": {
            "next_attempt_at": null,
            "last_error": null,
        },
        "attribution": ATTRIBUTION,
        "privacy_url": PRIVACY_URL,
    })
}

/// A linked character's fresh NEXON `(level, combat power, as_of)`.
pub(super) type NexonValues = (Option<i64>, Option<i64>, String);

impl Store {
    /// The module's startup flag: on only with the roster.
    pub fn character_data_on(&self) -> bool {
        self.roster_on() && self.character_data
    }

    /// `POST /__mock/modules {character_data}`.
    pub fn set_character_data(&mut self, on: bool) {
        self.character_data = on;
    }

    /// `POST /__mock/character-data {health}`: without a scouter URL
    /// (`setup`) estimates are off too.
    pub fn set_chardata_health(&mut self, health: Health) {
        self.chardata_health = health;
        if health == Health::Setup {
            self.set_estimate_scenario(super::estimates::Scenario::Off);
        }
    }

    pub(super) fn connected(&self) -> bool {
        self.chardata_health != Health::Setup
    }

    fn roster_character(&self, id: &str) -> Result<&super::characters::Char, MoveError> {
        self.characters
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| coded(404, "not_found", "That character is not on the roster."))
    }

    pub(super) fn link_of(&self, id: &str) -> Option<&Link> {
        self.links.iter().find(|link| link.character_id == id)
    }

    pub fn character_link(&self, id: &str) -> Result<Value, MoveError> {
        self.roster_character(id)?;
        Ok(view(id, self.link_of(id), self.connected(), now_secs()))
    }

    /// For readiness: `id`'s strip state and, from a linked character's
    /// fresh data only, its NEXON values.
    pub(super) fn nexon_values(&self, id: &str, now: i64) -> (&'static str, Option<NexonValues>) {
        let link = self.link_of(id);
        let state = match link {
            Some(link) => state_of(link, now),
            None if self.connected() => "not_linked",
            None => "not_connected",
        };
        let values = fresh(link, now)
            .filter(|_| state == "linked")
            .map(|(data, fetched)| {
                (
                    data["level"].as_i64(),
                    data["combat_power"].as_i64(),
                    iso_date((fetched + TZ_OFFSET_SECS).div_euclid(DAY)),
                )
            });
        (state, values)
    }

    /// `201`: pending; the mock never calls anything.
    pub fn request_link(&mut self, id: &str, req: LinkRequest) -> Result<Value, MoveError> {
        let character = self.roster_character(id)?;
        if !self.connected() {
            return Err(coded(
                409,
                "not_connected",
                "Character data isn't connected yet.",
            ));
        }
        if character.retired_at.is_some() {
            return Err(coded(
                409,
                "retired",
                "A retired character can't be linked.",
            ));
        }
        if character.world != "msea" {
            return Err(coded(
                422,
                "region",
                "Only MapleSEA characters can be linked.",
            ));
        }
        let version = req.version.ok_or_else(|| {
            coded(
                422,
                "version_required",
                "Send the link version as loaded (0 when not linked).",
            )
        })?;
        let current = self.link_of(id).cloned();
        if current.as_ref().map_or(0, |link| link.version) != version {
            return Err(coded(409, "stale", "The link changed since it was loaded."));
        }
        if current.as_ref().is_some_and(|link| link.state != "stale") {
            return Err(coded(
                409,
                "already_linked",
                "That character is already linked or waiting for its member.",
            ));
        }
        let now = now_secs();
        self.links.retain(|link| link.character_id != id);
        let link = Link {
            character_id: id.into(),
            state: "pending",
            version: version + 1,
            requested_by: ("admin", "discord:1001".into()),
            requested_at: now,
            linked_at: None,
            consent_surface: None,
            data: None,
            cards: Vec::new(),
        };
        let answer = view(id, Some(&link), true, now);
        self.links.push(link);
        Ok(answer)
    }

    /// After a create: with the module on and connected, the new MapleSEA
    /// character's member is asked to link it (`pending`; `failed` if that
    /// were refused); `None` when nothing is asked.
    pub(super) fn ask_link_on_create(&mut self, id: &str) -> Option<&'static str> {
        if !self.character_data_on() || !self.connected() {
            return None;
        }
        Some(
            match self.request_link(id, LinkRequest { version: Some(0) }) {
                Ok(_) => "pending",
                Err(_) => "failed",
            },
        )
    }

    /// `GET /api/admin/characters/lookup`.
    pub fn lookup_status(&self) -> Value {
        json!({ "connected": self.connected() })
    }

    /// `POST /api/admin/characters/lookup`: invented answers by IGN prefix
    /// (`Nope…` not found, `Busy…` calls held, `Offline…` not connected,
    /// `Wait…` the per-admin spacing), else found; nothing is kept. The
    /// `setup` health is not connected, `alert`/`refused` hold every call.
    pub fn look_up(&self, req: LookupRequest) -> Result<Value, MoveError> {
        let ign = super::characters::ign(&req.ign)?;
        if super::characters::world(req.world.as_deref())? != "msea" {
            return Err(coded(
                422,
                "region",
                "Only MapleSEA characters can be looked up.",
            ));
        }
        let prefixed = |prefix: &str| {
            ign.len() >= prefix.len() && ign[..prefix.len()].eq_ignore_ascii_case(prefix)
        };
        if !self.connected() || prefixed("offline") {
            return Err(coded(
                409,
                "not_connected",
                "Character data isn't connected yet.",
            ));
        }
        if prefixed("wait") {
            return Err(coded(
                429,
                "lookup_cooldown",
                "Wait a few seconds before looking up again.",
            ));
        }
        if matches!(self.chardata_health, Health::Alert | Health::Refused) || prefixed("busy") {
            return Err(coded(
                503,
                "nexon_unavailable",
                "The Open API can't be called right now (maintenance).",
            ));
        }
        if prefixed("nope") {
            return Err(coded(
                404,
                "not_found",
                "NEXON has no MapleSEA character by that name.",
            ));
        }
        // A stable invented class and level per name.
        const CLASSES: [&str; 6] = ["Bishop", "Kain", "Night Lord", "Paladin", "Adele", "Lara"];
        let seed = ign.bytes().map(usize::from).sum::<usize>();
        let now = now_secs();
        Ok(json!({
            "name": ign,
            "world": "Aquila",
            "class": CLASSES[seed % CLASSES.len()],
            "level": 220 + seed % 70,
            "image_url": null,
            "as_of": iso_date((now + TZ_OFFSET_SECS).div_euclid(DAY)),
            "attribution": ATTRIBUTION,
        }))
    }

    pub fn unlink(&mut self, id: &str) -> Result<Value, MoveError> {
        let before = self.links.len();
        self.links.retain(|link| link.character_id != id);
        if self.links.len() == before {
            return Err(coded(404, "not_linked", "That character is not linked."));
        }
        Ok(json!({"message": "Unlinked: its character data is deleted."}))
    }

    /// An admin's refresh is always allowed; only held calls stop it.
    pub fn refresh_link(&mut self, id: &str) -> Result<Value, MoveError> {
        let retired = self.roster_character(id)?.retired_at.is_some();
        if !self.connected() {
            return Err(coded(
                409,
                "not_connected",
                "Character data isn't connected yet.",
            ));
        }
        if matches!(self.chardata_health, Health::Alert | Health::Refused) {
            return Err(coded(
                503,
                "nexon_unavailable",
                "The Open API can't be called right now (calls are held).",
            ));
        }
        let now = now_secs();
        let Some(link) = self
            .links
            .iter_mut()
            .find(|link| link.character_id == id && link.state == "linked" && !retired)
        else {
            return Err(coded(409, "not_linked", "That character is not linked."));
        };
        if let Some((data, fetched)) = &mut link.data {
            data["level"] = json!(data["level"].as_i64().unwrap_or(1) + 1);
            *fetched = now;
        }
        Ok(view(id, Some(link), true, now))
    }

    /// Config's `character_data` block; `None` while the module is off.
    pub fn chardata_health(&self) -> Option<Value> {
        if !self.character_data_on() {
            return None;
        }
        let now = now_secs();
        let iso = iso_secs;
        // Today 04:00 in Singapore, and the hourly sweep at 12:00.
        let batch = now - (8 * 3_600);
        let count = |state: &str| {
            self.links
                .iter()
                .filter(|link| state_of(link, now) == state)
                .count()
        };
        let links = json!({
            "pending": count("pending"),
            "active": count("linked") + count("expired"),
            "stale": count("stale"),
        });
        let posts = |items: Vec<Value>| {
            let of = |state: &str| items.iter().filter(|item| item["state"] == state).count();
            json!({"due": 0, "unknown": of("unknown"), "stuck": of("stuck"), "overdue": of("overdue"), "items": items})
        };
        let scouter = |state: &str, error: Option<(&str, i64)>| {
            json!({
                "state": state,
                "url": (state != "off").then_some("http://scouter:8091"),
                "last_lookup_at": (state != "off").then(|| iso(now - 8 * 60)),
                "last_error": error.map(|(code, _)| code),
                "last_error_at": error.map(|(_, at)| iso(at)),
            })
        };
        let scouter_off = !self.estimates_on();
        let (nexon, backoff, calls, last_batch, links, posts, scouter) = match self.chardata_health
        {
            Health::Ok => (
                "ok",
                None,
                Some(162),
                json!({"at": iso(batch + 60), "refreshed": 41, "failed": 0, "stopped": null}),
                links,
                posts(Vec::new()),
                scouter("ok", None),
            ),
            Health::Alert => (
                "backoff",
                Some(iso(now + 30 * 60)),
                Some(188),
                json!({"at": iso(batch + 60), "refreshed": 38, "failed": 3, "stopped": "OPENAPI00011"}),
                links,
                posts(vec![
                    json!({"reference": "3fa91c", "state": "stuck", "channel_id": "hstar-party",
                        "posted_at": iso(now - 29 * DAY), "jump_url": "https://discord.com/channels/900000000000000001/900000000000000107/900000000000004242"}),
                    json!({"reference": "9be210", "state": "unknown", "channel_id": "kalos-four",
                        "posted_at": iso(now - 2 * DAY), "jump_url": null}),
                ]),
                scouter(
                    "verification_required",
                    Some(("verification_required", now - 13 * 60)),
                ),
            ),
            Health::Setup => (
                "no_key",
                None,
                Some(0),
                Value::Null,
                Value::Null,
                Value::Null,
                scouter("off", None),
            ),
            Health::Refused => (
                "key_refused",
                None,
                Some(1),
                json!({"at": iso(batch), "refreshed": 0, "failed": 1, "stopped": "OPENAPI00005"}),
                links,
                posts(Vec::new()),
                scouter("unreachable", Some(("unreachable", now - 2 * 60))),
            ),
        };
        Some(json!({
            "nexon": nexon,
            "backoff_until": backoff,
            "calls_24h": calls,
            "budget": BUDGET,
            "last_batch": last_batch,
            "links": links,
            "last_sweep": iso(now),
            "posts": posts,
            "scouter": if scouter_off { json!({"state": "off", "url": null, "last_lookup_at": null, "last_error": null, "last_error_at": null}) } else { scouter },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_names_seeded_characters() {
        let characters = super::super::characters::seed();
        for link in seed() {
            assert!(characters.iter().any(|c| c.id == link.character_id));
        }
    }

    #[test]
    fn the_seed_draws_every_link_state() {
        let now = now_secs();
        let states: Vec<_> = seed().iter().map(|link| state_of(link, now)).collect();
        for state in ["linked", "pending", "stale", "expired"] {
            assert!(states.contains(&state), "{state}");
        }
        let unlinked = view("ch-s7", None, true, now);
        assert_eq!(unlinked["state"], "not_linked");
        assert_eq!(view("ch-s7", None, false, now)["state"], "not_connected");
    }
}
