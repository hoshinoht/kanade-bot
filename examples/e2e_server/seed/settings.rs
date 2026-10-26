//! Config (mock `config::defaults`): the persona files the server reads at
//! startup, and the saved settings sections the mock's Config shows.

use std::{fs, io, path::Path};

use chrono::NaiveTime;
use kanade::domain::settings::{
    Chatbot, Notifications, Persona, Pings, Profanity, RoleProfileAssignment, Section, SelfService,
    save_section,
};

use super::{Seed, members::STAFF_ROLE};
use crate::Fail;

/// The mock's invented placeholder words (its stand-in built-in list), as
/// extra words: the real built-in list is real profanity, which the specs
/// never name.
const EXTRA_WORDS: [&str; 3] = ["drat", "frak", "smeg"];

/// The persona catalog: id, label.
const BUNDLES: [(&str, &str); 3] = [
    ("kanade", "Kanade"),
    ("yuuki", "YuukiSakuna"),
    ("plain", "Plain (no persona)"),
];

/// Reply profiles: id, label, voice, prompt, offered to members.
const PROFILES: [(&str, &str, &str, &str, bool); 4] = [
    (
        "default",
        "Default",
        "Warm and plain",
        "Answers plainly in the guild's voice, names the run facts first and keeps it short.",
        true,
    ),
    (
        "terse",
        "Terse",
        "Short to the point of blunt",
        "One or two short sentences, schedule facts only, no small talk and no emoji.",
        true,
    ),
    (
        "kanade",
        "Kanade",
        "comedy.",
        "## Kanade\n- **Teases** lightly in the persona's voice\n- keeps every *schedule* fact exact",
        true,
    ),
    (
        "sparkly",
        "Sparkly",
        "Overexcited kouhai",
        "Bursts with enthusiasm and exclamation, still lands the facts. Private while it settles in.",
        false,
    ),
];

/// The persona directory: the tracked Kanade bundle (also standing in for
/// the catalog's other two), the catalog and the reply profiles.
pub fn persona_files(dir: &Path, kanade: &Path) -> io::Result<()> {
    let bundle = fs::read_to_string(kanade)?;
    let mut catalog = String::from("schema_version: 1\ndefault: kanade\npersonas:\n");
    for (id, label) in BUNDLES {
        fs::write(
            dir.join(format!("bundles/{id}.yaml")),
            bundle.replacen("\nid: kanade\n", &format!("\nid: {id}\n"), 1),
        )?;
        catalog.push_str(&format!(
            "  - id: {id}\n    label: {label:?}\n    aliases: []\n"
        ));
    }
    fs::write(dir.join("catalog.yaml"), catalog)?;
    for (id, label, voice, prompt, _) in PROFILES {
        let prompt = prompt.replace('\n', "\n  ");
        fs::write(
            dir.join(format!("profiles/{id}.yaml")),
            format!(
                "schema_version: 1\nid: {id}\nlabel: {label}\nvoice: {voice:?}\nprompt: |\n  {prompt}\n"
            ),
        )?;
    }
    Ok(())
}

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    let store = &*seed.store;
    let sections = [
        Section::SelfService(SelfService {
            public_portal: true,
            ..SelfService::default()
        }),
        Section::Pings(Pings {
            day_of_ping_time: NaiveTime::from_hms_opt(9, 0, 0).ok_or("09:00")?,
            countdown_minutes: vec![60, 15],
        }),
        // The header batch at 18:00, after the pinned noon and before the
        // first countdown (21:00): a generation starting at noon has no batch
        // due and nothing to catch up on, so no rewrite runs behind a spec.
        Section::Notifications(Notifications {
            header_generation_time: NaiveTime::from_hms_opt(18, 0, 0).ok_or("18:00")?,
            ..Notifications::default()
        }),
        Section::Chatbot(Chatbot {
            enabled: true,
            ..Chatbot::default()
        }),
        Section::Persona(Persona {
            active: "kanade".into(),
            profile_visibility: PROFILES
                .iter()
                .filter(|profile| profile.4)
                .map(|profile| profile.0.to_owned())
                .collect(),
            role_profiles: vec![
                RoleProfileAssignment {
                    role_id: STAFF_ROLE.into(),
                    profile: "terse".into(),
                },
                RoleProfileAssignment {
                    role_id: "300002".into(),
                    profile: "default".into(),
                },
            ],
        }),
        Section::Profanity(Profanity {
            extra_words: EXTRA_WORDS.map(str::to_owned).to_vec(),
            ..Profanity::default()
        }),
    ];
    for section in &sections {
        save_section(store, section).await?;
    }
    Ok(())
}
