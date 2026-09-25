use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use twilight_model::id::Id;

use super::*;

const AVATAR: &str = "0123456789abcdef0123456789abcdef";
const BANNER: &str = "a_fedcba9876543210fedcba9876543210";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("kanade-identity-{}", uuid::Uuid::new_v4()));
        files::ensure_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Canned CDN answers by path prefix; everything else fails.
#[derive(Default)]
struct Stub {
    user: Option<CurrentUser>,
    images: HashMap<&'static str, Result<Image, &'static str>>,
    asked: Mutex<Vec<String>>,
}

impl IdentitySource for Stub {
    async fn current_user(&self) -> Option<CurrentUser> {
        self.user.clone()
    }

    async fn image(&self, path: &str) -> Result<Image, &'static str> {
        self.asked.lock().unwrap().push(path.to_owned());
        self.images
            .iter()
            .find(|(prefix, _)| path.starts_with(*prefix))
            .map_or(Err("unavailable"), |(_, image)| image.clone())
    }
}

fn user(avatar: Option<&str>, banner: Option<&str>) -> CurrentUser {
    serde_json::from_value(json!({
        "id": "42", "username": "kanade", "global_name": null, "discriminator": "0",
        "avatar": avatar, "banner": banner, "bot": true, "mfa_enabled": false,
    }))
    .unwrap()
}

fn image(content_type: &str, bytes: &[u8]) -> Result<Image, &'static str> {
    Ok(Image {
        content_type: content_type.into(),
        bytes: bytes.to_vec(),
    })
}

fn ready_cache() -> GuildCache {
    let cache = GuildCache::new(Id::new(7));
    cache.set_self_user(&user(Some(AVATAR), None));
    cache
}

fn events() -> Vec<(String, serde_json::Value)> {
    logging::captured()
        .into_iter()
        .map(|line| (line["event"].as_str().unwrap_or_default().to_owned(), line))
        .collect()
}

#[tokio::test]
async fn refresh_writes_both_images_and_replaces_a_stale_extension() {
    logging::capture();
    let dir = TempDir::new();
    fs::write(dir.0.join("avatar.webp"), b"old").unwrap();
    let stub = Stub {
        user: Some(user(Some(AVATAR), Some(BANNER))),
        images: HashMap::from([
            ("/avatars/42/", image("image/png", b"PNG")),
            ("/banners/42/", image("image/gif; charset=binary", b"GIF")),
        ]),
        ..Stub::default()
    };
    refresh(&stub, &ready_cache(), &dir.0).await;

    assert_eq!(fs::read(dir.0.join("avatar.png")).unwrap(), b"PNG");
    assert_eq!(fs::read(dir.0.join("banner.gif")).unwrap(), b"GIF");
    assert!(
        !dir.0.join("avatar.webp").exists(),
        "stale extension removed"
    );
    assert!(!dir.0.join(".avatar.tmp").exists());
    assert_eq!(
        *stub.asked.lock().unwrap(),
        vec![
            format!("/avatars/42/{AVATAR}.png?size=256"),
            format!("/banners/42/{BANNER}.gif?size=600"),
        ]
    );
    let events = events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "identity_cached");
    assert_eq!(events[0].1["avatar"], true);
    assert_eq!(events[0].1["banner"], true);
}

#[tokio::test]
async fn guild_avatar_wins_and_a_missing_banner_falls_back_to_the_wash() {
    logging::capture();
    let dir = TempDir::new();
    fs::write(dir.0.join("banner.png"), b"old banner").unwrap();
    let cache = ready_cache();
    let guild: ImageHash = BANNER.parse().unwrap();
    cache.member_profile(Id::new(42), None, Some(guild));
    let stub = Stub {
        user: Some(user(Some(AVATAR), None)),
        images: HashMap::from([("/guilds/7/users/42/avatars/", image("image/webp", b"W"))]),
        ..Stub::default()
    };
    refresh(&stub, &cache, &dir.0).await;
    assert_eq!(fs::read(dir.0.join("avatar.webp")).unwrap(), b"W");
    assert!(!files::cached(&dir.0, "banner"), "no banner: the wash");
    assert_eq!(
        *stub.asked.lock().unwrap(),
        vec![format!("/guilds/7/users/42/avatars/{BANNER}.gif?size=256")]
    );
}

