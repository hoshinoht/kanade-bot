//! Static PWA shells with SPA fallback, boss art and identity art.

use std::sync::Arc;

use kanade::api::{
    listeners::Site,
    state::{ChannelEntry, ChannelList},
};

use crate::support::{self, ADMIN_HOST, Fixture, PUBLIC_HOST, SECRET, get, read, request};

#[tokio::test]
async fn each_origin_serves_its_own_shell_with_spa_fallback() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let admin = support::admin(&http).await;
    let public = support::public(&http).await;

    for path in ["/", "/week", "/runs/r1/sheet", "/config?section=pings"] {
        let reply = get(admin, ADMIN_HOST, path).await;
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(
            reply.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(reply.text(), "<!doctype html>admin shell");
    }
    assert_eq!(
        get(public, PUBLIC_HOST, "/week").await.text(),
        "<!doctype html>public shell"
    );
    assert_eq!(
        get(public, PUBLIC_HOST, "/offline.html").await.text(),
        "<!doctype html>napping"
    );

    let script = get(admin, ADMIN_HOST, "/assets/app-abc123.js").await;
    assert_eq!(
        script.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        script.body,
        read(&fixture.path("web/apps/admin/dist/assets/app-abc123.js"))
    );
    assert_eq!(
        get(admin, ADMIN_HOST, "/manifest.webmanifest")
            .await
            .header("content-type"),
        Some("application/manifest+json")
    );

    // A missing asset is a 404, never the HTML shell; neither app sees the other's files.
    for (address, host, path) in [
        (admin, ADMIN_HOST, "/assets/missing.js"),
        (public, PUBLIC_HOST, "/assets/app-abc123.js"),
        (admin, ADMIN_HOST, "/offline.html"),
    ] {
        let reply = get(address, host, path).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.api_error(), "not_found");
    }

    let head = request(admin, "HEAD", ADMIN_HOST, "/", &[]).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    let post = request(
        admin,
        "POST",
        ADMIN_HOST,
        "/week",
        &[("Content-Length", "0")],
    )
    .await;
    assert_eq!(post.status, 405);
}

#[tokio::test]
async fn traversal_and_symlink_escapes_are_refused() {
    let fixture = Fixture::new();
    let admin = support::admin(&fixture.http()).await;
    for path in [
        "/../secret.txt",
        "/../../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2e%2e%2f%2e%2e%2fsecret.txt",
        "/assets/..%2f..%2fsecret.txt",
        "/assets/%2e%2e/%2e%2e/secret.txt",
        "/..%5c..%5csecret.txt",
        "/assets/escape.js",
        "/art/portraits/..%2f..%2fsecret",
        "/art/portraits/../secret",
        "/art/entry/%2e%2e",
        "/art/portraits/Carling.png",
        "/art/bogus/Carling",
        "/art/portraits/Escape",
        "//api/admin/week",
        "/API/admin/week",
        "//art/portraits/Carling",
    ] {
        let reply = get(admin, ADMIN_HOST, path).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.api_error(), "not_found", "{path}");
        assert!(!reply.text().contains(SECRET), "{path}");
    }
}

#[tokio::test]
async fn admin_serves_catalog_art_and_missing_art_is_absent() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    let admin = support::admin(&http).await;

    // Catalog keys are mixed case, like v4's art file names.
    let portrait = get(admin, ADMIN_HOST, "/art/portraits/Carling").await;
    assert_eq!(portrait.status, 200);
    assert_eq!(portrait.header("content-type"), Some("image/png"));
    assert_eq!(
        portrait.body,
        read(&fixture.path("boss/portraits/Carling.png"))
    );
    let entry = get(admin, ADMIN_HOST, "/art/entry/Carling").await;
    assert_eq!(entry.header("content-type"), Some("image/webp"));

    for path in ["/art/icons/Carling", "/art/entry/missing", "/art/portraits"] {
        let reply = get(admin, ADMIN_HOST, path).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.api_error(), "not_found");
    }

    http.boss_dir = None;
    http.web_dir = None;
    let bare = support::admin(&http).await;
    assert_eq!(
        get(bare, ADMIN_HOST, "/art/portraits/Carling").await.status,
        404
    );
    assert_eq!(get(bare, ADMIN_HOST, "/").await.status, 404);
}

