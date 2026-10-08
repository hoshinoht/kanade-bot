//! The boss catalog (names and levels from boss/bosses.yaml) and art lookup.
//! Art is deployment-private: a missing file yields a null URL, never a broken image.

use super::dto::Boss;
use std::path::{Path, PathBuf};

pub const SUFFIXES: [&str; 4] = ["png", "webp", "jpg", "jpeg"];
/// `animated` only; still kinds never serve video.
const VIDEO_SUFFIXES: [&str; 1] = ["mp4"];

struct Def {
    key: &'static str,
    full: &'static str,
    level: u16,
    hue: u16,
    /// Token stem after the difficulty letter (`HStar`, `XBM`).
    short: &'static str,
    difficulties: &'static str,
    aliases: &'static [&'static str],
}

// Names, levels, difficulties and aliases from boss/bosses.yaml.
const BOSSES: [Def; 11] = [
    Def {
        key: "Lotus",
        full: "Lotus",
        level: 260,
        hue: 200,
        short: "Lotus",
        difficulties: "x",
        aliases: &["lotus", "lot", "suu"],
    },
    Def {
        key: "Seren",
        full: "Chosen Seren",
        level: 260,
        hue: 50,
        short: "Seren",
        difficulties: "nhx",
        aliases: &["seren", "serene", "chosenseren"],
    },
    Def {
        key: "Kalos",
        full: "Gatekeeper Kalos",
        level: 265,
        hue: 25,
        short: "Kalos",
        difficulties: "encx",
        aliases: &["kalos", "gatekeeper"],
    },
    Def {
        key: "FA",
        full: "The First Adversary",
        level: 270,
        hue: 205,
        short: "FA",
        difficulties: "enhx",
        aliases: &["fa", "firstadversary", "adversary"],
    },
    Def {
        key: "Carling",
        full: "Carling",
        level: 275,
        hue: 310,
        short: "Carling",
        difficulties: "enhx",
        aliases: &["carling", "carl", "kaling", "karling"],
    },
    Def {
        key: "BM",
        full: "Black Mage",
        level: 275,
        hue: 20,
        short: "BM",
        difficulties: "hx",
        aliases: &["bm", "blackmage", "bmage"],
    },
    Def {
        key: "MaleficStar",
        full: "Radiant Malefic Star",
        level: 280,
        hue: 52,
        short: "Star",
        difficulties: "nh",
        aliases: &["star", "rms", "malefic", "maleficstar"],
    },
    Def {
        key: "Bellona",
        full: "Bellona",
        level: 280,
        hue: 352,
        short: "Bellona",
        difficulties: "enh",
        aliases: &["bellona", "bell", "bella"],
    },
    Def {
        key: "Limbo",
        full: "Limbo",
        level: 285,
        hue: 280,
        short: "Limbo",
        difficulties: "nh",
        aliases: &["limbo", "limb"],
    },
    Def {
        key: "Baldrix",
        full: "Baldrix",
        level: 290,
        hue: 188,
        short: "Baldrix",
        difficulties: "nh",
        aliases: &["baldrix", "bald", "baldrick"],
    },
    Def {
        key: "Jupiter",
        full: "Jupiter",
        level: 295,
        hue: 170,
        short: "Jupiter",
        difficulties: "nh",
        aliases: &["jupiter", "jup"],
    },
];

pub const DIFFICULTY_NAMES: [(char, &str); 5] = [
    ('e', "Easy"),
    ('n', "Normal"),
    ('h', "Hard"),
    ('c', "Chaos"),
    ('x', "Extreme"),
];

/// A boss on a run or timing: token, catalog key, difficulty letter.
#[derive(Clone, PartialEq, Debug)]
pub struct BossRef {
    pub token: String,
    pub key: &'static str,
    pub difficulty: &'static str,
}

fn letter(c: char) -> Option<&'static str> {
    match c {
        'e' => Some("e"),
        'n' => Some("n"),
        'h' => Some("h"),
        'c' => Some("c"),
        'x' => Some("x"),
        _ => None,
    }
}

/// v4's boss parser: `hstar, hfa`, `XKalos + NCarling`. Each word is a
/// difficulty letter then an alias; the difficulty must exist for that boss.
pub fn parse_bosses(text: &str) -> Result<Vec<BossRef>, String> {
    let mut out: Vec<BossRef> = Vec::new();
    for word in text
        .split(|c: char| c == ',' || c == '+' || c.is_whitespace())
        .filter(|w| !w.is_empty())
    {
        let lower = word.to_lowercase();
        let mut chars = lower.chars();
        let first = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        let (Some(diff), Some(def)) = (
            letter(first),
            BOSSES.iter().find(|d| d.aliases.contains(&rest.as_str())),
        ) else {
            return Err(format!(
                "\u{201c}{word}\u{201d} is not a boss: start with e/n/h/c/x, then the boss (hstar, xkalos)."
            ));
        };
        if !def.difficulties.contains(diff) {
            let name = DIFFICULTY_NAMES
                .iter()
                .find(|(c, _)| c.to_string() == diff)
                .map_or(diff, |(_, n)| n);
            return Err(format!("{} has no {name} difficulty.", def.full));
        }
        let boss = BossRef {
            token: format!("{}{}", diff.to_uppercase(), def.short),
            key: def.key,
            difficulty: diff,
        };
        if !out.contains(&boss) {
            out.push(boss);
        }
    }
    if out.is_empty() {
        return Err("Name at least one boss.".into());
    }
    Ok(out)
}

