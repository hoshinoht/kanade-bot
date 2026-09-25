//! Optional fixed persona lines for answers the model could not give (user
//! decision 2026-09-25: blocked content gets a fixed persona line, never the
//! provider's refusal text). Missing lines fall back to code-owned text.

use serde::Deserialize;

use super::{
    PersonaError,
    markdown::strip,
    staging::{has_mention, is_line_break},
};

pub const MAX_FAILURE_CHARS: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFailures {
    #[serde(default)]
    content_blocked: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Failures {
    /// Said when the provider's content filter blocked the answer.
    pub content_blocked: Option<String>,
}

/// One line, unpadded, at most [`MAX_FAILURE_CHARS`], with no mentions,
/// links or placeholders: it is posted verbatim.
pub fn check_failure_line(line: &str) -> Result<(), PersonaError> {
    if line.is_empty() || line != strip(line) {
        return Err(PersonaError::Invalid(
            "failure lines must be non-empty and unpadded",
        ));
    }
    if line.chars().any(is_line_break) {
        return Err(PersonaError::Invalid("failure lines must be one line"));
    }
    if line.chars().count() > MAX_FAILURE_CHARS {
        return Err(PersonaError::Invalid("failure line is too long"));
    }
    if has_mention(line) {
        return Err(PersonaError::Invalid("failure lines must not mention"));
    }
    let lower = line.to_lowercase();
    if lower.contains("://") || lower.contains("www.") || line.contains("](") {
        return Err(PersonaError::Invalid("failure lines must not carry links"));
    }
    if line.contains(['{', '}']) {
        return Err(PersonaError::Invalid("failure lines take no placeholders"));
    }
    Ok(())
}

impl TryFrom<RawFailures> for Failures {
    type Error = PersonaError;

    fn try_from(raw: RawFailures) -> Result<Self, PersonaError> {
        let Some(line) = raw.content_blocked else {
            return Err(PersonaError::Invalid("failures must declare a line"));
        };
        check_failure_line(&line)?;
        Ok(Self {
            content_blocked: Some(line),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CompiledPersona, PersonaId, parse_bundle};
    use super::*;

    fn bundle(failures: &str) -> Result<super::super::Bundle, PersonaError> {
        let text = format!(
            "schema_version: 1\nid: demo\nidentity: I am Demo.\nbehaviour:\n  prompt: Be kind.\nstaging:\n  schedule: s\n  guide: g\n  guide_named: 'about {{boss}}'\n  write: w\n  generic: x\n{failures}"
        );
        parse_bundle(&text, &PersonaId::parse("demo").unwrap())
    }

    #[test]
    fn the_content_blocked_line_is_optional_and_compiled() {
        let plain = bundle("").unwrap();
        assert_eq!(plain.failures, None);
        assert_eq!(
            CompiledPersona::compile(&plain, None).content_blocked_line(),
            None
        );
        let declared = bundle("failures:\n  content_blocked: Nope, not that one.\n").unwrap();
        assert_eq!(
            CompiledPersona::compile(&declared, None).content_blocked_line(),
            Some("Nope, not that one.")
        );
    }

    #[test]
    fn unsafe_or_empty_lines_are_refused() {
        for bad in [
            "failures: {}\n",
            "failures:\n  content_blocked: ''\n",
            "failures:\n  content_blocked: ' padded'\n",
            "failures:\n  content_blocked: 'hi <@123>'\n",
            "failures:\n  content_blocked: 'see https://x.y'\n",
            "failures:\n  content_blocked: 'about {boss}'\n",
            "failures:\n  content_blocked: \"two\\nlines\"\n",
            "failures:\n  content_blocked: x\n  other: y\n",
        ] {
            assert!(bundle(bad).is_err(), "{bad}");
        }
        let long = format!(
            "failures:\n  content_blocked: '{}'\n",
            "a".repeat(MAX_FAILURE_CHARS + 1)
        );
        assert!(bundle(&long).is_err());
    }
}
