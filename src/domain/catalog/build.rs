use std::collections::{HashMap, HashSet};

use super::error::BossTableError;
use super::model::{Boss, Difficulty};
use super::spec::{BossSpec, CatalogSpec, DifficultySpec};
use super::table::BossTable;
use super::text::normalise;
use crate::domain::pytext::{list_repr, repr, strip};

impl BossTable {
    /// Validate a decoded catalog.
    ///
    /// Messages match v4 `BossTable.from_dict` for every check a typed spec can
    /// still fail, including duplicates that a YAML map could not express.
    ///
    /// # Errors
    /// [`BossTableError`] naming the first invalid entry.
    pub fn from_spec(spec: &CatalogSpec) -> Result<Self, BossTableError> {
        let difficulties = build_difficulties(&spec.difficulties)?;
        if spec.bosses.is_empty() {
            return Err(BossTableError::new("bosses.yaml has no `bosses:` map"));
        }
        let mut table = Self {
            difficulties,
            bosses: Vec::with_capacity(spec.bosses.len()),
            by_short: HashMap::new(),
            aliases: HashMap::new(),
        };
        for boss_spec in &spec.bosses {
            let boss = build_boss(boss_spec, &table)?;
            table.register(boss)?;
        }
        Ok(table)
    }

    fn register(&mut self, boss: Boss) -> Result<(), BossTableError> {
        let at = self.bosses.len();
        let names = [&boss.short, &boss.full].into_iter().chain(&boss.aliases);
        for name in names {
            let key = normalise(name);
            if key.is_empty() {
                continue;
            }
            if let Some(&owner) = self.aliases.get(&key)
                && owner != at
            {
                return Err(BossTableError::new(format!(
                    "alias {} is claimed by both {} and {}",
                    repr(name),
                    repr(&self.bosses[owner].short),
                    repr(&boss.short)
                )));
            }
            self.aliases.insert(key, at);
        }
        self.by_short.insert(boss.short.clone(), at);
        self.bosses.push(boss);
        Ok(())
    }
}

fn build_difficulties(specs: &[DifficultySpec]) -> Result<Vec<Difficulty>, BossTableError> {
    if specs.is_empty() {
        return Err(BossTableError::new(
            "bosses.yaml has no `difficulties:` map",
        ));
    }
    let mut difficulties: Vec<Difficulty> = Vec::with_capacity(specs.len());
    for spec in specs {
        let prefix = &spec.prefix;
        let mut chars = prefix.chars();
        // Known divergence: Rust's `Alphabetic` also admits letter numbers (Ⅻ),
        // combining marks and circled letters (Ⓐ) that Python's `isalpha` refuses,
        // and follows a newer Unicode. Matching exactly needs a ~300-range table.
        // Typed tokens are ASCII-normalised so cannot reach such a prefix, but
        // spelled difficulty words and stored canonical names can: the only
        // effect is that Rust accepts a catalog v4 would refuse to load.
        let single_letter =
            matches!((chars.next(), chars.next()), (Some(c), None) if c.is_alphabetic());
        if !single_letter {
            return Err(BossTableError::new(format!(
                "difficulty prefix {} must be a single letter",
                repr(prefix)
            )));
        }
        if strip(&spec.label).is_empty() {
            return Err(BossTableError::new(format!(
                "difficulty {} must have a non-empty name",
                repr(prefix)
            )));
        }
        let letter = prefix.to_lowercase();
        if difficulties.iter().any(|d| d.letter == letter) {
            return Err(BossTableError::new(format!(
                "difficulty prefix {} is duplicated",
                repr(prefix)
            )));
        }
        difficulties.push(Difficulty {
            letter,
            label: spec.label.clone(),
        });
    }
    Ok(difficulties)
}

