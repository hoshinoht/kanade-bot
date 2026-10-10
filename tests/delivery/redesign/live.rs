//! The message style through the tick and the refresh worker: read live per
//! card, posted with each picture once, edited without uploads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::TimeDelta;
use kanade::bot::delivery::CardRefresh;
use kanade::bot::delivery::cards::CardKit;
use kanade::domain::settings::MessageStyle;
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::cards::{answer, art_dir, created, edits, kit, pictures, seed_day_of, world};
use crate::scenarios::{self, now};
use crate::support;

/// The style read through `redesigned` on every card.
fn styled(art: &crate::support::TempDir, redesigned: &Arc<AtomicBool>) -> CardKit {
    let live = Arc::clone(redesigned);
    CardKit {
        style: Some(Arc::new(move || {
            if live.load(Ordering::SeqCst) {
                MessageStyle::Redesigned
            } else {
                MessageStyle::Classic
            }
        })),
        ..kit(Some(art))
    }
}

#[tokio::test]
async fn the_tick_posts_and_refresh_edits_in_the_live_style() {
    let art = art_dir();
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    let (star, _) = seed_day_of(&*store).await;
    let redesigned = Arc::new(AtomicBool::new(true));
    let cards = styled(&art, &redesigned);
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
    let refresh = CardRefresh {
        store: Arc::clone(&store),
        transport: Arc::clone(&fake),
        members: Arc::new(world.roster.clone()),
        cards,
        policy: scenarios::config().policy,
        quiet: Arc::new(AtomicBool::new(false)),
        now: Arc::new(|| now() + TimeDelta::minutes(1)),
    };
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

    // Switching back applies on the next render.
    redesigned.store(false, Ordering::SeqCst);
    answer(&*store, &star, "1001", true, now() + TimeDelta::minutes(2)).await;
    assert_eq!(refresh.refresh(std::slice::from_ref(&star)).await, 1);
    let latest = edits(&fake).pop().expect("classic edit");
    assert_eq!(latest.embeds.as_ref().expect("embeds").len(), 1, "classic");
}
