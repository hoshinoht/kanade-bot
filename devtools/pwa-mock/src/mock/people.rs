//! Members (v4 /members): the bossing roster plus members with chatbot access.

use super::dto::*;
use super::seed::PERSONAS;
use super::{MoveError, Store};

impl Store {
    /// A seeded member's name, for their portrait's monogram.
    pub fn member_display(&self, id: &str) -> Option<String> {
        self.members
            .iter()
            .find(|m| m.seed.id == id)
            .map(|m| m.seed.name.to_owned())
    }

    fn member_row(&self, index: usize) -> MemberRow {
        let m = &self.members[index];
        MemberRow {
            id: m.seed.id,
            name: m.seed.name,
            nickname: m.seed.nickname,
            aliases: m.aliases.clone(),
            runs_this_week: self
                .runs
                .iter()
                .filter(|r| !r.next_week && r.participants.iter().any(|p| p.id == m.seed.id))
                .count(),
            ping_level: m.ping_level,
            persona: m.persona,
            persona_available: m
                .persona
                .is_none_or(|p| PERSONAS.iter().any(|(k, _)| *k == p)),
            bossing: m.seed.bossing,
            access: m.seed.access,
        }
    }

    pub fn member_rows(&self) -> Vec<MemberRow> {
        (0..self.members.len())
            .map(|i| self.member_row(i))
            .collect()
    }

    pub fn personas() -> Vec<Persona> {
        PERSONAS
            .iter()
            .map(|&(key, name)| Persona {
                key,
                name,
                voice: super::config::profile_label_voice(key).map_or("", |(_, voice)| voice),
            })
            .collect()
    }

    fn member_index(&self, id: &str) -> Result<usize, MoveError> {
        self.members
            .iter()
            .position(|m| m.seed.id == id)
            .ok_or(MoveError::NotFound)
    }

    pub fn patch_member(&mut self, id: &str, patch: MemberPatch) -> Result<MemberRow, MoveError> {
        let index = self.member_index(id)?;
        if let Some(level) = patch.ping_level.as_deref() {
            self.members[index].ping_level = match level {
                "essential" => "essential",
                "all" => "all",
                "off" => "off",
                _ => return Err(MoveError::invalid("Ping level is essential, all or off.")),
            };
        }
        if let Some(persona) = patch.persona.as_deref() {
            self.members[index].persona = match persona {
                "" => None,
                key => Some(
                    PERSONAS
                        .iter()
                        .find(|(k, _)| *k == key)
                        .ok_or(MoveError::invalid("No such reply style."))?
                        .0,
                ),
            };
        }
        Ok(self.member_row(index))
    }

    pub fn add_alias(&mut self, id: &str, alias: &str) -> Result<MemberRow, MoveError> {
        let index = self.member_index(id)?;
        let alias = alias.trim().to_lowercase();
        if alias.is_empty()
            || alias.len() > 32
            || !alias
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(MoveError::invalid(
                "An alias is one word: letters, digits, - or _.",
            ));
        }
        if self.members.iter().any(|m| m.aliases.contains(&alias)) {
            return Err(MoveError::Invalid(format!(
                "\u{201c}{alias}\u{201d} already names someone."
            )));
        }
        self.members[index].aliases.push(alias);
        Ok(self.member_row(index))
    }

    /// Idempotent, like the server: an alias not held leaves the row as it is.
    pub fn remove_alias(&mut self, id: &str, alias: &str) -> Result<MemberRow, MoveError> {
        let index = self.member_index(id)?;
        let alias = alias.trim().to_lowercase();
        self.members[index].aliases.retain(|held| *held != alias);
        Ok(self.member_row(index))
    }
}
