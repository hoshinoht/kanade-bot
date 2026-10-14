use kanade::extract::{Amendment, AmendmentKind, merge};

#[test]
fn the_latest_explicit_value_wins_and_evidence_is_pooled() {
    let moved = |evidence: &str, time: &str| Amendment {
        kind: AmendmentKind::Move,
        bosses: vec!["HMaleficStar".into()],
        day_ref: None,
        time_ref: Some(time.into()),
        participants: Vec::new(),
        rsvp: None,
        is_question: false,
        confidence: 0.9,
        evidence_message_ids: vec![evidence.into()],
        target_run_hint: None,
    };
    let merged = merge::merge(
        &[moved("9", "9pm"), moved("8", "10pm")],
        &[] as &[&str],
        &[vec!["HMaleficStar".into()]],
    );
    assert_eq!(
        merged,
        [Amendment {
            evidence_message_ids: vec!["9".into(), "8".into()],
            ..moved("9", "10pm")
        }]
    );
}
