use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    CompletionResponse, Effort, ExecutionLimits, FakeAction, FakeProvider, FinishReason, Message,
    RetryPolicy, Usage,
    governor::{ModelClient, Random, Role, RoleRoute, XorShift},
    identity::{
        BotIdentity, CodeLexicon, NamePool, PseudonymCodec, PseudonymConfig, ScanExemptions,
    },
    setup::{Models, build},
};
use serde_json::{json, to_string};

use super::{cases, probe, report::AggregateReport, scoring};

const ALIAS: &str = "synthetic-strong-model";

fn response(content: &str) -> CompletionResponse {
    CompletionResponse {
        model: ALIAS.into(),
        content: Some(content.into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 17,
            completion_tokens: 9,
        }),
    }
}

fn fake_client(
    actions: impl IntoIterator<Item = FakeAction>,
) -> (
    ModelClient<FakeProvider>,
    Arc<FakeProvider>,
    RoleRoute,
    Arc<kanade::infrastructure::llm::governor::Governor>,
) {
    let _ = kanade::runtime::tls::install_ring_provider();
    let random: Arc<dyn Random> = Arc::new(XorShift::new(7));
    let Models::Ready(stack) = build(
        super::model_setup(
            "http://127.0.0.1:1".into(),
            ALIAS.into(),
            b"offline-test-key".to_vec(),
        ),
        Arc::clone(&random),
    )
    .unwrap() else {
        panic!("models unavailable");
    };
    assert!(stack.client.masking());
    let governor = Arc::clone(&stack.governor);
    let route = governor.route(Role::Rewrite).unwrap();
    assert!(route.external);
    assert!(!route.unmasked_allowed);
    let provider = Arc::new(FakeProvider::new(actions));
    let client = ModelClient::new(
        Arc::clone(&governor),
        Arc::clone(&provider),
        ExecutionLimits::default(),
        RetryPolicy::default(),
    )
    .unwrap()
    .with_masking(true);
    (client, provider, route, governor)
}

fn codec(boss_alias: &str) -> PseudonymCodec {
    let schema = probe::output_schema();
    let exemptions = ScanExemptions::default()
        .with_texts([probe::AUDIT_PROMPT, probe::TASK_PROMPT, "propose_add"])
        .with_value(&schema.schema);
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin().with_terms([boss_alias]),
        bot: BotIdentity::default(),
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(17)),
    })
    .with_scan_exemptions(&exemptions)
}

