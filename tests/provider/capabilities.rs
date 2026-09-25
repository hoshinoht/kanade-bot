use kanade::infrastructure::llm::{
    AdmissionLimits, Effort, ModelCapabilities, TrustZone, parse_models_list, resolve_capabilities,
};

#[test]
fn kanata_listing_parses_metadata_and_tolerates_junk() {
    let models =
        parse_models_list(include_bytes!("../fixtures/provider/models-kanata.json")).unwrap();
    let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "sumi-structured",
            "codex-like",
            "glm-cloud",
            "plain-alias",
            "bad-metadata",
            "odd-flags"
        ]
    );

    let structured = models[0].capabilities.clone().unwrap();
    assert_eq!(
        structured,
        ModelCapabilities {
            operations: vec!["chat".into()],
            structured_output: true,
            sampling_controls: true,
            reasoning_control: true,
            function_tools: true,
            streaming: true,
            trust_zone: Some(TrustZone::PrivateNetwork),
            // Kanata's `none` is our `Off`.
            reasoning_efforts: Some(vec![
                Effort::Off,
                Effort::Minimal,
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::Xhigh,
                Effort::Max,
            ]),
            context_tokens: Some(32_768),
            admission: Some(AdmissionLimits {
                max_in_flight: 4,
                max_queue: Some(16),
                queue_ms: Some(1_000),
                adapter_max_in_flight: Some(2),
            }),
        }
    );
    assert_eq!(structured.admission.unwrap().concurrency(), 2);
    let codex = models[1].capabilities.clone().unwrap();
    assert!(!codex.structured_output && !codex.sampling_controls && codex.reasoning_control);
    assert_eq!(codex.context_tokens, None);
    let admission = codex.admission.unwrap();
    assert_eq!(admission.adapter_max_in_flight, None);
    assert_eq!(admission.concurrency(), 8, "no adapter cap");
    assert!(codex.is_cloud("codex-like"));

    let cloud = models[2].capabilities.clone().unwrap();
    assert_eq!(cloud.trust_zone, Some(TrustZone::Local));
    assert!(cloud.is_cloud("glm-cloud"));
    assert!(!cloud.is_cloud("glm"));
    assert_eq!(cloud.reasoning_efforts, None);

    assert_eq!(models[3].capabilities, None);
    assert_eq!(
        models[4].capabilities, None,
        "non-object metadata is absent"
    );
    let odd = models[5].capabilities.clone().unwrap();
    assert!(
        !odd.structured_output,
        "wrong-typed flags keep their default"
    );
    assert!(odd.function_tools);
    assert_eq!(odd.trust_zone, None);
    assert_eq!(odd.reasoning_efforts, None);
    assert_eq!(odd.operations, vec!["chat".to_owned()]);
    assert_eq!(odd.context_tokens, None);
    assert_eq!(
        odd.admission, None,
        "admission needs a numeric max_in_flight"
    );
    assert_eq!(
        models[2].capabilities.clone().unwrap().admission,
        None,
        "public listener publishes no admission block"
    );
}

#[test]
fn bare_ollama_listing_has_no_metadata() {
    let models =
        parse_models_list(include_bytes!("../fixtures/provider/models-ollama.json")).unwrap();
    assert_eq!(models.len(), 2);
    assert!(models.iter().all(|model| model.capabilities.is_none()));
}

#[test]
fn non_listing_bodies_are_rejected_without_panicking() {
    for body in [
        b"".as_slice(),
        b"not json",
        b"[]",
        b"{}",
        b"{\"data\":{}}",
        b"{\"data\":null}",
        b"{\"data\":[",
        &[0xff, 0xfe, 0x00],
    ] {
        assert!(parse_models_list(body).is_none());
    }
    assert_eq!(parse_models_list(b"{\"data\":[]}"), Some(Vec::new()));
}

#[test]
fn capability_precedence_is_published_then_declared_then_minimal() {
    let published = ModelCapabilities {
        structured_output: true,
        ..ModelCapabilities::minimal()
    };
    let declared = ModelCapabilities {
        sampling_controls: true,
        ..ModelCapabilities::minimal()
    };
    assert_eq!(
        resolve_capabilities(Some(&published), Some(&declared)),
        published
    );
    assert_eq!(resolve_capabilities(None, Some(&declared)), declared);
    let minimal = resolve_capabilities(None, None);
    assert_eq!(minimal, ModelCapabilities::minimal());
    assert!(
        !minimal.structured_output
            && !minimal.sampling_controls
            && !minimal.reasoning_control
            && !minimal.function_tools
    );
    assert!(!minimal.is_cloud("qwen3:8b"));
    assert!(minimal.is_cloud("gpt-oss:120b-cloud"));
}
