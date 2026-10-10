//! The startup persona selection as one operator log line (ids, file
//! basenames and error classes only; never persona text).

use serde_json::{Value, json};

use crate::{
    chat::persona::{PersonaSnapshot, ProfileSet, Provenance, SelectionSource},
    runtime::logging,
};

fn source(source: SelectionSource) -> &'static str {
    match source {
        SelectionSource::Configured => "configured",
        SelectionSource::CatalogDefault => "catalog_default",
        SelectionSource::TrackedFallback => "tracked_fallback",
    }
}

pub fn persona_selected(snapshot: &PersonaSnapshot) {
    let profiles = snapshot.active().map(|active| &active.profiles);
    emit(snapshot.provenance(), profiles);
}

fn emit(provenance: &Provenance, profiles: Option<&ProfileSet>) {
    let issues: Vec<Value> = provenance
        .issues
        .iter()
        .map(|issue| {
            json!({
                "candidate": issue.candidate.map_or("catalog", source),
                "error": issue.error.to_string(),
            })
        })
        .collect();
    let Some(effective) = &provenance.effective else {
        logging::event(
            "ERROR",
            "persona_unavailable",
            json!({
                "configured": provenance.configured.as_ref().map(ToString::to_string),
                "issues": issues,
            }),
        );
        return;
    };
    let profile_ids: Vec<String> = profiles.map_or_else(Vec::new, |set| {
        set.readable.keys().map(ToString::to_string).collect()
    });
    let unreadable: Vec<Value> = profiles.map_or_else(Vec::new, |set| {
        set.unreadable
            .iter()
            .map(|issue| json!({"file": issue.basename, "error": issue.error.to_string()}))
            .collect()
    });
    let fallback = provenance.source != Some(SelectionSource::Configured);
    let warn = fallback
        || !issues.is_empty()
        || !unreadable.is_empty()
        || provenance.profiles_issue.is_some();
    logging::event(
        if warn { "WARN" } else { "INFO" },
        "persona_selected",
        json!({
            "configured": provenance.configured.as_ref().map(ToString::to_string),
            "effective": effective.to_string(),
            "source": provenance.source.map(source),
            "bundle_file": provenance.bundle.as_ref().map(|bundle| bundle.basename.clone()),
            "profiles": profile_ids.len(),
            "profile_ids": profile_ids,
            "unreadable_profiles": unreadable,
            "profiles_issue": provenance.profiles_issue.as_ref().map(ToString::to_string),
            "issues": issues,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::persona::{CandidateIssue, PersonaError, PersonaId, Source};

    fn id(text: &str) -> PersonaId {
        PersonaId::parse(text).expect("id")
    }

    #[test]
    fn fallback_and_profiles_issue_warn_with_fields() {
        logging::capture();
        let provenance = Provenance {
            configured: Some(id("aria")),
            effective: Some(id("kanade")),
            source: Some(SelectionSource::CatalogDefault),
            bundle: Some(Source {
                basename: "kanade.yaml".into(),
                sha256: "00".into(),
            }),
            issues: vec![CandidateIssue {
                candidate: Some(SelectionSource::Configured),
                error: PersonaError::Missing,
            }],
            profiles_issue: Some(PersonaError::Symlink),
        };
        emit(&provenance, Some(&ProfileSet::default()));
        let lines = logging::captured();
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["event"], "persona_selected");
        assert_eq!(line["configured"], "aria");
        assert_eq!(line["effective"], "kanade");
        assert_eq!(line["source"], "catalog_default");
        assert_eq!(line["bundle_file"], "kanade.yaml");
        assert_eq!(line["profiles"], 0);
        assert_eq!(line["issues"][0]["candidate"], "configured");
        assert_eq!(line["issues"][0]["error"], "persona file is missing");
        assert!(line["profiles_issue"].is_string());
    }

    #[test]
    fn configured_selection_is_info() {
        logging::capture();
        let provenance = Provenance {
            configured: Some(id("kanade")),
            effective: Some(id("kanade")),
            source: Some(SelectionSource::Configured),
            bundle: None,
            issues: Vec::new(),
            profiles_issue: None,
        };
        emit(&provenance, Some(&ProfileSet::default()));
        assert_eq!(logging::captured()[0]["level"], "INFO");
    }

    #[test]
    fn nothing_valid_is_persona_unavailable() {
        logging::capture();
        let provenance = Provenance {
            configured: None,
            effective: None,
            source: None,
            bundle: None,
            issues: vec![CandidateIssue {
                candidate: None,
                error: PersonaError::Missing,
            }],
            profiles_issue: None,
        };
        emit(&provenance, None);
        let line = &logging::captured()[0];
        assert_eq!(line["level"], "ERROR");
        assert_eq!(line["event"], "persona_unavailable");
        assert_eq!(line["issues"][0]["candidate"], "catalog");
    }
}
