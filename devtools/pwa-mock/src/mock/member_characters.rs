//! The member's own characters on the public origin, as the server answers
//! them (`src/api/public/characters/`): their roster rows with the admin
//! rules (no delete), the NEXON notice (v1 word for word), Link / Cancel /
//! Unlink / Unlink all / My data, the Add lookup preview, their own
//! MapleScouter lookups (one counted start per 30 minutes; a failed one
//! does not count; the pinned clock never ages it, so it lasts until the
//! next reset) and the party's characters on a run they are on. Another
//! member's character or lookup is the generic 404.
//!
//! Link outcomes by IGN prefix (wholly invented): `Nope…` NEXON has no such
//! character (nothing kept), `Busy…` the daily budget is spent, `Later…`
//! linked with the data to come; the `alert`/`refused` health holds every
//! call (`unavailable`). `POST /__mock/public/characters {"roster": …}`
//! draws Asahi's characters as the member-characters boards do (`board`:
//! AsahiKanna linked main, AsahiDK pending on Ren's request, AsahiPath
//! stale, AsahiBlaze expired, AsahiMech retired) or with none (`empty`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::character_data::{
    ATTRIBUTION, DAY, Link, LookupRequest, NOTICE_VERSION, PRIVACY_URL, coded, facts, linked,
    state_of, view,
};
use super::characters::{Char, CharacterCreate, CharacterPatch};
use super::clock::{iso_secs, now_secs};
use super::estimates::{EstimateRequest, LABEL, NOTE};
use super::portal::snowflake;
use super::{MoveError, Store};

/// A member starts one counted MapleScouter lookup this often.
const ESTIMATE_SPACING: i64 = 30 * 60;
/// A pending request lapses after this.
const PENDING_TTL: i64 = 14 * DAY;

/// The server's generic 404 (no oracle for another member's ids).
fn generic() -> MoveError {
    coded(404, "not_found", "No such endpoint on this origin.")
}

/// A member character write already answered under an `Idempotency-Key`.
#[derive(Clone)]
pub struct Keyed {
    key: String,
    request: String,
    answer: Value,
}

