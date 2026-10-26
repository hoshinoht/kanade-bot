//! The `bot` component alone (`KANADE_COMPONENTS=bot`): the Discord side
//! over the store, `/healthz` alone on the admin listener, nothing public.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::*;
use crate::runtime::serve::budget;

const PORTAL_HOST: &str = "portal.kanade.test";

/// A configured public listener on `port`, which only `web` turns on.
fn public_listener(port: &str) -> [(&'static str, &str); 3] {
    [
        ("KANADE_PUBLIC_HOST", PORTAL_HOST),
        ("KANADE_PUBLIC_BIND", port),
        ("KANADE_CLOUDFLARED_PEER", "127.0.0.1"),
    ]
}

fn logged(event: &str) -> Vec<Value> {
    kanade_log::captured()
        .into_iter()
        .filter(|line| line["event"] == event)
        .collect()
}

/// One `GET` from this host, as the local healthcheck sends it.
async fn get(address: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let text = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(text.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap();
    (status, body.to_owned())
}

#[tokio::test]
async fn bot_only_serves_healthz_alone_and_stops_inside_the_budget() {
    kanade_log::capture();
    let public = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let public_address = public.local_addr().unwrap();
    drop(public);
    let port = public_address.to_string();
    let mut extra = vec![("KANADE_COMPONENTS", "bot")];
    extra.extend(public_listener(&port));
    let harness = Harness::with(&extra);
    assert_eq!(harness.config.runtime.public_bind, None);

    let (events, source) = script();
    let wiring = Wiring {
        source,
        transport: Arc::clone(&harness.fake),
        clock: Arc::new(clock::system_now),
        tick: TICK,
        extraction: extract::Timing::default(),
    };
    let (trigger, triggered) = oneshot::channel::<()>();
    let serve = serve_with(
        &harness.config,
        async {
            let _ = triggered.await;
        },
        wiring,
    );
    let script = async {
        eventually!("the admin listener", !logged("server_started").is_empty());
        let address: SocketAddr = logged("server_started")[0]["bind"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let (status, body) = get(address, "/healthz").await;
        assert_eq!(status, 503, "not ready before READY: {body}");
        events.send(ready()).unwrap();
        events.send(guild_create(&[HOME_A])).unwrap();
        eventually!("a ready health document", {
            let (status, body) = get(address, "/healthz").await;
            status == 200 && body.contains(r#""discord":"ready""#)
        });
        let (_, body) = get(address, "/healthz").await;
        let health: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (&health["mode"], &health["storage"], &health["scheduler"]),
            (&json!("live"), &json!("ok"), &json!("running"))
        );
        // No admin or public router: every other path is unmounted.
        for path in ["/api/admin/config", "/api/admin/session", "/"] {
            assert_eq!(get(address, path).await.0, 404, "{path}");
        }
        assert_eq!(logged("server_started").len(), 1, "one listener");
        assert!(
            TcpStream::connect(public_address).await.is_err(),
            "the public listener is not bound"
        );
        assert_eq!(logged("web_disabled").len(), 1);
        let _ = trigger.send(());
        Instant::now()
    };
    let (served, triggered_at) = tokio::join!(serve, script);
    served.expect("a bot-only serve stops cleanly");
    assert!(triggered_at.elapsed() < budget::TOTAL);
    assert_eq!(logged("store_closed").len(), 1);
    drop(events);
    let store = store::open(&harness.config.store)
        .await
        .expect("serve released the store");
    store::close(store, Duration::ZERO).await;
}

/// The countdown card's "Open in portal" button links the public listener's
/// host only while `web` serves it.
#[tokio::test]
async fn countdown_cards_link_the_portal_only_while_web_runs() {
    for (components, linked) in [("bot,web", true), ("bot", false)] {
        let mut extra = vec![("KANADE_COMPONENTS", components)];
        extra.extend(public_listener("127.0.0.1:0"));
        let harness = Harness::with(&extra);
        let policy = policy(&harness);
        let now = clock::system_now();
        let mut due_run = String::new();
        harness
            .seed(async |store| {
                use kanade_domain::settings::{SettingsStore, keys};
                store
                    .put_settings_rows(vec![(keys::PUBLIC_PORTAL.to_owned(), "1".to_owned())])
                    .await
                    .unwrap();
                let start = now + chrono::Duration::seconds(59 * 60 + 30);
                due_run = create_run(store, &policy, now, HOME_C, start).await;
                SchedulerService::new(store, RandomIds, FixedClock(now))
                    .with_attendance(policy.attendance)
                    .as_origin(Origin::for_tests())
                    .add_reminder(
                        &due_run,
                        "countdown_60",
                        start - chrono::Duration::minutes(60),
                        None,
                    )
                    .await
                    .unwrap()
                    .expect("a new reminder");
            })
            .await;
        let (mut discord, ctx) = harness.start().await;
        drive(&mut discord, async {
            ctx.events.send(ready()).unwrap();
            ctx.events.send(guild_create(&[HOME_C])).unwrap();
            eventually!(
                "the countdown",
                countdown_message(&ctx.store, &due_run).await.is_some()
            );
        })
        .await;
        let links = harness
            .fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Create {
                    channel, message, ..
                } if channel.get() == HOME_C => Some(format!("{:?}", message.components)),
                _ => None,
            })
            .filter(|components| components.contains(PORTAL_HOST))
            .count();
        assert_eq!(links, usize::from(linked), "{components}");
        finish(&harness, ctx).await;
    }
}