#[tokio::test]
async fn identity_uses_cached_art_or_generated_stand_ins() {
    let fixture = Fixture::new();
    let mut http = fixture.http();
    let admin = support::admin(&http).await;
    let identity = get(admin, ADMIN_HOST, "/api/identity").await.json();
    let version = identity["version"].as_str().unwrap().to_owned();
    assert_eq!(version.len(), 12);
    assert_eq!(
        identity,
        serde_json::json!({
            "name": "Kanade",
            "avatar": format!("/identity/avatar?v={version}"),
            "banner": format!("/identity/banner?v={version}"),
            "cached": false,
            "version": version,
            "bot_user_id": null,
        })
    );
    let avatar = get(admin, ADMIN_HOST, "/identity/avatar").await;
    assert_eq!(avatar.header("content-type"), Some("image/svg+xml"));
    assert!(avatar.text().contains(">K</text>"));

    let dir = fixture.path("identity");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("avatar.png"), b"\x89PNG avatar").unwrap();
    http.identity_dir = Some(dir.clone());
    let cached = support::admin(&http).await;
    let identity = get(cached, ADMIN_HOST, "/api/identity").await.json();
    assert_eq!(identity["cached"], true);
    assert_ne!(identity["version"], version, "art changes the version");
    let avatar = get(cached, ADMIN_HOST, "/identity/avatar").await;
    assert_eq!(avatar.body, b"\x89PNG avatar");
    assert_eq!(
        avatar.header("cache-control"),
        Some("public, max-age=86400, must-revalidate")
    );
    let etag = avatar.header("etag").unwrap().to_owned();
    let revalidated = request(
        cached,
        "GET",
        ADMIN_HOST,
        "/identity/avatar?v=x",
        &[("If-None-Match", &etag)],
    )
    .await;
    assert_eq!(revalidated.status, 304);
    assert!(revalidated.body.is_empty());
    assert_eq!(
        get(cached, ADMIN_HOST, "/identity/banner")
            .await
            .header("content-type"),
        Some("image/svg+xml")
    );

    // A refresh replaces the file (temp + rename): new version, new ETag.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(dir.join(".avatar.tmp"), b"RIFF webp avatar").unwrap();
    std::fs::rename(dir.join(".avatar.tmp"), dir.join("avatar.webp")).unwrap();
    std::fs::remove_file(dir.join("avatar.png")).unwrap();
    let refreshed = get(cached, ADMIN_HOST, "/api/identity").await.json();
    assert_ne!(refreshed["version"], identity["version"]);
    let avatar = get(cached, ADMIN_HOST, "/identity/avatar").await;
    assert_eq!(avatar.header("content-type"), Some("image/webp"));
    assert_ne!(avatar.header("etag"), Some(etag.as_str()));
}

struct NamedBot;

impl ChannelList for NamedBot {
    fn channels(&self) -> Vec<ChannelEntry> {
        Vec::new()
    }

    fn bot_name(&self) -> Option<String> {
        Some("Yoisaki".into())
    }
}

#[tokio::test]
async fn public_identity_shows_the_live_name_but_never_the_bot_id() {
    let fixture = Fixture::new();
    let http = fixture.http();
    let mut site = Site::public(&http).unwrap();
    let before = support::spawn(site.clone()).await;
    assert_eq!(
        get(before, PUBLIC_HOST, "/api/identity").await.json()["name"],
        "Kanade",
        "configured name before READY"
    );
    site.bot = Some(Arc::new(NamedBot));
    let public = support::spawn(site).await;
    let identity = get(public, PUBLIC_HOST, "/api/identity").await.json();
    assert_eq!(identity["name"], "Yoisaki");
    assert_eq!(identity["bot_user_id"], serde_json::Value::Null);
    let avatar = get(public, PUBLIC_HOST, "/identity/avatar").await;
    assert!(avatar.text().contains(">Y</text>"));
}

/// Ten invented bytes; never the private art.
const CLIP: &[u8] = b"0123456789";

fn with_clip(fixture: &Fixture) {
    let dir = fixture.path("boss/artwork/animated");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Carling.mp4"), CLIP).unwrap();
}

