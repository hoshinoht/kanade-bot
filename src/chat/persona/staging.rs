//! Strict staging-line rules and literal `{boss}` substitution.

use super::{
    PersonaError,
    markdown::strip,
    schema::{Staging, StagingOverride},
};

/// Per-line budget before substitution, in characters.
pub const MAX_STAGING_CHARS: usize = 200;
/// Budget for a rendered `guide_named` line; longer renders use `guide`.
pub const MAX_RENDERED_STAGING_CHARS: usize = 300;
pub const BOSS_FIELD: &str = "{boss}";

/// Which silent staging line the chat indicator shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingState {
    Schedule,
    Guide,
    Write,
    Generic,
}

fn is_line_break(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')
}

/// Visible Discord mention syntax; mentions stay disabled when sending regardless.
pub(crate) fn has_mention(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("<@")
        || lower.contains("<#")
        || lower.contains("@everyone")
        || lower.contains("@here")
}

/// Braces must form only the allowed literal fields.
pub(crate) fn only_fields(text: &str, allowed: &[&str]) -> bool {
    let mut rest = text.to_owned();
    for field in allowed {
        rest = rest.replace(field, "");
    }
    !rest.contains(['{', '}'])
}

fn check(value: &str, named: bool) -> Result<(), PersonaError> {
    let text = strip(value);
    if text.is_empty() {
        return Err(PersonaError::Invalid("staging lines must be non-empty"));
    }
    if text.chars().any(is_line_break) {
        return Err(PersonaError::Invalid("staging lines must be one line"));
    }
    if text.chars().count() > MAX_STAGING_CHARS {
        return Err(PersonaError::Invalid("staging line is too long"));
    }
    if has_mention(text) {
        return Err(PersonaError::Invalid("staging lines must not mention"));
    }
    let fields = text.matches(BOSS_FIELD).count();
    let allowed: &[&str] = if named { &[BOSS_FIELD] } else { &[] };
    if (named && fields != 1) || !only_fields(text, allowed) {
        return Err(PersonaError::Invalid(
            "only guide_named has exactly one {boss} field; no other braces",
        ));
    }
    Ok(())
}

pub(crate) fn validate(staging: &Staging) -> Result<(), PersonaError> {
    check(&staging.schedule, false)?;
    check(&staging.guide, false)?;
    check(&staging.guide_named, true)?;
    check(&staging.write, false)?;
    check(&staging.generic, false)
}

pub(crate) fn validate_override(staging: &StagingOverride) -> Result<(), PersonaError> {
    let lines = [
        (&staging.schedule, false),
        (&staging.guide, false),
        (&staging.guide_named, true),
        (&staging.write, false),
        (&staging.generic, false),
    ];
    for (line, named) in lines {
        if let Some(line) = line {
            check(line, named)?;
        }
    }
    Ok(())
}

/// Outer whitespace removed, as v4 parses staging.
pub(crate) fn stripped(staging: &Staging) -> Staging {
    let own = |line: &String| strip(line).to_owned();
    Staging {
        schedule: own(&staging.schedule),
        guide: own(&staging.guide),
        guide_named: own(&staging.guide_named),
        write: own(&staging.write),
        generic: own(&staging.generic),
    }
}

impl Staging {
    pub fn line(&self, state: StagingState) -> &str {
        match state {
            StagingState::Schedule => &self.schedule,
            StagingState::Guide => &self.guide,
            StagingState::Write => &self.write,
            StagingState::Generic => &self.generic,
        }
    }

    /// `guide_named` with `boss` substituted literally (no formatter). Unsafe or
    /// empty names and over-budget results fall back to the `guide` line.
    pub fn guide_named_for(&self, boss: &str) -> String {
        let boss = strip(boss);
        let unsafe_name = boss.is_empty() || boss.chars().any(is_line_break) || has_mention(boss);
        if !unsafe_name {
            let rendered = self.guide_named.replacen(BOSS_FIELD, boss, 1);
            if rendered.chars().count() <= MAX_RENDERED_STAGING_CHARS && !has_mention(&rendered) {
                return rendered;
            }
        }
        self.guide.clone()
    }
}