fn build_boss(spec: &BossSpec, table: &BossTable) -> Result<Boss, BossTableError> {
    let short = &spec.short;
    let invalid = |detail: &str| Err(BossTableError::new(format!("{short}{detail}")));
    if strip(short).is_empty() {
        return Err(BossTableError::new(
            "boss short name must be a non-empty string",
        ));
    }
    if table.by_short.contains_key(short) {
        return Err(BossTableError::new(format!(
            "boss short name {} is duplicated",
            repr(short)
        )));
    }
    let full = spec.full.clone().unwrap_or_else(|| short.clone());
    if strip(&full).is_empty() {
        return invalid(".full must be a non-empty string");
    }

    let difficulties: Vec<String> = match &spec.difficulties {
        Some(raw) if raw.is_empty() => return invalid(".difficulties must be a non-empty list"),
        Some(raw) if raw.iter().any(|d| strip(d).is_empty()) => {
            return invalid(".difficulties must contain only non-empty strings");
        }
        Some(raw) => raw.iter().map(|d| d.to_lowercase()).collect(),
        None => table
            .difficulties
            .iter()
            .map(|d| d.letter.clone())
            .collect(),
    };
    if difficulties.iter().collect::<HashSet<_>>().len() != difficulties.len() {
        return invalid(".difficulties must not contain duplicates");
    }
    let unknown: Vec<&String> = difficulties
        .iter()
        .filter(|letter| table.difficulty(letter).is_none())
        .collect();
    if !unknown.is_empty() {
        return invalid(&format!(
            " lists difficulty prefix(es) {} that are not in `difficulties:`",
            list_repr(&unknown)
        ));
    }

    let level = match spec.level {
        None => None,
        Some(level) => match u64::try_from(level) {
            Ok(level) if level > 0 => Some(level),
            _ => return invalid(".level must be a positive integer or null"),
        },
    };
    if spec.aliases.iter().any(|alias| strip(alias).is_empty()) {
        return invalid(".aliases must contain only non-empty strings");
    }
    if let Some(portrait) = &spec.portrait
        && (strip(portrait).is_empty() || !is_basename(portrait))
    {
        return invalid(".portrait must be a non-empty basename");
    }
    let guide_colour = match &spec.guide {
        None => None,
        Some(guide) => match guide.colour {
            None => return invalid(".guide.colour must be an integer"),
            Some(colour) => match u32::try_from(colour) {
                Ok(colour) if colour <= 0xFF_FFFF => Some(colour),
                _ => return invalid(".guide.colour must be between 0 and 0xFFFFFF"),
            },
        },
    };

    Ok(Boss {
        short: short.clone(),
        full,
        level,
        difficulties,
        aliases: spec.aliases.clone(),
        portrait: spec.portrait.clone(),
        guide_colour,
    })
}