#[tokio::test(start_paused = true)]
async fn live_governor_fake_paces_four_masked_cases_and_redacts_the_aggregate() {
    let guesses = json!({
        "guesses": ["Éowyn Star", "11", "invented decoy", "HFA"],
        "quasi_identifier_categories": ["schedule_timing", "boss_or_game_context"]
    })
    .to_string();
    let evaluation = cases::load().unwrap();
    let actions = (0..evaluation.cases.len()).map(|_| FakeAction::Response(response(&guesses)));
    let (client, provider, route, governor) = fake_client(actions);
    let codec = codec(&evaluation.boss_alias_collision);
    let snapshot = governor.snapshot(chrono::DateTime::UNIX_EPOCH);
    assert_eq!(snapshot[0].permits.total, 1);
    assert_eq!(snapshot[0].rate.capacity, 1);
    assert_eq!(snapshot[0].rate.refill_per_min, 60);
    let candidate_count = scoring::candidate_count(
        evaluation
            .cases
            .iter()
            .flat_map(|case| case.candidates().iter().map(String::as_str)),
    );
    let mut aggregate = AggregateReport::new(evaluation.cases.len(), candidate_count);

    assert!(evaluation.cases.len() <= probe::MAX_CASES);
    for (index, case) in evaluation.cases.iter().enumerate() {
        probe::pace_before_case(index).await;
        match probe::run_case(&client, &route, &codec, case).await {
            Ok(result) => aggregate.record_success(result),
            Err(()) => aggregate.record_failure(),
        }
    }

    let requests = provider.requests();
    assert_eq!(requests.len(), evaluation.cases.len());
    assert!(requests.iter().all(|request| {
        request.max_output_tokens == probe::MAX_OUTPUT_TOKENS
            && request.output_schema.is_some()
            && to_string(request).is_ok_and(|encoded| {
                !encoded.contains("https://")
                    && !encoded.contains("http://")
                    && !encoded.contains("example.invalid")
                    && !encoded.contains("Éowyn Star")
                    && !encoded.contains("Míra")
                    && !encoded.contains("Renée")
                    && !encoded.contains("Vesper Quill")
                    && !encoded.contains("Aster Vale")
                    && !encoded.contains("114200000000000011")
            })
    }));
    assert!(
        requests
            .iter()
            .any(|request| { to_string(request).is_ok_and(|encoded| encoded.contains('⟦')) })
    );

    let output = to_string(&aggregate).unwrap();
    let summary: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(aggregate.failed_acceptance());
    assert_eq!(
        summary["completed"].as_u64(),
        Some(evaluation.cases.len() as u64)
    );
    assert_eq!(summary["failures"].as_u64(), Some(0));
    assert_eq!(
        summary["exact_recoveries_per_case"].as_u64(),
        Some((evaluation.cases.len() + 1) as u64)
    );
    assert_eq!(
        summary["false_guesses_per_case"].as_u64(),
        Some((evaluation.cases.len() * 2 - 1) as u64)
    );
    assert_eq!(
        summary["ambiguous_terms_per_case"].as_u64(),
        Some(evaluation.cases.len() as u64)
    );
    assert_eq!(summary["abstentions"].as_u64(), Some(0));
    assert_eq!(
        summary["usage_responses"].as_u64(),
        Some(evaluation.cases.len() as u64)
    );
    assert_eq!(
        summary["prompt_tokens"].as_u64(),
        Some(17 * evaluation.cases.len() as u64)
    );
    assert_eq!(
        summary["candidate_identity_strings_scored"].as_u64(),
        Some(candidate_count as u64)
    );
    assert!(!output.contains(&evaluation.boss_alias_collision));
    assert!(!output.contains("Éowyn Star"));
    assert!(!output.contains("invented decoy"));
    assert!(!output.contains("114200000000000011"));
    assert!(!output.contains("https://"));
}

#[tokio::test]
async fn a_masking_client_without_a_scanner_sends_nothing() {
    let (client, provider, route, _) = fake_client([FakeAction::Response(response("{}"))]);
    let mut session = client
        .open_rewrite("privacy-eval-test", Duration::from_secs(1))
        .unwrap();
    let request = kanade::infrastructure::llm::ChatRequest {
        model: route.alias,
        messages: vec![Message::User {
            content: "Éowyn Star".into(),
        }],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 64,
        reasoning: Some(Effort::Off),
        sampling: None,
    };
    assert!(session.complete(&request).await.is_err());
    assert!(provider.requests().is_empty());
}

#[test]
fn live_options_are_required_and_the_candidate_matcher_ignores_case() {
    assert!(super::parse_options(Vec::<std::ffi::OsString>::new()).is_err());
    assert_eq!(
        scoring::score_response(
            &json!({
                "guesses": ["éOWYN STAR", "unknown"],
                "quasi_identifier_categories": []
            })
            .to_string(),
            &["Éowyn Star".to_owned()],
            &[]
        )
        .unwrap()
        .exact_recoveries,
        1
    );
}

#[test]
fn repeated_candidate_strings_across_cases_are_counted_once() {
    let first = ["shared", "first"];
    let second = ["shared", "second"];
    let cases = [first.as_slice(), second.as_slice()];
    assert_eq!(
        scoring::candidate_count(cases.iter().flat_map(|case| case.iter().copied())),
        3
    );
}

#[test]
fn boss_alias_collision_is_not_an_exact_identity_recovery() {
    let evaluation = cases::load().unwrap();
    let case = &evaluation.cases[0];
    let alias = &evaluation.boss_alias_collision;
    assert!(
        !case
            .candidates()
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(alias))
    );
    let answer = json!({
        "guesses": [alias],
        "quasi_identifier_categories": []
    })
    .to_string();
    let score =
        scoring::score_response(&answer, case.candidates(), case.ambiguous_terms()).unwrap();
    assert_eq!(score.exact_recoveries, 0);
    assert_eq!(score.false_guesses, 0);
    assert_eq!(score.ambiguous_terms, 1);
}
