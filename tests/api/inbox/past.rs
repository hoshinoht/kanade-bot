//! `GET /api/admin/inbox/past`: closed proposals and member requests, newest
//! closed first, with outcome, decider, reason and History link; paged by
//! the last id shown.

use kanade_domain::drafts::{DraftChange, DraftUpdate, DraftWrite, SUPERSEDED};

use super::*;

const PAGE: &str = "inbox.json#/$defs/PastPage";

impl Inbox {
    async fn past(&self, query: &str) -> Reply {
        request(
            self.reads.admin,
            "GET",
            ADMIN_HOST,
            &format!("/api/admin/inbox/past{query}"),
            &[("Cookie", self.reads.cookie.as_str())],
        )
        .await
    }

    async fn past_page(&self, query: &str) -> Value {
        let reply = self.past(query).await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let value = reply.json();
        assert_valid(PAGE, "/api/admin/inbox/past", &value);
        value
    }

    /// Close a live draft directly, as the domain would, at `at`.
    async fn close(
        &self,
        id: &str,
        status: DraftStatus,
        actor: Actor,
        reason: Option<&str>,
        at: DateTime<Utc>,
    ) {
        let store = &self.reads.store;
        let draft = store.load_draft(id).await.unwrap().unwrap().draft;
        let written = store
            .update_draft(DraftUpdate {
                draft_id: id.into(),
                expected_version: draft.version,
                actor,
                at,
                change: DraftChange::Close {
                    status,
                    reason: reason.map(str::to_owned),
                    notices: Vec::new(),
                },
            })
            .await
            .unwrap();
        assert!(matches!(written, DraftWrite::Written(_)), "{id} closed");
    }
}

fn ids_of(items: &Value) -> Vec<String> {
    strs(items, "id").into_iter().map(str::to_owned).collect()
}

