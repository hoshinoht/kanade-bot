//! Static PWA shells with SPA fallback, boss art and identity art.

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
    assert_eq!(
        identity,
        serde_json::json!({
            "name": "Kanade",
            "avatar": "/identity/avatar",
            "banner": "/identity/banner",
            "cached": false,
            "bot_user_id": null,
        })
    );
    let avatar = get(admin, ADMIN_HOST, "/identity/avatar").await;
    assert_eq!(avatar.header("content-type"), Some("image/svg+xml"));
    assert!(avatar.text().contains(">K</text>"));

    let dir = fixture.path("identity");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("avatar.png"), b"\x89PNG avatar").unwrap();
    http.identity_dir = Some(dir);
    let cached = support::admin(&http).await;
    assert_eq!(
        get(cached, ADMIN_HOST, "/api/identity").await.json()["cached"],
        true
    );
    let avatar = get(cached, ADMIN_HOST, "/identity/avatar").await;
    assert_eq!(avatar.body, b"\x89PNG avatar");
    assert_eq!(avatar.header("cache-control"), Some("public, max-age=3600"));
    assert_eq!(
        get(cached, ADMIN_HOST, "/identity/banner")
            .await
            .header("content-type"),
        Some("image/svg+xml")
    );
}
