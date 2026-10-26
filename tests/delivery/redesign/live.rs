//! Cards through the tick and the refresh worker: posted with each picture
//! once, edited without uploads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::TimeDelta;
use kanade::cards::redesign::OPEN_IN_PORTAL;
use kanade::cards::{CardKit, V2Kit};
use kanade::discord::transport::Op;
use kanade::infrastructure::store::SqliteStore;
use twilight_model::channel::message::Component;

use crate::cards::{answer, art_dir, created, edits, kit, pictures, refresher, seed_day_of, world};
use crate::scenarios::{self, now};
use crate::support;

#[tokio::test]
async fn the_tick_posts_and_refresh_edits_without_uploads() {
    let art = art_dir();
    let store = Arc::new(SqliteStore::open_temp().await);
    let world = world();
    let fake = Arc::new(support::fake());
    let (star, _) = seed_day_of(&*store).await;
    let cards = kit(Some(&art));
    let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(cards.clone());
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let posts = created(&fake);
    let [post] = posts.as_slice() else {
        panic!("one post: {posts:?}");
    };
    assert!(
        post.content.as_deref().unwrap().contains("\n-# "),
        "{:?}",
        post.content
    );
    assert_eq!(post.embeds.len(), 2, "one embed per run");
    let names: Vec<&str> = post
        .attachments
        .iter()
        .map(|upload| upload.filename.as_str())
        .collect();
    assert_eq!(
        names,
        ["MaleficStar.png", "image-MaleficStar.png", "Kalos.webp"]
    );
    assert_eq!(
        pictures(&post.embeds[1]),
        (Some("attachment://Kalos.webp".to_owned()), None)
    );

    // An answer re-renders the card: the same names, nothing uploaded.
    answer(&*store, &star, "1002", true, now() + TimeDelta::minutes(1)).await;
    let refresh = refresher(&store, &fake, &world, cards);
    assert_eq!(refresh.refresh(std::slice::from_ref(&star)).await, 1);
    let edited = edits(&fake);
    let [edit] = edited.as_slice() else {
        panic!("one edit: {edited:?}");
    };
    let embeds = edit.embeds.as_ref().expect("embeds");
    assert_eq!(embeds.len(), 2);
    assert_eq!(
        pictures(&embeds[0]),
        (
            Some("attachment://MaleficStar.png".to_owned()),
            Some("attachment://image-MaleficStar.png".to_owned())
        )
    );
    assert!(
        embeds[0].fields[0].value.contains("Bex"),
        "{:?}",
        embeds[0].fields
    );
}

/// The link buttons of a legacy post or edit: (label, url).
fn links(rows: &[Component]) -> Vec<(String, String)> {
    rows.iter()
        .flat_map(|row| match row {
            Component::ActionRow(row) => row.components.as_slice(),
            _ => &[],
        })
        .filter_map(|component| match component {
            Component::Button(button) => Some((button.label.clone()?, button.url.clone()?)),
            _ => None,
        })
        .collect()
}

/// "Open in portal" follows the live portal switch on the next re-render
/// of the posted card (the refresh and header-rewrite path).
#[tokio::test]
async fn refresh_adds_and_drops_the_portal_link_with_the_switch() {
    let art = art_dir();
    let store = Arc::new(SqliteStore::open_temp().await);
    let world = world();
    let fake = Arc::new(support::fake());
    let (star, _) = seed_day_of(&*store).await;
    let open = Arc::new(AtomicBool::new(true));
    let switch = Arc::clone(&open);
    let cards = CardKit {
        v2: V2Kit {
            portal: Some("https://portal.example".to_owned()),
            portal_open: Some(Arc::new(move || switch.load(Ordering::SeqCst))),
            ..V2Kit::default()
        },
        ..kit(Some(&art))
    };
    let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(cards.clone());
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let posts = created(&fake);
    let [post] = posts.as_slice() else {
        panic!("one post: {posts:?}");
    };
    // Two runs on the card: the portal's root.
    let link = vec![(
        OPEN_IN_PORTAL.to_owned(),
        "https://portal.example/".to_owned(),
    )];
    assert_eq!(links(&post.components), link);
    assert_eq!(post.embeds.len(), 2);
    assert_eq!(
        fake.count(Op::AddReaction),
        2,
        "✅ and ❌ seeded beside the link row"
    );

    let refresh = refresher(&store, &fake, &world, cards);
    let rerender = async |minutes: i64, user: &str| {
        answer(
            &*store,
            &star,
            user,
            true,
            now() + TimeDelta::minutes(minutes),
        )
        .await;
        assert_eq!(refresh.refresh(std::slice::from_ref(&star)).await, 1);
        edits(&fake).pop().expect("edit")
    };

    open.store(false, Ordering::SeqCst);
    let closed = rerender(1, "1002").await;
    assert_eq!(
        closed.components,
        Some(Vec::new()),
        "the closed portal's row is cleared"
    );
    assert!(closed.valid() && closed.embeds.is_some());

    open.store(true, Ordering::SeqCst);
    let reopened = rerender(2, "1001").await;
    assert_eq!(links(reopened.components.as_deref().unwrap()), link);
    assert!(reopened.valid() && reopened.v2_components().is_none());
}
