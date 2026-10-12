//! Character data (module `character_data`), as the server's admin API
//! answers it: one character's link and NEXON data, the admin request
//! (pending until the member confirms in Discord, which the mock does not
//! have), unlink and the 24-hour refresh cap. Seeded with wholly invented
//! data: MikaNL linked, AsahiDK pending, everyone else not linked. On while
//! the roster module is on.

use serde::Deserialize;
use serde_json::{Value, json};

use super::clock::{iso_secs, now_secs};
use super::{MoveError, Store};

pub const ATTRIBUTION: &str = "Data based on NEXON Open API";
const DAY: i64 = 86_400;

#[derive(Clone)]
pub struct Link {
    character_id: String,
    /// `pending`, `linked` or `stale`.
    state: &'static str,
    version: u32,
    requested_by: (&'static str, String),
    requested_at: i64,
    linked_at: Option<i64>,
    /// Invented data and when it was fetched.
    data: Option<(Value, i64)>,
}

fn facts(name: &str, class: &str, level: i64, power: i64, fetched: i64) -> (Value, i64) {
    (
        json!({
            "name": name,
            "world": "Aquila",
            "class": class,
            "level": level,
            "combat_power": power,
            "image_url": null,
        }),
        fetched,
    )
}

/// MikaNL (`ch-s2`) linked, refreshed a day ago; AsahiDK (`ch-s5`) waiting
/// for Asahi.
pub fn seed() -> Vec<Link> {
    let now = now_secs();
    vec![
        Link {
            character_id: "ch-s2".into(),
            state: "linked",
            version: 2,
            requested_by: ("admin", "token".into()),
            requested_at: now - 3 * DAY,
            linked_at: Some(now - 2 * DAY),
            data: Some(facts("MikaNL", "Night Lord", 287, 48_213_377, now - DAY)),
        },
        Link {
            character_id: "ch-s5".into(),
            state: "pending",
            version: 1,
            requested_by: ("admin", "token".into()),
            requested_at: now - DAY / 2,
            linked_at: None,
            data: None,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    #[serde(default)]
    version: Option<u32>,
}

fn coded(status: u16, code: &'static str, message: &str) -> MoveError {
    MoveError::Coded(status, code, message.into())
}

fn view(id: &str, link: Option<&Link>, now: i64) -> Value {
    let iso = iso_secs;
    let data = link
        .and_then(|link| link.data.as_ref())
        .filter(|(_, fetched)| fetched + 30 * DAY > now)
        .map(|(data, fetched)| {
            let mut data = data.clone();
            data["as_of"] = json!(iso(*fetched)[..10].to_owned());
            data["fetched_at"] = json!(iso(*fetched));
            data["expires_at"] = json!(iso(fetched + 30 * DAY));
            data
        });
    let refreshable = link
        .and_then(|link| link.data.as_ref())
        .map(|(_, fetched)| fetched + DAY)
        .filter(|at| *at > now);
    json!({
        "character_id": id,
        "state": link.map_or("not_linked", |link| link.state),
        "version": link.map_or(0, |link| link.version),
        "connected": true,
        "requested_by": link.map(|link| json!({"kind": link.requested_by.0, "id": link.requested_by.1})),
        "requested_at": link.map(|link| iso(link.requested_at)),
        "consent_version": link.and_then(|link| link.linked_at).map(|_| 1),
        "consented_at": link.and_then(|link| link.linked_at).map(iso),
        "linked_at": link.and_then(|link| link.linked_at).map(iso),
        "notice_version": 1,
        "data": data,
        "refresh": {
            "refreshable_at": refreshable.map(iso),
            "next_attempt_at": null,
            "last_error": null,
        },
        "attribution": ATTRIBUTION,
    })
}

impl Store {
    fn roster_character(&self, id: &str) -> Result<&super::characters::Char, MoveError> {
        self.characters
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| coded(404, "not_found", "That character is not on the roster."))
    }

    fn link_of(&self, id: &str) -> Option<&Link> {
        self.links.iter().find(|link| link.character_id == id)
    }

    pub fn character_link(&self, id: &str) -> Result<Value, MoveError> {
        self.roster_character(id)?;
        Ok(view(id, self.link_of(id), now_secs()))
    }

    /// `201`: pending; the mock never calls anything.
    pub fn request_link(&mut self, id: &str, req: LinkRequest) -> Result<Value, MoveError> {
        let character = self.roster_character(id)?;
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
            requested_by: ("admin", "token".into()),
            requested_at: now,
            linked_at: None,
            data: None,
        };
        let answer = view(id, Some(&link), now);
        self.links.push(link);
        Ok(answer)
    }

    pub fn unlink(&mut self, id: &str) -> Result<Value, MoveError> {
        let before = self.links.len();
        self.links.retain(|link| link.character_id != id);
        if self.links.len() == before {
            return Err(coded(404, "not_linked", "That character is not linked."));
        }
        Ok(json!({"message": "Unlinked: its character data is deleted."}))
    }

    pub fn refresh_link(&mut self, id: &str) -> Result<Value, MoveError> {
        self.roster_character(id)?;
        let now = now_secs();
        let Some(link) = self
            .links
            .iter_mut()
            .find(|link| link.character_id == id && link.state == "linked")
        else {
            return Err(coded(409, "not_linked", "That character is not linked."));
        };
        if let Some((data, fetched)) = &mut link.data {
            if *fetched + DAY > now {
                return Err(coded(
                    429,
                    "refresh_cap",
                    &format!(
                        "Refreshed less than 24 hours ago; it can refresh again from {}.",
                        iso_secs(*fetched + DAY)
                    ),
                ));
            }
            data["level"] = json!(data["level"].as_i64().unwrap_or(1) + 1);
            *fetched = now;
        }
        Ok(view(id, Some(link), now))
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
}
