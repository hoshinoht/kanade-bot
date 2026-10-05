//! Which write-hook hints ([`Written`]) each store sends: the same writes on
//! every store must hint the same kinds, and writes that change nothing
//! (identical gateway updates, missing rows, replays) hint nothing.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};

use super::{WriteObserver, Written, conformance, members_conformance, model_log_conformance};
use crate::domain::{
    members::{MemberStore, PortalEdit},
    model_log::ModelLogStore,
    schedule::{Change, ChangeSet},
    scheduler::{ScheduleStore, Scope},
    settings::{SettingsStore, keys},
};

/// Run the checks against `store`, after `observe` installs the hook on it.
pub async fn run_suite<S>(store: &S, observe: impl FnOnce(&S, WriteObserver) -> bool)
where
    S: ScheduleStore + MemberStore + ModelLogStore + SettingsStore,
{
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    assert!(observe(
        store,
        Arc::new(move |written| sink.lock().unwrap().push(written))
    ));
    let take = || std::mem::take(&mut *seen.lock().unwrap());
    let at = Utc.with_ymd_and_hms(2026, 9, 2, 0, 0, 0).unwrap();

    let revision = store.load(&Scope::All).await.unwrap().revision;
    let run = conformance::run("hint-run", None, conformance::week(), 20);
    store
        .commit(
            revision,
            ChangeSet {
                changes: vec![Change::PutRun(run)],
            },
            conformance::meta(),
        )
        .await
        .unwrap();
    assert_eq!(take(), [Written::Schedule], "a commit");

    store
        .put_settings_rows(vec![(keys::QUIET_MODE.into(), "1".into())])
        .await
        .unwrap();
    assert_eq!(take(), [Written::Settings], "a settings save");

    let alice = members_conformance::gateway("200", "Alice", &["20"]);
    store.apply_gateway(alice.clone()).await.unwrap();
    store.apply_gateway(alice).await.unwrap();
    assert_eq!(
        take(),
        [Written::Members],
        "an identical gateway update hints nothing"
    );
    store
        .apply_gateway(members_conformance::gateway("200", "Alice B", &["20"]))
        .await
        .unwrap();
    assert!(!store.member_departed("404").await.unwrap());
    let edit = PortalEdit {
        add_alias: Some("ally".into()),
        ..PortalEdit::default()
    };
    assert!(
        store
            .apply_portal("404", edit.clone())
            .await
            .unwrap()
            .is_none()
    );
    store.apply_portal("200", edit).await.unwrap();
    assert!(store.member_departed("200").await.unwrap());
    assert_eq!(
        take(),
        [Written::Members, Written::Members, Written::Members],
        "a rename, a portal edit and a departure; missing rows hint nothing"
    );

    store
        .record_chat(model_log_conformance::chat("hint-chat", at))
        .await
        .unwrap();
    store
        .record_masked_chat(
            model_log_conformance::chat("hint-masked", at),
            model_log_conformance::masked(),
        )
        .await
        .unwrap();
    store
        .record_extraction(model_log_conformance::extraction("hint-x", at))
        .await
        .unwrap();
    store
        .insert_rescan_job(model_log_conformance::rescan("hint-job", at))
        .await
        .unwrap();
    assert_eq!(
        take(),
        [
            Written::Chat,
            Written::Chat,
            Written::Extraction,
            Written::Rescan
        ],
        "log rows"
    );
    assert!(
        store
            .record_chat(model_log_conformance::chat("hint-chat", at))
            .await
            .is_err()
    );
    assert_eq!(take(), [], "a refused write hints nothing");
}