/// `MyCharacterCreate`.
pub type MyCreate = CharacterCreate;
/// `MyCharacterPatch`.
pub type MyPatch = CharacterPatch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MyLookup {
    ign: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkConfirm {
    notice: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MyEstimate {
    character_id: String,
    #[serde(default)]
    refresh: bool,
}

/// A refused member lookup start: a plain refusal, or the 30-minute limit.
pub enum EstimateRefused {
    Coded(MoveError),
    Limited { next_at: String },
}

impl From<MoveError> for EstimateRefused {
    fn from(error: MoveError) -> Self {
        Self::Coded(error)
    }
}

/// Which characters Asahi has (`POST /__mock/public/characters`).
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RosterScenario {
    Board,
    Empty,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterRequest {
    pub roster: RosterScenario,
}

/// The v1 notice's words (`src/bot/chardata/copy.rs` `notice`).
fn notice_words(ign: &str, request: Option<Option<&str>>) -> Value {
    let lead = match request {
        Some(name) => format!(
            "{} asked to link {ign} (MapleSEA). Nothing is fetched until you press Link.",
            name.unwrap_or("An admin")
        ),
        None => "Nothing is fetched until you press Link.".to_owned(),
    };
    let part = |heading: &str, text: String| json!({ "heading": heading, "text": text });
    json!({
        "version": NOTICE_VERSION,
        "title": format!("Link {ign} to its NEXON Open API data?"),
        "lead": lead,
        "what": part("What Kanade fetches", format!("Name, world, class, level, combat power and the character image link, for {ign} only.")),
        "why": part("Why", "So guild admins can check boss readiness (level and combat power) when they plan runs.".into()),
        "how_long": part("How long", "Refreshed once a day and kept 30 days at most. /character unlink deletes it at once.".into()),
        "who": part("Who sees it", "Guild admins in Kanade’s admin app, and the channel when you post a /character share card. It is never shared with anyone else and never sent to chat models.".into()),
        "as_is": "The data is shown as is and may be out of date. Kanade is not affiliated with or endorsed by NEXON.",
        "link_label": format!("Link {ign}"),
        "cancel_label": "Cancel",
    })
}

impl Store {
    /// The caller's own character `id`; anyone else's is the generic 404.
    fn own(&self, me: &str, id: &str) -> Result<&Char, MoveError> {
        self.characters
            .iter()
            .find(|c| c.id == id && c.member_id == me)
            .ok_or_else(generic)
    }

    /// The admin who asked for `link`, by name (`null` otherwise).
    fn requester(&self, link: &Link) -> Option<String> {
        let (kind, id) = &link.requested_by;
        (*kind == "admin")
            .then(|| id.strip_prefix("discord:"))
            .flatten()
            .and_then(|user| self.member_display(user))
    }

    fn pending_json(&self, c: &Char, link: &Link) -> Value {
        json!({
            "character_id": c.id, "ign": c.ign, "class": c.class,
            "requested_by": {"kind": link.requested_by.0, "id": link.requested_by.1},
            "requested_by_name": self.requester(link),
            "requested_at": iso_secs(link.requested_at),
            "expires_at": iso_secs(link.requested_at + PENDING_TTL),
        })
    }

    fn mine(&self, me: &str) -> impl Iterator<Item = &Char> {
        self.characters.iter().filter(move |c| c.member_id == me)
    }

    fn my_replay(&self, key: &str, request: &str) -> Result<Option<(bool, Value)>, MoveError> {
        match self.member_character_writes.iter().find(|w| w.key == key) {
            Some(w) if w.request == request => Ok(Some((false, w.answer.clone()))),
            Some(_) => Err(coded(
                422,
                "idempotency_mismatch",
                "That Idempotency-Key was used for another request.",
            )),
            None => Ok(None),
        }
    }

    fn my_remember(&mut self, key: &str, request: String, answer: &Value) {
        self.member_character_writes.push(Keyed {
            key: key.to_owned(),
            request,
            answer: answer.clone(),
        });
    }

    /// `GET /api/public/me/characters`: `MyCharacters`.
    pub fn my_characters(&self, me: &str) -> Value {
        let now = now_secs();
        let module = self.character_data_on();
        let mut links = serde_json::Map::new();
        let mut pending = Vec::new();
        for c in self.mine(me) {
            if !module {
                break;
            }
            let link = self.link_of(&c.id);
            if let Some(link) = link.filter(|link| link.state == "pending") {
                pending.push((link.requested_at, self.pending_json(c, link)));
            }
            links.insert(c.id.clone(), view(&c.id, link, self.connected(), now));
        }
        pending.sort_by_key(|(at, _)| *at);
        json!({
            "characters": self.mine(me).map(Char::json).collect::<Vec<_>>(),
            "links": links,
            "pending": pending.into_iter().map(|(_, p)| p).collect::<Vec<_>>(),
            "character_data": module,
            "connected": module && self.connected(),
            "estimates": self.estimates_on(),
            "notice_version": NOTICE_VERSION,
            "privacy_url": module.then_some(PRIVACY_URL),
        })
    }

    /// `POST /api/public/me/characters`: `(created, MyCharacterCreated)`.
    pub fn my_create(
        &mut self,
        me: &str,
        key: &str,
        req: MyCreate,
    ) -> Result<(bool, Value), MoveError> {
        let request = format!(
            "create:{}:{:?}:{:?}:{:?}:{:?}",
            req.ign, req.world, req.class, req.level, req.role
        );
        if let Some(found) = self.my_replay(key, &request)? {
            return Ok(found);
        }
        let world = super::characters::world(req.world.as_deref())?;
        let linkable = world == "msea" && self.character_data_on() && self.connected();
        if !linkable && (req.class.is_none() || req.level.is_none()) {
            return Err(coded(
                422,
                "class_level_required",
                "Give the class and level: NEXON can't be asked for this character.",
            ));
        }
        let mut row = self.insert_character(me, req, false)?;
        row["linkable"] = json!(linkable);
        self.my_remember(key, request, &row);
        Ok((true, row))
    }

    /// `PATCH /api/public/me/characters/{id}`: the row; a new IGN or region
    /// unlinks it at once.
    pub fn my_patch(
        &mut self,
        me: &str,
        key: &str,
        id: &str,
        req: MyPatch,
    ) -> Result<Value, MoveError> {
        let request = format!("patch:{id}:{:?}:{:?}", req.ign, req.world);
        let before = self.own(me, id)?.clone();
        if let Some((_, found)) = self.my_replay(key, &request)? {
            return Ok(found);
        }
        let row = self.patch_character(id, req)?;
        if row["ign"] != before.ign || row["world"] != before.world {
            self.links.retain(|link| link.character_id != id);
        }
        self.my_remember(key, request, &row);
        Ok(row)
    }

    /// `GET /api/public/me/characters/{id}/link`.
    pub fn my_link(&self, me: &str, id: &str) -> Result<Value, MoveError> {
        self.own(me, id)?;
        Ok(view(id, self.link_of(id), self.connected(), now_secs()))
    }

    /// `GET /api/public/me/characters/{id}/link/notice`: `LinkNotice`.
    pub fn my_notice(&self, me: &str, id: &str) -> Result<Value, MoveError> {
        let c = self.own(me, id)?;
        if !self.connected() {
            return Err(coded(
                409,
                "not_connected",
                "Character data isn't connected yet.",
            ));
        }
        if c.world != "msea" {
            return Err(coded(
                422,
                "region",
                "Only MapleSEA characters can be linked.",
            ));
        }
        if c.retired_at.is_some() {
            return Err(coded(
                409,
                "retired",
                "A retired character can't be linked.",
            ));
        }
        let link = self.link_of(id);
        if link.is_some_and(|link| link.state == "linked") {
            return Err(coded(
                409,
                "already_linked",
                "That character is already linked.",
            ));
        }
        let request = link
            .filter(|link| link.state == "pending" && link.requested_by.0 == "admin")
            .map(|link| self.pending_json(c, link));
        let name = request
            .as_ref()
            .map(|request| request["requested_by_name"].as_str());
        let mut notice = notice_words(&c.ign, name);
        notice["character_id"] = json!(c.id);
        notice["ign"] = json!(c.ign);
        notice["request"] = request.unwrap_or(Value::Null);
        notice["privacy_url"] = json!(PRIVACY_URL);
        notice["attribution"] = json!(ATTRIBUTION);
        Ok(notice)
    }

    /// `POST /api/public/me/characters/{id}/link`: `LinkOutcome`, the
    /// consent recorded with surface `portal`.
    pub fn my_confirm(&mut self, me: &str, id: &str, req: LinkConfirm) -> Result<Value, MoveError> {
        let c = self.own(me, id)?.clone();
        if c.world != "msea" {
            return Ok(json!({"state": "other_region", "link": null}));
        }
        if c.retired_at.is_some() {
            return Err(coded(
                409,
                "retired",
                "A retired character can't be linked.",
            ));
        }
        if !self.connected() {
            return Err(coded(
                409,
                "not_connected",
                "Character data isn't connected yet.",
            ));
        }
        if req.notice != NOTICE_VERSION {
            return Err(coded(
                409,
                "notice_changed",
                "The notice changed: read it again before linking.",
            ));
        }
        let now = now_secs();
        let current = self.link_of(id).cloned();
        if current.as_ref().is_some_and(|link| link.state == "linked") {
            let link = view(id, current.as_ref(), true, now);
            return Ok(json!({"state": "linked", "link": link}));
        }
        let prefixed = |prefix: &str| c.ign.to_ascii_lowercase().starts_with(prefix);
        if matches!(
            self.chardata_health,
            super::character_data::Health::Alert | super::character_data::Health::Refused
        ) {
            return Ok(json!({"state": "unavailable", "link": null}));
        }
        if prefixed("busy") {
            return Ok(json!({"state": "busy", "link": null}));
        }
        self.links.retain(|link| link.character_id != id);
        if prefixed("nope") {
            return Ok(json!({"state": "not_found", "link": null}));
        }
        let data = (!prefixed("later")).then(|| {
            facts(
                &c.ign,
                c.class.as_deref().unwrap_or("Bishop"),
                c.level.unwrap_or(250) + 1,
                41_250_000,
                now,
            )
        });
        let link = Link {
            version: current.as_ref().map_or(0, |link| link.version) + 1,
            requested_by: current
                .as_ref()
                .map_or(("member", me.to_owned()), |link| link.requested_by.clone()),
            requested_at: current.as_ref().map_or(now, |link| link.requested_at),
            consent_surface: Some("portal"),
            ..linked(id, me, now, data.flatten())
        };
        let answer = view(id, Some(&link), true, now);
        let state = if answer["data"].is_null() {
            "linked_later"
        } else {
            "linked"
        };
        self.links.push(link);
        Ok(json!({"state": state, "link": answer}))
    }

    /// `POST /api/public/me/characters/{id}/link/cancel`: nothing changes.
    pub fn my_cancel(&self, me: &str, id: &str) -> Result<Value, MoveError> {
        self.own(me, id)?;
        let request = self
            .link_of(id)
            .filter(|link| link.state == "pending" && link.requested_by.0 == "admin");
        Ok(json!({
            "request_kept": request.is_some(),
            "requested_by_name": request.and_then(|link| self.requester(link)),
        }))
    }

    /// `DELETE /api/public/me/characters/{id}/link`: `Unlinked`, at once.
    pub fn my_unlink(&mut self, me: &str, key: &str, id: &str) -> Result<Value, MoveError> {
        let request = format!("unlink:{id}");
        if let Some((_, found)) = self.my_replay(key, &request)? {
            return Ok(found);
        }
        let ign = self.own(me, id)?.ign.clone();
        let link = self
            .link_of(id)
            .cloned()
            .ok_or_else(|| coded(404, "not_linked", "That character is not linked."))?;
        self.links.retain(|link| link.character_id != id);
        let answer = json!({"unlinked": ign, "cards": link.cards.len()});
        self.my_remember(key, request, &answer);
        Ok(answer)
    }

    /// `DELETE /api/public/me/characters/links`: `UnlinkedAll`.
    pub fn my_unlink_all(&mut self, me: &str, key: &str) -> Result<Value, MoveError> {
        let request = "unlink_all".to_owned();
        if let Some((_, found)) = self.my_replay(key, &request)? {
            return Ok(found);
        }
        let mine: Vec<(String, String)> = self
            .mine(me)
            .filter(|c| self.link_of(&c.id).is_some())
            .map(|c| (c.id.clone(), c.ign.clone()))
            .collect();
        self.links
            .retain(|link| !mine.iter().any(|(id, _)| *id == link.character_id));
        let answer = json!({"unlinked": mine.into_iter().map(|(_, ign)| ign).collect::<Vec<_>>()});
        self.my_remember(key, request, &answer);
        Ok(answer)
    }

    /// `GET /api/public/me/data`: `MyData`, requests first, then the active
    /// main, other active characters and retired ones, each group by IGN.
    pub fn my_data(&self, me: &str) -> Value {
        let now = now_secs();
        let mut entries: Vec<_> = self
            .mine(me)
            .filter_map(|c| {
                let link = self.link_of(&c.id);
                if link.is_none() && (c.world != "msea" || c.retired_at.is_some()) {
                    return None;
                }
                let state = link.map_or("not_linked", |link| state_of(link, now));
                let pending = state == "pending";
                let shown = view(&c.id, link, true, now);
                let order = (
                    !pending,
                    c.retired_at.is_some(),
                    !c.active_main(),
                    c.ign.to_ascii_lowercase(),
                    c.ign.clone(),
                    c.id.clone(),
                );
                Some((order, json!({
                    "character_id": c.id, "ign": c.ign, "state": state,
                    "retired": c.retired_at.is_some(),
                    "data": shown["data"],
                    "requested_by": if pending { shown["requested_by"].clone() } else { Value::Null },
                    "requested_by_name": link.filter(|_| pending).and_then(|link| self.requester(link)),
                    "requested_at": link.filter(|_| pending).map(|link| iso_secs(link.requested_at)),
                    "request_expires_at": link.filter(|_| pending).map(|link| iso_secs(link.requested_at + PENDING_TTL)),
                    "linked_at": shown["linked_at"],
                    "notice": shown["consent_version"],
                    "consented_at": shown["consented_at"],
                    "consent_surface": shown["consent_surface"],
                    "cards": link.map(|link| link.cards.iter().map(|(channel, at)| json!({
                        "channel": channel, "posted_at": iso_secs(*at), "cleared_by": iso_secs(at + 29 * DAY),
                    })).collect::<Vec<_>>()).unwrap_or_default(),
                })))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        json!({
            "entries": entries.into_iter().map(|(_, entry)| entry).collect::<Vec<_>>(),
            "notice_version": NOTICE_VERSION,
            "privacy_url": PRIVACY_URL,
            "attribution": ATTRIBUTION,
        })
    }

    /// `POST /api/public/me/characters/lookup`: the admin lookup's answer,
    /// never kept (the mock does not rate-limit it).
    pub fn my_look_up(&self, req: MyLookup) -> Result<Value, MoveError> {
        self.look_up(LookupRequest::msea(req.ign))
    }

    /// `POST /api/public/me/estimates`: `{job}` (`202`), or the member's
    /// 30-minute limit.
    pub fn my_estimate_start(
        &mut self,
        me: &str,
        req: MyEstimate,
    ) -> Result<Value, EstimateRefused> {
        let id = req.character_id.trim().to_owned();
        self.own(me, &id)?;
        let status = self.estimate_status(Some(&id), None)?;
        // Already queued or running: that lookup, at no cost.
        if matches!(status["job"]["state"].as_str(), Some("queued" | "running")) {
            return Ok(json!({ "job": status["job"] }));
        }
        let now = now_secs();
        if let Some((at, job)) = &self.member_estimate
            && now < at + ESTIMATE_SPACING
            && self.estimates.job_state(job) != Some("failed")
        {
            return Err(EstimateRefused::Limited {
                next_at: iso_secs(at + ESTIMATE_SPACING),
            });
        }
        let mut started = self.start_estimate(EstimateRequest::character(id, req.refresh))?;
        let job = started["job"]["id"].as_str().unwrap_or_default().to_owned();
        if started["job"]["result"]["cache_hit"] != true {
            self.estimate_started_by_member(&job, me);
            started["job"]["started_by"] = json!({"kind": "member", "id": format!("discord:{me}")});
            self.member_estimate = Some((now, job));
        }
        Ok(started)
    }

    /// `GET /api/public/me/estimates?character_id=`: `EstimateStatus`, the
    /// running job only when it is the member's own.
    pub fn my_estimate_status(&self, me: &str, id: &str) -> Result<Value, MoveError> {
        self.own(me, id)?;
        let mut status = self.estimate_status(Some(id), None)?;
        let theirs = self
            .estimates
            .running_id()
            .is_some_and(|running| self.estimates.only_of(running, me));
        if !theirs {
            status["running"] = Value::Null;
        }
        Ok(status)
    }

    /// `GET /api/public/me/estimates/{job}`: only a lookup of the member's
    /// own characters (each read advances it, as the admin's).
    pub fn my_estimate(&mut self, me: &str, job: &str) -> Result<Value, MoveError> {
        if !self.estimates.only_of(job, me) {
            return Err(generic());
        }
        self.estimate(job)
    }

    /// `GET /api/public/runs/{id}/characters`: `RunPartyCharacters`, for a
    /// member on the run; NEXON values on the member's own slot only.
    pub fn party_characters(&self, me: &str, id: &str) -> Result<Value, MoveError> {
        let run = self.run_rec(id)?;
        if !run.participants.iter().any(|p| p.id == me) {
            return Err(MoveError::NotFound);
        }
        let now = now_secs();
        let module = self.character_data_on();
        let estimates = self.estimates_on();
        let mut nexon_data = false;
        let slots: Vec<Value> = run
            .participants
            .iter()
            .map(|p| {
                let you = p.id == me;
                let brought = self
                    .brings(run, p.id)
                    .and_then(|(cid, _)| self.characters.iter().find(|c| c.id == cid));
                let nexon = brought
                    .filter(|_| you && module)
                    .and_then(|c| self.nexon_values(&c.id, now).1)
                    .and_then(|(level, _, as_of)| level.map(|level| (level, as_of)));
                nexon_data |= nexon.is_some();
                let character = brought.map(|c| match &nexon {
                    Some((level, as_of)) => json!({
                        "ign": c.ign, "class": c.class, "level": level,
                        "level_source": "nexon", "as_of": as_of,
                    }),
                    None => json!({
                        "ign": c.ign, "class": c.class, "level": c.level,
                        "level_source": "roster", "as_of": null,
                    }),
                });
                let scouter = brought.filter(|_| estimates).map(|c| {
                    run.bosses
                        .iter()
                        .filter_map(|b| {
                            let mut reading = self.estimates.reading(&c.id, &b.token, now);
                            reading.as_object()?;
                            reading["boss"] = json!(b.key);
                            reading["difficulty"] = json!(b.difficulty);
                            Some(reading)
                        })
                        .collect::<Vec<_>>()
                });
                json!({
                    "member": {
                        "id": snowflake(p.id),
                        "name": self.member_display(p.id).unwrap_or_else(|| p.id.to_owned()),
                    },
                    "you": you,
                    "character": character,
                    "scouter": scouter,
                })
            })
            .collect();
        Ok(json!({
            "run_id": run.id,
            "slots": slots,
            "character_data": module,
            "estimates": estimates,
            "nexon_data": nexon_data,
            "attribution": ATTRIBUTION,
            "estimate_label": LABEL,
            "estimate_note": NOTE,
        }))
    }

    /// `POST /__mock/public/characters {roster}`: Asahi's characters as the
    /// boards draw them, or none.
    pub fn seed_member_characters(&mut self, me: &str, scenario: RosterScenario) {
        let now = now_secs();
        let ids: Vec<String> = self.mine(me).map(|c| c.id.clone()).collect();
        self.links.retain(|link| !ids.contains(&link.character_id));
        self.picks.retain(|pick| !ids.contains(&pick.character));
        self.characters.retain(|c| c.member_id != me);
        self.member_estimate = None;
        if let RosterScenario::Empty = scenario {
            return;
        }
        let batch = now - (7 * 3_600 + 59 * 60);
        let rows = [
            ("ch-b1", "AsahiKanna", "Kanna", 285, true, false),
            ("ch-b2", "AsahiDK", "Dark Knight", 284, false, false),
            ("ch-b3", "AsahiPath", "Pathfinder", 262, false, false),
            ("ch-b4", "AsahiBlaze", "Blaze Wizard", 258, false, false),
            ("ch-b5", "AsahiMech", "Mechanic", 262, false, true),
        ];
        for (id, ign, class, level, main, retired) in rows {
            let req = CharacterCreate {
                ign: ign.into(),
                world: None,
                class: Some(class.into()),
                level: Some(level),
                role: Some(if main { "main" } else { "alt" }.into()),
            };
            if self.insert_character(me, req, false).is_ok()
                && let Some(c) = self.characters.last_mut()
            {
                c.id = id.into();
                if retired {
                    c.retired_at = Some(iso_secs(now - 20 * DAY));
                }
            }
        }
        let mut kanna = linked(
            "ch-b1",
            me,
            now - 10 * DAY,
            facts("AsahiKanna", "Kanna", 286, 51_400_000, batch),
        );
        kanna.cards = vec![("#hstar-party", now - 3 * DAY)];
        self.links.extend([
            kanna,
            Link {
                state: "pending",
                version: 1,
                requested_by: ("admin", "discord:1002".into()),
                requested_at: now - 2 * 3_600,
                linked_at: None,
                consent_surface: None,
                data: None,
                ..linked("ch-b2", me, now, None)
            },
            Link {
                state: "stale",
                ..linked("ch-b3", me, now - 20 * DAY, None)
            },
            linked(
                "ch-b4",
                me,
                now - 40 * DAY,
                facts(
                    "AsahiBlaze",
                    "Blaze Wizard",
                    259,
                    30_120_000,
                    now - 31 * DAY,
                ),
            ),
        ]);
    }
}