#[tokio::test]
async fn animated_art_serves_mp4_with_byte_ranges() {
    let fixture = Fixture::new();
    with_clip(&fixture);
    let admin = support::admin(&fixture.http()).await;
    let path = "/art/animated/Carling";

    let full = get(admin, ADMIN_HOST, path).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.body, CLIP);
    assert_eq!(full.header("content-type"), Some("video/mp4"));
    assert_eq!(full.header("accept-ranges"), Some("bytes"));
    assert_eq!(full.header("content-length"), Some("10"));
    assert_eq!(full.header("cache-control"), Some("public, max-age=3600"));
    let etag = full.header("etag").unwrap().to_owned();

    for (range, status, content_range, body) in [
        ("bytes=2-5", 206, Some("bytes 2-5/10"), &b"2345"[..]),
        ("bytes=7-", 206, Some("bytes 7-9/10"), b"789"),
        ("bytes=-3", 206, Some("bytes 7-9/10"), b"789"),
        ("bytes=8-99", 206, Some("bytes 8-9/10"), b"89"),
        ("bytes=10-", 416, Some("bytes */10"), b""),
        ("bytes=-0", 416, Some("bytes */10"), b""),
        // Multi-range and malformed headers are ignored: the whole file.
        ("bytes=0-1,4-5", 200, None, CLIP),
        ("bytes=5-2", 200, None, CLIP),
        ("lines=0-1", 200, None, CLIP),
    ] {
        let reply = request(admin, "GET", ADMIN_HOST, path, &[("Range", range)]).await;
        assert_eq!(reply.status, status, "{range}");
        assert_eq!(reply.header("content-range"), content_range, "{range}");
        assert_eq!(reply.body, body, "{range}");
        assert_eq!(reply.header("accept-ranges"), Some("bytes"), "{range}");
        assert_eq!(
            reply.header("content-length"),
            Some(body.len().to_string().as_str()),
            "{range}"
        );
        if status == 416 {
            assert_eq!(reply.header("cache-control"), Some("no-store"));
        } else {
            assert_eq!(reply.header("content-type"), Some("video/mp4"));
        }
    }

    // A stale `If-Range` validator gets the whole (changed) file; a current one gets the range.
    let stale = request(
        admin,
        "GET",
        ADMIN_HOST,
        path,
        &[("Range", "bytes=0-1"), ("If-Range", "\"old\"")],
    )
    .await;
    assert_eq!((stale.status, stale.body.as_slice()), (200, CLIP));
    let current = request(
        admin,
        "GET",
        ADMIN_HOST,
        path,
        &[("Range", "bytes=0-1"), ("If-Range", &etag)],
    )
    .await;
    assert_eq!((current.status, current.body.as_slice()), (206, &b"01"[..]));
    let revalidated = request(admin, "GET", ADMIN_HOST, path, &[("If-None-Match", &etag)]).await;
    assert_eq!(revalidated.status, 304);
    assert!(revalidated.body.is_empty());

    let head = request(admin, "HEAD", ADMIN_HOST, path, &[("Range", "bytes=0-3")]).await;
    assert_eq!(head.status, 206);
    assert_eq!(head.header("content-range"), Some("bytes 0-3/10"));
    assert!(head.body.is_empty());
}

#[tokio::test]
async fn still_kinds_never_serve_video_and_animated_never_serves_stills() {
    let fixture = Fixture::new();
    with_clip(&fixture);
    for (relative, body) in [
        ("boss/portraits/Reel.mp4", CLIP),
        ("boss/portraits/icon/Reel.mp4", CLIP),
        ("boss/artwork/entry/Reel.mp4", CLIP),
        ("boss/artwork/animated/Still.png", b"\x89PNG still"),
    ] {
        let path = fixture.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.path("secret.txt"),
        fixture.path("boss/artwork/animated/Escape.mp4"),
    )
    .unwrap();
    let admin = support::admin(&fixture.http()).await;
    for path in [
        "/art/portraits/Reel",
        "/art/icons/Reel",
        "/art/entry/Reel",
        "/art/animated/Still",
        "/art/animated/Carling.mp4",
        "/art/animated/Escape",
        "/art/animated/..%2f..%2fsecret",
        "/art/animated/%2e%2e",
        "/art/Animated/Carling",
        "/art/animated/Ca.rling",
    ] {
        let reply = request(admin, "GET", ADMIN_HOST, path, &[("Range", "bytes=0-1")]).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.api_error(), "not_found", "{path}");
        assert!(!reply.text().contains(SECRET), "{path}");
    }
}

/// Invented stand-ins for the build's siblings: the server never decodes them.
fn with_siblings(fixture: &Fixture) {
    let dist = fixture.path("web/apps/admin/dist");
    std::fs::write(dist.join("assets/app-abc123.js.br"), b"BR-BYTES").unwrap();
    std::fs::write(dist.join("assets/app-abc123.js.gz"), b"GZIP-BYTES").unwrap();
    // The shell has only a gzip sibling (the build drops a variant that is not smaller).
    std::fs::write(dist.join("index.html.gz"), b"GZIP-SHELL").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.path("secret.txt"),
        dist.join("manifest.webmanifest.br"),
    )
    .unwrap();
}

