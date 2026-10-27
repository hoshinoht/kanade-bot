//! What one member sees of their own characters (`/character …` in
//! Discord): each roster character with its link and fresh data, read only
//! through [`CharacterData::current_link`] and [`CharacterData::data`], so
//! a link that no longer matches its character is never shown.

use super::CharacterData;
use kanade_domain::characters::Character;
use kanade_domain::chardata::{LINK_REGION, Link, Post, Snapshot};
use kanade_domain::scheduler::StoreError;

/// A link as its member sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberLink {
    pub link: Link,
    /// Fresh data (an active link on an active character only).
    pub snapshot: Option<Snapshot>,
    /// Its tracked share cards.
    pub posts: Vec<Post>,
}

/// One of the member's roster characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCharacter {
    pub character: Character,
    pub link: Option<MemberLink>,
}

impl MemberCharacter {
    /// An active MapleSEA character: one a member may link or share.
    pub fn linkable(&self) -> bool {
        self.character.is_active() && self.character.world == LINK_REGION
    }
}

impl CharacterData {
    /// `member_id`'s roster characters (oldest first, retired and other
    /// regions included), each with its link if it has one.
    pub async fn member_characters(
        &self,
        member_id: &str,
    ) -> Result<Vec<MemberCharacter>, StoreError> {
        let characters = self.roster.member_characters(member_id.to_owned()).await?;
        let mut out = Vec::with_capacity(characters.len());
        for character in characters {
            let link = match self.current_link(&character.id).await? {
                Some((link, _)) => Some(MemberLink {
                    snapshot: self.data(&character.id).await?,
                    posts: self.character_posts(&character.id).await?,
                    link,
                }),
                None => None,
            };
            out.push(MemberCharacter { character, link });
        }
        Ok(out)
    }
}
