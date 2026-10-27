//! Chat lifecycle log lines ([`ChatEvent`] → JSON). Never question or reply
//! text, never member ids.

use kanade_chat::driver::ChatEvent;
use kanade_voice::persona::ProfileSource;

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
        } => kanade_log::log!(
            info,
            "chat_admitted",
            interaction_id,
            channel = if *thread { "thread" } else { "channel" },
            position,
        ),
        ChatEvent::Ignored { reason } => {
            kanade_log::log!(info, "chat_ignored", reason);
        }
        ChatEvent::Cancelled {
            interaction_id,
            reason,
        } => kanade_log::log!(info, "chat_cancelled", interaction_id, reason),
        ChatEvent::Finished {
            interaction,
            generation,
            persona,
            model,
            reasoning,
        } => {
            let failed = generation.failure.is_some();
            let (source, saved_unavailable) = profile_source(persona.profile_source);
            kanade_log::log!(
                if failed { "WARN" } else { "INFO" },
                if failed {
                    "chat_failed"
                } else {
                    "chat_answered"
                },
                interaction_id = interaction.id,
                outcome = interaction.outcome.as_str(),
                persona = persona.bundle.to_string(),
                profile = persona.profile.as_ref().map(ToString::to_string),
                profile_source = source,
                saved_style_unavailable = saved_unavailable,
                model,
                reasoning = reasoning.map(|effort| effort.as_str()),
                route = generation.route(),
                rounds = generation.rounds,
                tools = generation.tool_calls,
                latency_ms = interaction.latency_ms,
                model_ms = generation.model_ms,
                tools_ms = generation.tools_ms,
                clean_retry = generation.clean_retry,
                withheld = interaction.withheld,
                // The gateway's own log keys: correlation ids only. The
                // session's full list, so a failed request is named too.
                session_id = interaction.session_id,
                request_ids = generation.request_ids,
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
            kanade_log::log!(
                if *enabled && !ready { "WARN" } else { "INFO" },
                "chat_setup_changed",
                enabled,
                ready,
                not_ready,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;
    use kanade_chat::answer::Generation;
    use kanade_domain::model_log::{ChatInteraction, ChatOutcome};
    use kanade_llm::Effort;
    use kanade_voice::persona::{
        CompileProvenance, ExampleSource, PersonaId, ProfileId, VoiceSource,
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
            cached_tokens: None,
            rounds: Vec::new(),
            persona: None,
            profile: None,
            profile_source: None,
            error_code: None,
            session_id: None,
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
        kanade_log::capture();
        let (mut row, provenance) = (row(), provenance());
        row.session_id = Some("kanade-chat-0000abcd-9".into());
        row.rounds = vec![kanade_domain::model_log::ChatRound {
            model: "chat-model".into(),
            reasoning: None,
            reasoning_content: None,
            reasoning_tokens: None,
            finish_reason: Some("stop".into()),
            latency_ms: Some(700),
            tool_bundles: Vec::new(),
            tools: Vec::new(),
            tool_calls: json!([]),
            response: Some("SECRET REPLY".into()),
            route: None,
            clean: false,
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            prompt_estimate: None,
            request_ids: vec![
                "kanade-chat-0000abcd-9-1".into(),
                "kanade-chat-0000abcd-9-2".into(),
            ],
        }];
        let generation = Generation {
            reply: "SECRET REPLY".into(),
            rounds: 2,
            tool_calls: vec!["list_runs".into()],
            model_ms: 700,
            tools_ms: 50,
            request_ids: row.rounds[0].request_ids.clone(),
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
        let line = kanade_log::captured().remove(0);
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
        assert_eq!(line["session_id"], "kanade-chat-0000abcd-9");
        assert_eq!(
            line["request_ids"],
            json!(["kanade-chat-0000abcd-9-1", "kanade-chat-0000abcd-9-2"])
        );
        let text = line.to_string();
        for private in ["SECRET", "4242", "1001"] {
            assert!(!text.contains(private), "{private} leaked: {text}");
        }
    }

    #[test]
    fn an_external_failure_is_labeled_unmasked() {
        use kanade_chat::answer::AnswerFailure;
        kanade_log::capture();
        let (mut row, provenance) = (row(), provenance());
        // The failed request answered no round: only the session list names it.
        row.session_id = Some("kanade-chat-0000abcd-a".into());
        let generation = Generation {
            external: true,
            external_unmasked: true,
            failure: Some(AnswerFailure::Malformed),
            request_ids: vec!["kanade-chat-0000abcd-a-1".into()],
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
        let line = kanade_log::captured().remove(0);
        assert_eq!(line["event"], "chat_failed");
        assert_eq!(line["route"], "external_unmasked");
        assert_eq!(line["session_id"], "kanade-chat-0000abcd-a");
        assert_eq!(line["request_ids"], json!(["kanade-chat-0000abcd-a-1"]));
        assert!(!line.as_object().unwrap().contains_key("masking"));
        let text = line.to_string();
        for private in ["SECRET", "4242", "1001"] {
            assert!(!text.contains(private), "{private} leaked: {text}");
        }
    }

    #[test]
    fn setup_changed_says_why_chat_is_not_ready() {
        kanade_log::capture();
        observe(
            &ChatEvent::SetupChanged {
                enabled: true,
                ready: false,
            },
            unready,
        );
        let line = kanade_log::captured().remove(0);
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["event"], "chat_setup_changed");
        assert_eq!(line["not_ready"], json!(["no_model_route"]));
    }
}
