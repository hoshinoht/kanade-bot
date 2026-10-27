//! Self-service nudge seed pools. Persona text supplies only the lead-in; code
//! appends the action and link. Rewriting and rotation live with the caller.

use serde::Deserialize;

use super::{
    PersonaError,
    markdown::strip,
    staging::{has_mention, is_line_break, only_fields},
};

pub const MIN_NUDGE_LINES: usize = 3;
pub const MAX_NUDGE_LINES: usize = 20;
pub const MAX_NUDGE_CHARS: usize = 140;
pub const NUDGE_FIELDS: [&str; 3] = ["{boss}", "{day}", "{time}"];

/// `Gentle` after failures or frustration; mood always beats the profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NudgeMood {
    Playful,
    Gentle,
}

/// What the code-appended action does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NudgePurpose {
    /// The member can edit the run: `→ [edit the run](<link>)`.
    SelfService,
    /// A request needs approval: `→ [request a change](<link>)`.
    RequestForm,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMoods {
    #[serde(default)]
    playful: Option<Vec<String>>,
    #[serde(default)]
    gentle: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNudges {
    #[serde(default)]
    playful: Option<Vec<String>>,
    #[serde(default)]
    gentle: Option<Vec<String>>,
    #[serde(default)]
    request_form: Option<RawMoods>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MoodPools {
    pub playful: Option<Vec<String>>,
    pub gentle: Option<Vec<String>>,
}

impl MoodPools {
    pub fn pool(&self, mood: NudgeMood) -> Option<&[String]> {
        match mood {
            NudgeMood::Playful => self.playful.as_deref(),
            NudgeMood::Gentle => self.gentle.as_deref(),
        }
    }

    fn is_empty(&self) -> bool {
        self.playful.is_none() && self.gentle.is_none()
    }
}

/// Top-level pools serve every purpose; `request_form` pools take precedence for requests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Nudges {
    pub general: MoodPools,
    pub request_form: MoodPools,
}

impl Nudges {
    /// The most specific pool this file declares for `purpose` and `mood`.
    pub fn pool(&self, purpose: NudgePurpose, mood: NudgeMood) -> Option<&[String]> {
        let specific = match purpose {
            NudgePurpose::SelfService => None,
            NudgePurpose::RequestForm => self.request_form.pool(mood),
        };
        specific.or_else(|| self.general.pool(mood))
    }
}

/// Seed-line rules shared by persona pools, built-ins and rewrite output checks.
pub fn check_nudge_line(line: &str) -> Result<(), PersonaError> {
    let lower = line.to_lowercase();
    if line.is_empty() || line != strip(line) {
        return Err(PersonaError::Invalid(
            "nudge lines must be non-empty and unpadded",
        ));
    }
    if line.chars().any(is_line_break) {
        return Err(PersonaError::Invalid("nudge lines must be one line"));
    }
    if line.chars().count() > MAX_NUDGE_CHARS {
        return Err(PersonaError::Invalid("nudge line is too long"));
    }
    if has_mention(line) {
        return Err(PersonaError::Invalid("nudge lines must not mention"));
    }
    if lower.contains("://") || lower.contains("www.") || line.contains("](") {
        return Err(PersonaError::Invalid("nudge lines must not carry links"));
    }
    if !only_fields(line, &NUDGE_FIELDS) {
        return Err(PersonaError::Invalid(
            "nudge placeholders are limited to {boss}, {day} and {time}",
        ));
    }
    Ok(())
}

fn pool(lines: Option<Vec<String>>) -> Result<Option<Vec<String>>, PersonaError> {
    let Some(lines) = lines else { return Ok(None) };
    if !(MIN_NUDGE_LINES..=MAX_NUDGE_LINES).contains(&lines.len()) {
        return Err(PersonaError::Invalid("nudge pools hold 3 to 20 lines"));
    }
    for line in &lines {
        check_nudge_line(line)?;
    }
    Ok(Some(lines))
}

fn moods(
    playful: Option<Vec<String>>,
    gentle: Option<Vec<String>>,
) -> Result<MoodPools, PersonaError> {
    Ok(MoodPools {
        playful: pool(playful)?,
        gentle: pool(gentle)?,
    })
}

impl TryFrom<RawNudges> for Nudges {
    type Error = PersonaError;

    fn try_from(raw: RawNudges) -> Result<Self, Self::Error> {
        let general = moods(raw.playful, raw.gentle)?;
        let request_form = match raw.request_form {
            Some(section) => {
                let pools = moods(section.playful, section.gentle)?;
                if pools.is_empty() {
                    return Err(PersonaError::Invalid("nudges.request_form is empty"));
                }
                pools
            }
            None => MoodPools::default(),
        };
        if general.is_empty() && request_form.is_empty() {
            return Err(PersonaError::Invalid("nudges must declare a pool"));
        }
        Ok(Self {
            general,
            request_form,
        })
    }
}

/// Where a compiled seed pool came from; never model-visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NudgeSource {
    Profile,
    Bundle,
    BuiltIn,
}

/// Single-pass literal `{boss}`/`{day}`/`{time}` substitution; inserted values
/// are never rescanned and no general formatter runs.
pub fn fill_nudge(line: &str, boss: &str, day: &str, time: &str) -> String {
    let values = [boss, day, time];
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        match NUDGE_FIELDS
            .iter()
            .position(|field| tail.starts_with(field))
        {
            Some(index) => {
                out.push_str(values[index]);
                rest = &tail[NUDGE_FIELDS[index].len()..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