/// v4 `Path(p).name == p and "\\" not in p`; like v4 this admits `..`, which
/// only ever resolves to a directory and so never serves a portrait.
fn is_basename(value: &str) -> bool {
    !value.contains(['/', '\\']) && value != "."
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::GuideSpec;

    fn difficulty(prefix: &str, label: &str) -> DifficultySpec {
        DifficultySpec {
            prefix: prefix.into(),
            label: label.into(),
        }
    }

    fn catalog(bosses: Vec<BossSpec>) -> CatalogSpec {
        CatalogSpec {
            difficulties: vec![difficulty("n", "Normal"), difficulty("h", "Hard")],
            bosses,
        }
    }

    fn boss(short: &str) -> BossSpec {
        BossSpec {
            short: short.into(),
            ..BossSpec::default()
        }
    }

    fn error(spec: &CatalogSpec) -> String {
        BossTable::from_spec(spec).unwrap_err().to_string()
    }

    #[test]
    fn catalog_needs_difficulties_and_bosses() {
        assert_eq!(
            error(&CatalogSpec::default()),
            "bosses.yaml has no `difficulties:` map"
        );
        assert_eq!(error(&catalog(vec![])), "bosses.yaml has no `bosses:` map");
    }

    #[test]
    fn difficulty_entries_are_validated() {
        let mut spec = catalog(vec![boss("Star")]);
        spec.difficulties.push(difficulty("hx", "Bad"));
        assert_eq!(
            error(&spec),
            "difficulty prefix 'hx' must be a single letter"
        );
        spec.difficulties.pop();
        spec.difficulties.push(difficulty("H", "Again"));
        assert_eq!(error(&spec), "difficulty prefix 'H' is duplicated");
        spec.difficulties.pop();
        spec.difficulties.push(difficulty("x", "  "));
        assert_eq!(error(&spec), "difficulty 'x' must have a non-empty name");
    }

    #[test]
    fn prefix_letters_use_rust_alphabetic_not_python_isalpha() {
        for prefix in ["é", "Ж", "Ⅻ", "Ⓐ"] {
            let mut spec = catalog(vec![boss("Star")]);
            spec.difficulties.push(difficulty(prefix, "Odd"));
            assert!(BossTable::from_spec(&spec).is_ok(), "{prefix}");
        }
        for prefix in ["1", "_", "²", "\u{301}"] {
            let mut spec = catalog(vec![boss("Star")]);
            spec.difficulties.push(difficulty(prefix, "Odd"));
            assert!(
                error(&spec).ends_with("must be a single letter"),
                "{prefix}"
            );
        }
    }

    #[test]
    fn boss_entries_are_validated() {
        let cases: Vec<(BossSpec, &str)> = vec![
            (boss(" "), "boss short name must be a non-empty string"),
            (
                BossSpec {
                    full: Some(String::new()),
                    ..boss("Star")
                },
                "Star.full must be a non-empty string",
            ),
            (
                BossSpec {
                    difficulties: Some(vec![]),
                    ..boss("Star")
                },
                "Star.difficulties must be a non-empty list",
            ),
            (
                BossSpec {
                    difficulties: Some(vec!["N".into(), "n".into()]),
                    ..boss("Star")
                },
                "Star.difficulties must not contain duplicates",
            ),
            (
                BossSpec {
                    difficulties: Some(vec!["q".into(), "n".into(), "z".into()]),
                    ..boss("Star")
                },
                "Star lists difficulty prefix(es) ['q', 'z'] that are not in `difficulties:`",
            ),
            (
                BossSpec {
                    level: Some(0),
                    ..boss("Star")
                },
                "Star.level must be a positive integer or null",
            ),
            (
                BossSpec {
                    aliases: vec!["".into()],
                    ..boss("Star")
                },
                "Star.aliases must contain only non-empty strings",
            ),
            (
                BossSpec {
                    portrait: Some("a/b.png".into()),
                    ..boss("Star")
                },
                "Star.portrait must be a non-empty basename",
            ),
            (
                BossSpec {
                    guide: Some(GuideSpec::default()),
                    ..boss("Star")
                },
                "Star.guide.colour must be an integer",
            ),
            (
                BossSpec {
                    guide: Some(GuideSpec {
                        colour: Some(0x100_0000),
                    }),
                    ..boss("Star")
                },
                "Star.guide.colour must be between 0 and 0xFFFFFF",
            ),
        ];
        for (spec, expected) in cases {
            assert_eq!(error(&catalog(vec![spec])), expected);
        }
    }

    #[test]
    fn duplicate_names_are_refused() {
        assert_eq!(
            error(&catalog(vec![boss("Star"), boss("Star")])),
            "boss short name 'Star' is duplicated"
        );
        let clash = BossSpec {
            aliases: vec!["s-t-a-r".into()],
            ..boss("Kalos")
        };
        assert_eq!(
            error(&catalog(vec![boss("Star"), clash])),
            "alias 's-t-a-r' is claimed by both 'Star' and 'Kalos'"
        );
    }

    #[test]
    fn defaults_fill_full_name_and_difficulties() {
        let table = BossTable::from_spec(&catalog(vec![boss("Star")])).unwrap();
        let star = table.boss("Star").unwrap();
        assert_eq!(star.full(), "Star");
        assert_eq!(star.difficulties(), ["n", "h"]);
    }
}
