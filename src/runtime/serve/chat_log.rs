//! Chat lifecycle log lines ([`ChatEvent`] → JSON). Never question or reply
//! text, never member ids.

use serde_json::json;

use crate::{
    chat::{driver::ChatEvent, persona::ProfileSource},
    infrastructure::llm::identity::IdentityLeakBlocked,
    runtime::logging,
};

/// Why chat cannot answer now, for `chat_setup_changed`.
pub(super) struct Readiness {
    pub model_route: bool,
    pub persona: bool,
}

fn profile_source(source: Option<ProfileSource>) -> (&'static str, bool) {
    match source {
        Some(ProfileSource::MemberSelection) => ("saved", false),
        Some(ProfileSource::RoleAssignment) => ("role", false),
        Some(ProfileSource::BundleDefault {
            saved_selection_unavailable,
        }) => ("default", saved_selection_unavailable),
        None => ("default", false),
    }
}

pub(super) fn observe(event: &ChatEvent<'_>, readiness: impl FnOnce() -> Readiness) {
    match event {
        ChatEvent::Admitted {
            interaction_id,
            thread,
            position,
        } => logging::event(
            "INFO",
            "chat_admitted",
            json!({
                "interaction_id": interaction_id,
                "channel": if *thread { "thread" } else { "channel" },
                "position": position,
            }),
        ),
        ChatEvent::Ignored { reason } => {
            logging::event("INFO", "chat_ignored", json!({"reason": reason}));
        }
        ChatEvent::Cancelled {
            interaction_id,
            reason,
        } => logging::event(
            "INFO",
            "chat_cancelled",
            json!({"interaction_id": interaction_id, "reason": reason}),
        ),
        ChatEvent::Finished {
            interaction,
            generation,
            persona,
            model,
            reasoning,
        } => {
            let failed = generation.failure.is_some();
            if let Some(blocked) = generation.leak_blocked() {
                logging::event("WARN", IdentityLeakBlocked::EVENT, blocked.payload());
            }
            let (source, saved_unavailable) = profile_source(persona.profile_source);
            logging::event(
                if failed { "WARN" } else { "INFO" },
                if failed {
                    "chat_failed"
                } else {
                    "chat_answered"
                },
                json!({
                    "interaction_id": interaction.id,
                    "outcome": interaction.outcome.as_str(),
                    "persona": persona.bundle.to_string(),
                    "profile": persona.profile.as_ref().map(ToString::to_string),
                    "profile_source": source,
                    "saved_style_unavailable": saved_unavailable,
                    "model": model,
                    "reasoning": reasoning.map(|effort| effort.as_str()),
                    "route": generation.route(),
                    "masking": generation.pseudonymized,
                    "rounds": generation.rounds,
                    "tools": generation.tool_calls,
                    "latency_ms": interaction.latency_ms,
                    "model_ms": generation.model_ms,
                    "tools_ms": generation.tools_ms,
                    "clean_retry": generation.clean_retry,
                    "withheld": interaction.withheld,
                }),
            );
        }
        ChatEvent::SetupChanged { enabled, ready } => {
            let mut not_ready = Vec::new();
            if !ready {
                let readiness = readiness();
                if !readiness.model_route {
                    not_ready.push("no_model_route");
                }
                if !readiness.persona {
                    not_ready.push("no_persona");
                }
            }
            logging::event(
                if *enabled && !ready { "WARN" } else { "INFO" },
                "chat_setup_changed",
                json!({"enabled": enabled, "ready": ready, "not_ready": not_ready}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use crate::{
        chat::{
            answer::Generation,
            persona::{CompileProvenance, ExampleSource, PersonaId, ProfileId, VoiceSource},
        },
        domain::model_log::{ChatInteraction, ChatOutcome},
        infrastructure::llm::Effort,
    };

    fn row() -> ChatInteraction {
        ChatInteraction {
            id: "row-1".into(),
            at: Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap(),
            channel_id: Some("50".into()),
            message_id: Some("1001".into()),
            member_id: Some("4242".into()),
            question: "SECRET QUESTION".into(),
            reply: "SECRET REPLY".into(),
            outcome: ChatOutcome::Answered,
            error: None,
            clean_retry: false,
            withheld: false,
            guardrail: json!({}),
            request_count: 2,
            latency_ms: Some(900),
            model_ms: Some(700),
            tools_ms: Some(50),
            prompt_tokens: None,
            completion_tokens: None,
            rounds: Vec::new(),
            persona: None,
            profile: None,
            profile_source: None,
            error_code: None,
        }
    }

    fn provenance() -> CompileProvenance {
        CompileProvenance {
            bundle: PersonaId::parse("kanade").unwrap(),
            profile: Some(ProfileId::parse("gentle").unwrap()),
            profile_source: Some(ProfileSource::MemberSelection),
            bundle_file: None,
            profile_file: None,
            voice: VoiceSource::Profile,
            examples: ExampleSource::None,
        }
    }

    fn unready() -> Readiness {
        Readiness {
            model_route: false,
            persona: true,
        }
    }

    #[test]
    fn chat_answered_names_persona_profile_and_model_without_content() {
        logging::capture();
        let (row, provenance) = (row(), provenance());
        let generation = Generation {
            reply: "SECRET REPLY".into(),
            rounds: 2,
            tool_calls: vec!["list_runs".into()],
            model_ms: 700,
            tools_ms: 50,
            ..Generation::default()
        };
        observe(
            &ChatEvent::Finished {
                interaction: &row,
                generation: &generation,
                persona: &provenance,
                model: "chat-model",
                reasoning: Some(Effort::Low),
            },
            unready,
        );
        let line = logging::captured().remove(0);
        assert_eq!(line["level"], "INFO");
        assert_eq!(line["event"], "chat_answered");
        assert_eq!(line["interaction_id"], "row-1");
        assert_eq!(line["persona"], "kanade");
        assert_eq!(line["profile"], "gentle");
        assert_eq!(line["profile_source"], "saved");
        assert_eq!(line["model"], "chat-model");
        assert_eq!(line["reasoning"], "low");
        assert_eq!(line["route"], "homelab");
        assert_eq!(line["tools"], json!(["list_runs"]));
        let text = line.to_string();
        for private in ["SECRET", "4242", "1001"] {
            assert!(!text.contains(private), "{private} leaked: {text}");
        }
    }

    #[test]
    fn a_masked_refusal_logs_the_payload_route_and_masking_only() {
        use crate::chat::answer::AnswerFailure;
        use crate::infrastructure::llm::governor::{Charge, Role, SessionError, SessionFailure};
        use crate::infrastructure::llm::identity::{IdentityLeakBlocked, LeakKind};
        logging::capture();
        let (row, provenance) = (row(), provenance());
        let blocked = IdentityLeakBlocked {
            role: Role::Chat,
            kinds: vec![LeakKind::Name],
            count: 2,
        };
        let generation = Generation {
            external: true,
            pseudonymized: true,
            failure: Some(AnswerFailure::Session(SessionError {
                failure: SessionFailure::IdentityLeakBlocked(blocked),
                charge: Charge::Refunded,
            })),
            ..Generation::default()
        };
        observe(
            &ChatEvent::Finished {
                interaction: &row,
                generation: &generation,
                persona: &provenance,
                model: "chat-model",
                reasoning: None,
            },
            unready,
        );
        let lines = logging::captured();
        assert_eq!(lines[0]["event"], "identity_leak_blocked");
        assert_eq!(lines[0]["level"], "WARN");
        assert_eq!(lines[0]["role"], "chat");
        assert_eq!(lines[0]["kinds"], json!(["name"]));
        assert_eq!(lines[0]["count"], 2);
        assert_eq!(lines[1]["event"], "chat_failed");
        assert_eq!(lines[1]["route"], "external_masked");
        assert_eq!(lines[1]["masking"], true);
        for line in &lines {
            let text = line.to_string();
            for private in ["SECRET", "4242", "1001"] {
                assert!(!text.contains(private), "{private} leaked: {text}");
            }
        }
    }

    #[test]
    fn setup_changed_says_why_chat_is_not_ready() {
        logging::capture();
        observe(
            &ChatEvent::SetupChanged {
                enabled: true,
                ready: false,
            },
            unready,
        );
        let line = logging::captured().remove(0);
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["event"], "chat_setup_changed");
        assert_eq!(line["not_ready"], json!(["no_model_route"]));
    }
}