/// Whether a saved run-length override names an exact catalog boss key and
/// one of that boss's supported lowercase difficulty letters.
pub fn valid_run_length_override(key: &str, difficulty: &str) -> bool {
    BOSSES
        .iter()
        .find(|boss| boss.key == key)
        .is_some_and(|boss| boss.difficulties.contains(difficulty))
}

pub fn boss_ref(token: &str) -> Option<BossRef> {
    parse_bosses(token).ok()?.into_iter().next()
}

#[derive(serde::Serialize)]
pub struct DifficultyOption {
    pub letter: &'static str,
    pub name: &'static str,
    pub token: String,
    /// A live weekly timing runs this difficulty.
    pub in_use: bool,
}

#[derive(serde::Serialize)]
pub struct BossRow {
    pub key: &'static str,
    pub name: &'static str,
    pub level: u16,
    pub hue: u16,
    pub portrait: Option<String>,
    pub difficulties: Vec<DifficultyOption>,
}

/// Asset kinds and their directories under the boss root.
#[derive(Clone, Copy)]
pub enum Kind {
    Portrait,
    Icon,
    Entry,
    Animated,
}

impl Kind {
    pub fn parse(segment: &str) -> Option<Self> {
        match segment {
            "portraits" => Some(Self::Portrait),
            "icons" => Some(Self::Icon),
            "entry" => Some(Self::Entry),
            "animated" => Some(Self::Animated),
            _ => None,
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::Portrait => "portraits",
            Self::Icon => "portraits/icon",
            Self::Entry => "artwork/entry",
            Self::Animated => "artwork/animated",
        }
    }

    fn suffixes(self) -> &'static [&'static str] {
        match self {
            Self::Animated => &VIDEO_SUFFIXES,
            _ => &SUFFIXES,
        }
    }

    fn url(self) -> &'static str {
        match self {
            Self::Portrait => "/art/portraits/",
            Self::Icon => "/art/icons/",
            Self::Entry => "/art/entry/",
            Self::Animated => "/art/animated/",
        }
    }
}

#[derive(Clone)]
pub struct Catalog {
    root: PathBuf,
}

impl Catalog {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The file for a boss key, if the deployment has one. Keys are catalog
    /// keys only, so no request path ever reaches the filesystem unchecked.
    pub fn file(&self, kind: Kind, key: &str) -> Option<PathBuf> {
        let def = BOSSES.iter().find(|d| d.key == key)?;
        self.named(kind, def.key)
    }

    /// Art for an event boss, named by its key. The caller has checked that a
    /// knowledge document declares exactly this key; letters and digits only.
    pub fn event_file(&self, kind: Kind, key: &str) -> Option<PathBuf> {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        self.named(kind, key)
    }

    fn named(&self, kind: Kind, basename: &str) -> Option<PathBuf> {
        let dir = self.root.join(kind.dir());
        kind.suffixes()
            .iter()
            .map(|s| dir.join(format!("{basename}.{s}")))
            .find(|p| p.is_file())
    }

    pub fn url(&self, kind: Kind, key: &str) -> Option<String> {
        self.file(kind, key).map(|_| format!("{}{key}", kind.url()))
    }

    /// An event boss's art URL, named by its key.
    pub fn event_url(&self, kind: Kind, key: &str) -> Option<String> {
        self.event_file(kind, key)
            .map(|_| format!("{}{key}", kind.url()))
    }

    /// An event boss's `(portrait, portrait_sm, art)` URLs, as on `Boss`.
    pub fn event_art(&self, key: &str) -> (Option<String>, Option<String>, Option<String>) {
        let url = |kind: Kind| self.event_url(kind, key);
        let portrait = url(Kind::Portrait);
        (
            portrait.clone(),
            url(Kind::Icon).or(portrait),
            url(Kind::Entry),
        )
    }

    /// The in-game list in level order, with what the guild's timings use ticked.
    pub fn rows(&self, in_use: &[String]) -> Vec<BossRow> {
        BOSSES
            .iter()
            .map(|d| BossRow {
                key: d.key,
                name: d.full,
                level: d.level,
                hue: d.hue,
                portrait: self.url(Kind::Portrait, d.key),
                difficulties: d
                    .difficulties
                    .chars()
                    .filter_map(|c| {
                        let (letter_, name) =
                            DIFFICULTY_NAMES.iter().copied().find(|(l, _)| *l == c)?;
                        let token = format!("{}{}", letter_.to_ascii_uppercase(), d.short);
                        Some(DifficultyOption {
                            letter: letter(letter_)?,
                            name,
                            in_use: in_use.contains(&token),
                            token,
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn boss(&self, token: &str, key: &str, difficulty: &'static str) -> Boss {
        let def = BOSSES.iter().find(|d| d.key == key);
        let portrait = self.url(Kind::Portrait, key);
        Boss {
            token: token.to_owned(),
            key: key.to_owned(),
            name: def.map_or(key, |d| d.full).to_owned(),
            difficulty,
            level: def.map(|d| d.level),
            portrait_sm: self.url(Kind::Icon, key).or_else(|| portrait.clone()),
            portrait,
            art: self.url(Kind::Entry, key),
            animated: self.url(Kind::Animated, key),
            hue: def.map_or(0, |d| d.hue),
        }
    }
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("webp") => "image/webp",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("gif") => "image/gif",
        Some("mp4") => "video/mp4",
        _ => "image/png",
    }
}