#[tokio::test]
async fn closed_items_list_newest_first_with_outcome_decider_and_history_link() {
    let inbox = seeded().await;
    let ids = &inbox.ids;
    assert_eq!(
        inbox.past_page("").await,
        json!({"items": [], "next_before": null}),
        "nothing closed yet"
    );

    use DraftStatus::{Discarded, Expired, Rejected, Withdrawn};
    let system = |component: &str| Actor::system(component);
    let (admin, member) = (|id: &str| Actor::admin(id), |id: &str| Actor::member(id));
    let (enough, needed) = (
        Some("One weekly is enough."),
        Some("We need you this week."),
    );
    let closes = [
        (
            &ids.expired_request,
            Expired,
            system("delivery"),
            None,
            (24, 0, 0),
        ),
        (
            &ids.expired_extraction,
            Discarded,
            system("extraction"),
            Some(SUPERSEDED),
            (28, 5, 0),
        ),
        (
            &ids.expired_chat,
            Discarded,
            admin("token"),
            Some("duplicate"),
            (28, 6, 0),
        ),
        (
            &ids.new_fixed,
            Rejected,
            admin("tailscale:ops"),
            enough,
            (29, 5, 0),
        ),
        (&ids.leave, Withdrawn, member("1002"), None, (29, 5, 10)),
        (
            &ids.cancel_chat,
            Rejected,
            member("1003"),
            None,
            (29, 5, 20),
        ),
        (
            &ids.swap,
            Rejected,
            admin("discord:1003"),
            needed,
            (29, 5, 30),
        ),
    ];
    let mut head = 0;
    for (n, (id, status, actor, reason, (day, hour, minute))) in closes.into_iter().enumerate() {
        // After the expiries, the Discord admin approves the move; that
        // retires the other live r-kalos proposal (closed superseded).
        if n == 3 {
            head = inbox.head().await;
            let version = inbox.item(&ids.moved).await["version"].clone();
            let approve = path(&ids.moved, "approve");
            ok(&inbox
                .discord(&approve, json!({"version": version}), &[])
                .await);
        }
        let at = utc(9, day, hour, minute);
        inbox.close(id, status, actor, reason, at).await;
    }

    let page = inbox.past_page("").await;
    assert_eq!(page["next_before"], Value::Null);
    let items = page["items"].as_array().unwrap().clone();
    let listed = ids_of(&page["items"]);
    // The approval and the move it retired closed at the same instant: by id.
    let mut same_instant = vec![ids.moved.clone(), ids.to_edit.clone()];
    same_instant.sort_by(|a, b| b.cmp(a));
    let mut expected = vec![
        ids.swap.clone(),
        ids.cancel_chat.clone(),
        ids.leave.clone(),
        ids.new_fixed.clone(),
    ];
    expected.extend(same_instant);
    expected.extend([
        ids.expired_chat.clone(),
        ids.expired_extraction.clone(),
        ids.expired_request.clone(),
    ]);
    assert_eq!(listed, expected);
    for live in [
        &ids.join,
        &ids.change,
        &ids.unauthorised,
        &ids.to_no_effect,
        &ids.cancel_nkalos,
    ] {
        assert!(!listed.contains(live), "{live} is still live");
    }
    let by = |id: &str| items.iter().find(|item| item["id"] == id).unwrap().clone();

    let moved = by(&ids.moved);
    assert_eq!(
        fields(&moved, &["outcome", "tab", "source", "kind"]),
        json!(["approved", "extractor", "extraction", "move"])
    );
    assert_eq!(
        moved["decided_by"],
        json!({"kind": "member", "id": "1003", "name": "Cara"})
    );
    assert_eq!(moved["history_seq"], head + 1);
    assert_eq!(moved["decided_at"], "2026-09-29T04:00:00Z");
    assert_eq!(moved["created_at"], "2026-09-29T04:00:00Z");
    assert_eq!(moved["reason"], Value::Null);
    assert_eq!(moved["source_id"], "log-1");
    assert_eq!(moved["summary"], "Alice asks for Wednesday");
    assert_eq!(moved["channel"], "#kalos-four");
    assert_eq!(moved["requester"], Value::Null);
    assert_eq!(moved["card_url"], Value::Null, "the card was never posted");
    let evidence = moved["evidence"].as_array().unwrap();
    assert_eq!(
        fields(&evidence[0], &["author", "missing"]),
        json!(["Alice", false])
    );
    // Not cached any more, but the link still names the message.
    assert_eq!(evidence[1]["missing"], true);
    assert_eq!(
        evidence[1]["url"],
        format!(
            "https://discord.com/channels/900/kalos-four/{}",
            snowflake(utc(9, 29, 3, 31))
        )
    );

    let to_edit = by(&ids.to_edit);
    assert_eq!(to_edit["history_seq"], Value::Null);
    assert_eq!(to_edit["outcome"], "superseded");
    assert_eq!(to_edit["decided_by"]["name"], "Kanade");

    let swap = by(&ids.swap);
    assert_eq!(
        fields(&swap, &["outcome", "tab", "source", "kind"]),
        json!(["rejected", "self_service", "self_service", "swap"])
    );
    assert_eq!(swap["reason"], "We need you this week.");
    assert_eq!(
        swap["decided_by"],
        json!({"kind": "admin", "id": "discord:1003", "name": "Cara"})
    );
    assert_eq!(swap["decided_at"], "2026-09-29T05:30:00Z");
    assert_eq!(swap["requester"]["id"], "1001");
    assert_eq!(swap["summary"], "Gus takes my spot");
    assert_eq!(swap["source_id"], Value::Null);
    assert_eq!(swap["evidence"], json!([]));

    let cases = [
        (&ids.cancel_chat, "rejected", "Cara", Value::Null),
        (&ids.leave, "withdrawn", "", Value::Null),
        (
            &ids.new_fixed,
            "rejected",
            "Admin (ops)",
            json!("One weekly is enough."),
        ),
        (
            &ids.expired_chat,
            "discarded",
            "Admin (token)",
            json!("duplicate"),
        ),
        (&ids.expired_extraction, "superseded", "Kanade", Value::Null),
        (&ids.expired_request, "expired", "Kanade", Value::Null),
    ];
    for (id, outcome, name, reason) in cases {
        let item = by(id);
        assert_eq!(item["outcome"], outcome, "{id}");
        assert_eq!(item["reason"], reason, "{id}");
        if !name.is_empty() {
            assert_eq!(item["decided_by"]["name"], name, "{id}");
        }
    }
    assert_eq!(by(&ids.leave)["decided_by"]["kind"], "member");
    assert_eq!(by(&ids.cancel_chat)["source"], "chat");
    assert_eq!(by(&ids.expired_request)["decided_by"]["id"], "delivery");

    // Pages of four, by the last id shown, add up to the whole list.
    let mut paged = Vec::new();
    let mut query = "?limit=4".to_owned();
    let mut pages = 0;
    loop {
        let page = inbox.past_page(&query).await;
        let got = ids_of(&page["items"]);
        assert!(got.len() <= 4);
        paged.extend(got);
        pages += 1;
        match page["next_before"].as_str() {
            Some(before) => query = format!("?limit=4&before={before}"),
            None => break,
        }
    }
    assert_eq!((pages, paged), (3, expected));
}

#[tokio::test]
async fn the_past_list_refuses_bad_queries_and_needs_a_session() {
    let inbox = seeded().await;
    for query in [
        "?limit=0",
        "?limit=201",
        "?limit=x",
        "?limit=",
        "?before=nope",
        "?before=",
        "?limit=4&limit=5",
        "?page=2",
    ] {
        let reply = inbox.past(query).await;
        assert_eq!(refused(&reply), code(422, "invalid_query"), "{query}");
    }
    assert_eq!(inbox.past("?limit=200").await.status, 200);

    let anonymous = request(
        inbox.reads.admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/inbox/past",
        &[],
    )
    .await;
    assert_eq!(anonymous.status, 401, "{}", anonymous.text());
    assert_valid(ERROR, "anonymous", &anonymous.json());
}