#[tokio::test]
async fn static_files_serve_the_best_precompressed_sibling() {
    let fixture = Fixture::new();
    with_siblings(&fixture);
    let admin = support::admin(&fixture.http()).await;
    let script = "/assets/app-abc123.js";
    let raw = read(&fixture.path("web/apps/admin/dist/assets/app-abc123.js"));

    for (accept, encoding, body) in [
        (
            Some("gzip, deflate, br, zstd"),
            Some("br"),
            &b"BR-BYTES"[..],
        ),
        (Some("br"), Some("br"), b"BR-BYTES"),
        (Some("gzip"), Some("gzip"), b"GZIP-BYTES"),
        (Some("br;q=0, gzip"), Some("gzip"), b"GZIP-BYTES"),
        (Some("br;q=0.5, gzip"), Some("gzip"), b"GZIP-BYTES"),
        (Some("*"), Some("br"), b"BR-BYTES"),
        (Some("br;q=0, gzip;q=0"), None, &raw),
        (Some("*;q=0"), None, &raw),
        (Some("identity"), None, &raw),
        (Some("deflate"), None, &raw),
        (None, None, &raw),
    ] {
        let headers: Vec<(&str, &str)> =
            accept.map(|a| ("Accept-Encoding", a)).into_iter().collect();
        let reply = request(admin, "GET", ADMIN_HOST, script, &headers).await;
        assert_eq!(reply.status, 200, "{accept:?}");
        assert_eq!(reply.header("content-encoding"), encoding, "{accept:?}");
        assert_eq!(reply.body, body, "{accept:?}");
        assert_eq!(
            reply.header("content-type"),
            Some("text/javascript; charset=utf-8"),
            "{accept:?}"
        );
        assert_eq!(
            reply.header("content-length"),
            Some(body.len().to_string().as_str()),
            "{accept:?}"
        );
        assert_eq!(reply.header("vary"), Some("Accept-Encoding"), "{accept:?}");
        assert_eq!(
            reply.header("cache-control"),
            Some("public, max-age=31536000, immutable"),
            "{accept:?}"
        );
    }

    let head = request(
        admin,
        "HEAD",
        ADMIN_HOST,
        script,
        &[("Accept-Encoding", "br")],
    )
    .await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("content-encoding"), Some("br"));
    assert_eq!(head.header("content-length"), Some("8"));
    assert_eq!(head.header("vary"), Some("Accept-Encoding"));
    assert!(head.body.is_empty());
}

#[tokio::test]
async fn the_spa_shell_is_negotiated_and_missing_siblings_fall_back_to_identity() {
    let fixture = Fixture::new();
    with_siblings(&fixture);
    let admin = support::admin(&fixture.http()).await;

    for path in ["/", "/week", "/index.html"] {
        let gzip = request(
            admin,
            "GET",
            ADMIN_HOST,
            path,
            &[("Accept-Encoding", "gzip, br")],
        )
        .await;
        assert_eq!(gzip.header("content-encoding"), Some("gzip"), "{path}");
        assert_eq!(gzip.body, b"GZIP-SHELL", "{path}");
        assert_eq!(
            gzip.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(gzip.header("vary"), Some("Accept-Encoding"), "{path}");
        assert_eq!(gzip.header("cache-control"), Some("no-cache"), "{path}");

        // No br sibling: identity, still varying.
        let br = request(admin, "GET", ADMIN_HOST, path, &[("Accept-Encoding", "br")]).await;
        assert_eq!(br.header("content-encoding"), None, "{path}");
        assert_eq!(br.text(), "<!doctype html>admin shell", "{path}");
        assert_eq!(br.header("vary"), Some("Accept-Encoding"), "{path}");
        assert_eq!(br.header("cache-control"), Some("no-cache"), "{path}");
    }

    // A symlinked sibling is never followed.
    let manifest = request(
        admin,
        "GET",
        ADMIN_HOST,
        "/manifest.webmanifest",
        &[("Accept-Encoding", "br")],
    )
    .await;
    assert_eq!(manifest.header("content-encoding"), None);
    assert_eq!(manifest.text(), "{}");
    assert_eq!(manifest.header("vary"), Some("Accept-Encoding"));

    // The siblings are encodings, not resources: a direct request is a 404.
    for path in [
        "/assets/app-abc123.js.br",
        "/assets/app-abc123.js.gz",
        "/index.html.GZ",
    ] {
        let reply = request(admin, "GET", ADMIN_HOST, path, &[("Accept-Encoding", "br")]).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.api_error(), "not_found", "{path}");
    }

    // Images are never negotiated.
    let portrait = request(
        admin,
        "GET",
        ADMIN_HOST,
        "/art/portraits/Carling",
        &[("Accept-Encoding", "br, gzip")],
    )
    .await;
    assert_eq!(portrait.header("content-encoding"), None);
    assert_eq!(portrait.header("vary"), None);
}
