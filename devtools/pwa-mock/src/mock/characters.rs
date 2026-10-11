//! The manual character roster (module `roster`), as the server's admin
//! routes answer it (`src/api/admin/characters/`): IGNs 4-12 letters or
//! digits, unique per world case-insensitively; a member's first active
//! character is the main, a new main takes the crown, and the main stays
//! while others are active. No seed: the list starts empty.

use serde::Deserialize;
use serde_json::{Value, json};

use super::clock::iso_now;
use super::{MoveError, Store};

#[derive(Clone)]
pub struct Char {
    id: String,
    member_id: String,
    ign: String,
    world: String,
    class: Option<String>,
    level: Option<i64>,
    main: bool,
    retired_at: Option<String>,
    created_at: String,
    updated_at: String,
    version: u64,
}

impl Char {
    fn active_main(&self) -> bool {
        self.main && self.retired_at.is_none()
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id, "member_id": self.member_id, "ign": self.ign, "world": self.world,
            "class": self.class, "level": self.level,
            "role": if self.main { "main" } else { "alt" },
            "retired_at": self.retired_at, "created_at": self.created_at,
            "updated_at": self.updated_at,
            "updated_by": { "kind": "admin", "id": "discord:1001" },
            "version": self.version,
        })
    }

    fn bump(&mut self) {
        self.version += 1;
        self.updated_at = iso_now();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterCreate {
    ign: String,
    #[serde(default)]
    world: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    level: Option<i64>,
    #[serde(default)]
    role: Option<String>,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterPatch {
    #[serde(default)]
    version: Option<u64>,
    #[serde(default)]
    ign: Option<String>,
    #[serde(default)]
    world: Option<String>,
    #[serde(default, deserialize_with = "present")]
    class: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    level: Option<Option<i64>>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    retired: Option<bool>,
}

fn invalid(message: &str) -> MoveError {
    MoveError::Coded(422, "invalid", message.into())
}

fn ign(text: &str) -> Result<String, MoveError> {
    let ign = text.trim();
    ((4..=12).contains(&ign.len()) && ign.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| ign.to_owned())
        .ok_or_else(|| invalid("An IGN is 4-12 letters or digits."))
}

fn world(text: Option<&str>) -> Result<String, MoveError> {
    let world = text.unwrap_or("").trim().to_ascii_lowercase();
    if world.is_empty() {
        return Ok("msea".into());
    }
    (world.len() <= 16
        && world
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
    .then_some(world)
    .ok_or_else(|| invalid("A world is 1-16 lowercase letters, digits or '-'."))
}

fn class(text: Option<&str>) -> Result<Option<String>, MoveError> {
    let class = text.unwrap_or("").trim();
    if class.is_empty() {
        return Ok(None);
    }
    (class.chars().count() <= 32)
        .then(|| Some(class.to_owned()))
        .ok_or_else(|| invalid("A class is at most 32 characters."))
}

fn level(level: i64) -> Result<i64, MoveError> {
    (1..=300)
        .contains(&level)
        .then_some(level)
        .ok_or_else(|| invalid("A level is 1-300."))
}

fn role(text: &str) -> Result<bool, MoveError> {
    match text {
        "main" => Ok(true),
        "alt" => Ok(false),
        _ => Err(invalid("A role is main or alt.")),
    }
}

fn main_required() -> MoveError {
    MoveError::Coded(
        409,
        "main_required",
        "A member with other active characters keeps a main: make another character the main first."
            .into(),
    )
}

fn not_found() -> MoveError {
    MoveError::Coded(
        404,
        "not_found",
        "That character is not on the roster.".into(),
    )
}

impl Store {
    fn has_member(&self, id: &str) -> Result<(), MoveError> {
        self.members
            .iter()
            .any(|m| m.seed.id == id)
            .then_some(())
            .ok_or_else(|| {
                MoveError::Coded(404, "not_found", "That member is not on the roster.".into())
            })
    }

    fn ign_taken(&self, id: &str, world: &str, ign: &str) -> bool {
        self.characters
            .iter()
            .any(|c| c.id != id && c.world == world && c.ign.eq_ignore_ascii_case(ign))
    }

    /// The member's other active main, demoted to an alt.
    fn demote_main(&mut self, member: &str, except: &str) {
        for c in &mut self.characters {
            if c.member_id == member && c.id != except && c.active_main() {
                c.main = false;
                c.bump();
            }
        }
    }

    pub fn characters_of(&self, member: &str) -> Result<Value, MoveError> {
        self.has_member(member)?;
        let rows: Vec<Value> = self
            .characters
            .iter()
            .filter(|c| c.member_id == member)
            .map(Char::json)
            .collect();
        Ok(json!({ "member_id": member, "characters": rows }))
    }

    pub fn create_character(
        &mut self,
        member: &str,
        req: CharacterCreate,
    ) -> Result<Value, MoveError> {
        let ign = ign(&req.ign)?;
        let world = world(req.world.as_deref())?;
        let class = class(req.class.as_deref())?;
        let level = req.level.map(level).transpose()?;
        let asked = req.role.as_deref().map(role).transpose()?;
        self.has_member(member)?;
        if self.ign_taken("", &world, &ign) {
            return Err(MoveError::Coded(
                422,
                "ign_taken",
                "That IGN is already on the roster in that world.".into(),
            ));
        }
        let has_main = self
            .characters
            .iter()
            .any(|c| c.member_id == member && c.active_main());
        let main = !has_main || asked.unwrap_or(false);
        self.next_id += 1;
        let id = format!("ch-{}", self.next_id);
        if main {
            self.demote_main(member, &id);
        }
        let now = iso_now();
        let row = Char {
            id,
            member_id: member.into(),
            ign,
            world,
            class,
            level,
            main,
            retired_at: None,
            created_at: now.clone(),
            updated_at: now,
            version: 1,
        };
        let out = row.json();
        self.characters.push(row);
        Ok(out)
    }

    pub fn patch_character(&mut self, id: &str, req: CharacterPatch) -> Result<Value, MoveError> {
        let version = req.version.ok_or_else(|| {
            MoveError::Coded(
                422,
                "version_required",
                "Send the version the character was loaded at.".into(),
            )
        })?;
        let new_ign = req.ign.as_deref().map(ign).transpose()?;
        let new_world = req.world.as_deref().map(|w| world(Some(w))).transpose()?;
        let new_class = req.class.map(|c| class(c.as_deref())).transpose()?;
        let new_level = req.level.map(|l| l.map(level).transpose()).transpose()?;
        let new_role = req.role.as_deref().map(role).transpose()?;
        let index = self
            .characters
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(not_found)?;
        let before = self.characters[index].clone();
        if before.version != version {
            return Err(MoveError::Stale);
        }
        let mut after = before.clone();
        if let Some(ign) = new_ign {
            after.ign = ign;
        }
        if let Some(world) = new_world {
            after.world = world;
        }
        if let Some(class) = new_class {
            after.class = class;
        }
        if let Some(level) = new_level {
            after.level = level;
        }
        match req.retired {
            Some(true) if after.retired_at.is_none() => after.retired_at = Some(iso_now()),
            Some(false) => after.retired_at = None,
            _ => {}
        }
        let others: Vec<&Char> = self
            .characters
            .iter()
            .filter(|c| c.member_id == before.member_id && c.id != id)
            .collect();
        let other_main = others.iter().any(|c| c.active_main());
        let others_active = others.iter().any(|c| c.retired_at.is_none());
        match new_role {
            Some(main) => after.main = main,
            None if after.retired_at.is_none() => after.main = !other_main,
            None => {}
        }
        if self.ign_taken(id, &after.world, &after.ign) {
            return Err(MoveError::Coded(
                422,
                "ign_taken",
                "That IGN is already on the roster in that world.".into(),
            ));
        }
        let active = after.retired_at.is_none();
        if (active && !after.main && !other_main)
            || (before.active_main() && !after.active_main() && others_active)
        {
            return Err(main_required());
        }
        let changed = (
            &after.ign,
            &after.world,
            &after.class,
            after.level,
            after.main,
            &after.retired_at,
        ) != (
            &before.ign,
            &before.world,
            &before.class,
            before.level,
            before.main,
            &before.retired_at,
        );
        if changed {
            if after.active_main() {
                self.demote_main(&before.member_id, id);
            }
            after.bump();
            self.characters[index] = after.clone();
        }
        Ok(after.json())
    }

    pub fn delete_character(&mut self, id: &str) -> Result<Value, MoveError> {
        let row = self
            .characters
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(not_found)?;
        let others_active = self
            .characters
            .iter()
            .any(|c| c.member_id == row.member_id && c.id != id && c.retired_at.is_none());
        if row.active_main() && others_active {
            return Err(main_required());
        }
        self.characters.retain(|c| c.id != id);
        Ok(json!({ "message": format!("{} is deleted.", row.ign) }))
    }
}