#[tokio::test]
async fn failures_keep_the_last_files_and_are_logged() {
    logging::capture();
    let dir = TempDir::new();
    fs::write(dir.0.join("avatar.png"), b"kept").unwrap();
    fs::write(dir.0.join("banner.png"), b"kept too").unwrap();

    refresh(&Stub::default(), &ready_cache(), &dir.0).await;
    let stub = Stub {
        user: Some(user(Some(AVATAR), Some(BANNER))),
        images: HashMap::from([
            ("/avatars/", image("text/html", b"<html>")),
            ("/banners/", Err("timeout")),
        ]),
        ..Stub::default()
    };
    refresh(&stub, &ready_cache(), &dir.0).await;

    assert_eq!(fs::read(dir.0.join("avatar.png")).unwrap(), b"kept");
    assert_eq!(fs::read(dir.0.join("banner.png")).unwrap(), b"kept too");
    let failed: Vec<_> = events()
        .into_iter()
        .filter(|(event, _)| event == "identity_refresh_failed")
        .map(|(_, line)| {
            (
                line["kind"].clone(),
                line["reason"].clone(),
                line["level"].clone(),
            )
        })
        .collect();
    assert_eq!(
        failed,
        vec![
            (json!("profile"), json!("unavailable"), json!("WARN")),
            (json!("avatar"), json!("content_type"), json!("WARN")),
            (json!("banner"), json!("timeout"), json!("WARN")),
        ]
    );
}

#[test]
fn oversize_empty_and_non_images_are_refused() {
    let ok = |content_type: &str, len: usize| {
        accept(&Image {
            content_type: content_type.into(),
            bytes: vec![0; len],
        })
    };
    assert_eq!(ok("image/png", 1), Ok("png"));
    assert_eq!(ok("IMAGE/JPEG", MAX_IMAGE), Ok("jpg"));
    assert_eq!(ok("image/png", MAX_IMAGE + 1), Err("size"));
    assert_eq!(ok("image/png", 0), Err("size"));
    for content_type in ["image/svg+xml", "text/html", ""] {
        assert_eq!(ok(content_type, 3), Err("content_type"), "{content_type}");
    }
}

/// One canned HTTP/1.1 answer on loopback.
async fn stub_cdn(head: &'static str, body: Vec<u8>) -> Cdn {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).await;
        let _ = stream
            .write_all(format!("{head}\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await;
        let _ = stream.write_all(&body).await;
    });
    Cdn::plain(address)
}

#[tokio::test]
async fn cdn_caps_the_body_and_never_follows_redirects() {
    let cdn = stub_cdn(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png",
        b"PNG".to_vec(),
    )
    .await;
    assert_eq!(
        cdn.get("/avatars/1/x.png").await,
        Ok(Image {
            content_type: "image/png".into(),
            bytes: b"PNG".to_vec(),
        })
    );
    let cdn = stub_cdn(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png",
        vec![0; MAX_IMAGE + 1],
    )
    .await;
    assert_eq!(
        cdn.get("/avatars/1/x.png").await,
        Err("oversize_or_truncated")
    );
    let cdn = stub_cdn(
        "HTTP/1.1 302 Found\r\nLocation: http://evil.example/",
        Vec::new(),
    )
    .await;
    assert_eq!(cdn.get("/avatars/1/x.png").await, Err("status"));
}

#[cfg(feature = "test-support")]
#[tokio::test(start_paused = true)]
async fn run_refreshes_after_ready_and_stops() {
    let dir = TempDir::new();
    let cache = Arc::new(GuildCache::new(Id::new(7)));
    let (stop, stopped) = watch::channel(false);
    let stub = Stub {
        user: Some(user(Some(AVATAR), None)),
        images: HashMap::from([("/avatars/", image("image/png", b"PNG"))]),
        ..Stub::default()
    };
    let task = tokio::spawn(run(stub, Arc::clone(&cache), dir.0.clone(), stopped));
    tokio::time::sleep(SETTLE * 2).await;
    assert!(!files::cached(&dir.0, "avatar"), "nothing before READY");
    cache.set_self_user(&user(Some(AVATAR), None));
    tokio::time::sleep(SETTLE * 2).await;
    assert!(files::cached(&dir.0, "avatar"));
    stop.send_replace(true);
    task.await.unwrap();
}
