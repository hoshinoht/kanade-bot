//! Member and admin portraits (user decisions 2026-10-02 and 2026-10-05):
//! Discord CDN avatars fetched on first use and cached by image hash in
//! `<KANADE_IDENTITY_DIR>/members/` on the data volume, served same-origin by
//! the admin routes (`admin::avatars`). A member who leaves is purged. No
//! avatar, no cache directory or a failed fetch answers the initial-letter
//! monogram, so a portrait never breaks.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use ring::digest::{SHA256, digest};
use tokio::sync::Semaphore;

use super::assets::monogram;
use crate::bot::identity::{self, Cdn, Image, files};

/// Requested edge in pixels: a 2x portrait for the largest place it shows.
pub const SIZE: u16 = 128;
/// Largest stored avatar; a 128 px PNG is tens of kilobytes.
pub const MAX_AVATAR: usize = 512 * 1024;
/// A failed fetch is not retried for this long (the monogram shows meanwhile).
const RETRY_AFTER: Duration = Duration::from_secs(300);
/// Remembered failures; past this the memory is simply cleared.
const MAX_FAILURES: usize = 4096;
/// Fetched images held in memory while the directory refuses writes.
const MAX_HELD: usize = 64;
/// Concurrent CDN fetches (a Members page asks for every row at once).
const FETCHES: usize = 4;
/// Cached files are `<user id>-<hash>.<ext>`.
const SUFFIXES: [&str; 4] = ["png", "webp", "gif", "jpg"];

/// Which CDN image is a user's portrait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvatarRef {
    user_id: String,
    hash: String,
    /// CDN path with query.
    path: String,
}

fn snowflake(text: &str) -> bool {
    (1..=20).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn image_hash(text: &str) -> bool {
    let hex = text.strip_prefix("a_").unwrap_or(text);
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl AvatarRef {
    /// The user avatar. Animated hashes still ask for PNG: the first frame.
    pub fn user(user_id: &str, hash: &str) -> Option<Self> {
        (snowflake(user_id) && image_hash(hash)).then(|| Self {
            user_id: user_id.to_owned(),
            hash: hash.to_owned(),
            path: format!("/avatars/{user_id}/{hash}.png?size={SIZE}"),
        })
    }

    /// A guild member's own avatar for this guild.
    pub fn member(guild_id: &str, user_id: &str, hash: &str) -> Option<Self> {
        (snowflake(guild_id) && snowflake(user_id) && image_hash(hash)).then(|| Self {
            user_id: user_id.to_owned(),
            hash: hash.to_owned(),
            path: format!("/guilds/{guild_id}/users/{user_id}/avatars/{hash}.png?size={SIZE}"),
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    fn stem(&self) -> String {
        format!("{}-{}", self.user_id, self.hash)
    }
}

pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Image, &'static str>> + Send + 'a>>;

/// Where avatar bytes come from: the Discord CDN in serve, a stub in tests.
pub trait AvatarFetch: Send + Sync {
    fn fetch<'a>(&'a self, path: &'a str) -> FetchFuture<'a>;
}

impl AvatarFetch for Cdn {
    fn fetch<'a>(&'a self, path: &'a str) -> FetchFuture<'a> {
        Box::pin(self.get(path))
    }
}

/// The file extension for an image that is small enough, declares an image
/// type and starts with that type's signature; anything else is refused, so
/// stored bytes can only ever be served as one of four image types.
pub fn accept(image: &Image) -> Result<&'static str, &'static str> {
    if image.bytes.len() > MAX_AVATAR {
        return Err("size");
    }
    let ext = identity::accept(image)?;
    let bytes = image.bytes.as_slice();
    let signed = match ext {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "jpg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    };
    if signed { Ok(ext) } else { Err("signature") }
}

fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/jpeg",
    }
}

/// A cached or freshly fetched portrait: content type and bytes.
pub type Portrait = (&'static str, Vec<u8>);

pub struct AvatarCache {
    /// `None`: nothing is fetched and every portrait is the monogram.
    dir: Option<PathBuf>,
    fetch: Option<Box<dyn AvatarFetch>>,
    failed: Mutex<HashMap<String, Instant>>,
    /// Images the directory would not store, served from memory for
    /// [`RETRY_AFTER`] so an unwritable cache never refetches on every view.
    held: Mutex<HashMap<String, (Instant, Portrait)>>,
    /// The purge sequence each departed user last saw: a fetch that spans a
    /// purge discards what it stored.
    purges: Mutex<HashMap<String, u64>>,
    purge_seq: AtomicU64,
    /// One fetch per image at a time, so concurrent first views share it.
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    fetches: Semaphore,
}

impl std::fmt::Debug for AvatarCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvatarCache")
            .field("dir", &self.dir)
            .field("fetch", &self.fetch.is_some())
            .finish_non_exhaustive()
    }
}

impl AvatarCache {
    /// The cache under `dir`, created owner-only. A directory that cannot be
    /// created leaves the cache off (monograms only).
    pub fn new(dir: Option<PathBuf>, fetch: Option<Box<dyn AvatarFetch>>) -> Self {
        let dir = dir.filter(|dir| files::ensure_dir(dir).is_ok());
        Self {
            dir,
            fetch,
            failed: Mutex::default(),
            held: Mutex::default(),
            purges: Mutex::default(),
            purge_seq: AtomicU64::new(0),
            inflight: Mutex::default(),
            fetches: Semaphore::new(FETCHES),
        }
    }

    /// Serve's cache: `<identity dir>/members` fed by the Discord CDN.
    pub fn discord(identity_dir: Option<&Path>) -> Self {
        let fetch = Cdn::new().map(|cdn| Box::new(cdn) as Box<dyn AvatarFetch>);
        Self::new(identity_dir.map(|dir| dir.join("members")), fetch)
    }

    fn failures(&self) -> std::sync::MutexGuard<'_, HashMap<String, Instant>> {
        self.failed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// An image held in memory because the directory refused it.
    fn held(&self, stem: &str) -> Option<Portrait> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.get(stem)
            .filter(|(at, _)| at.elapsed() < RETRY_AFTER)
            .map(|(_, portrait)| portrait.clone())
    }

    fn purge_mark(&self, user_id: &str) -> Option<u64> {
        self.purges
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(user_id)
            .copied()
    }

    fn recently_failed(&self, stem: &str) -> bool {
        self.failures()
            .get(stem)
            .is_some_and(|at| at.elapsed() < RETRY_AFTER)
    }

    async fn cached(dir: &Path, stem: &str) -> Option<Portrait> {
        for ext in SUFFIXES {
            if let Ok(bytes) = tokio::fs::read(dir.join(format!("{stem}.{ext}"))).await {
                return Some((content_type(ext), bytes));
            }
        }
        None
    }

    /// The portrait for `avatar`, from the cache or the CDN; `None` means
    /// the monogram (no avatar, cache off, or the fetch failed recently).
    pub async fn portrait(&self, avatar: Option<&AvatarRef>) -> Option<Portrait> {
        let (avatar, dir) = (avatar?, self.dir.as_deref()?);
        let stem = avatar.stem();
        if let Some(found) = Self::cached(dir, &stem).await.or_else(|| self.held(&stem)) {
            return Some(found);
        }
        if self.recently_failed(&stem) {
            return None;
        }
        let fetch = self.fetch.as_deref()?;
        let gate = Arc::clone(
            self.inflight
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(stem.clone())
                .or_default(),
        );
        let held = gate.lock().await;
        let found = self.fetch_once(fetch, dir, avatar, stem.clone()).await;
        drop(held);
        let mut inflight = self.inflight.lock().unwrap_or_else(PoisonError::into_inner);
        // Only this request still holds the gate: nobody is waiting on it.
        if Arc::strong_count(&gate) <= 2 {
            inflight.remove(&stem);
        }
        found
    }

    async fn fetch_once(
        &self,
        fetch: &dyn AvatarFetch,
        dir: &Path,
        avatar: &AvatarRef,
        stem: String,
    ) -> Option<Portrait> {
        // Another request may have fetched it (or failed) while this one waited.
        if let Some(found) = Self::cached(dir, &stem).await.or_else(|| self.held(&stem)) {
            return Some(found);
        }
        if self.recently_failed(&stem) {
            return None;
        }
        let _permit = self.fetches.acquire().await.ok()?;
        let mark = self.purge_mark(&avatar.user_id);
        match fetch
            .fetch(avatar.path())
            .await
            .and_then(|image| Ok((accept(&image)?, image.bytes)))
        {
            Ok((ext, bytes)) => {
                let portrait = (content_type(ext), bytes);
                let stored = files::store(dir, &stem, ext, &portrait.1).is_ok();
                // The member left while this fetch ran: keep nothing of theirs.
                if self.purge_mark(&avatar.user_id) != mark {
                    let _ = remove_user(dir, &avatar.user_id, None);
                    return None;
                }
                self.failures().remove(&stem);
                if stored {
                    let _ = remove_user(dir, &avatar.user_id, Some(&stem));
                } else {
                    // Unwritable cache: hold it in memory for the back-off.
                    let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
                    if held.len() >= MAX_HELD {
                        held.clear();
                    }
                    held.insert(stem, (Instant::now(), portrait.clone()));
                }
                Some(portrait)
            }
            Err(reason) => {
                crate::runtime::logging::event(
                    "WARN",
                    "avatar_fetch_failed",
                    serde_json::json!({"reason": reason}),
                );
                let mut failed = self.failures();
                if failed.len() >= MAX_FAILURES {
                    failed.clear();
                }
                failed.insert(stem, Instant::now());
                None
            }
        }
    }

    /// Drop every cached image of a member who left, including one a fetch
    /// still in flight is about to store.
    pub fn purge(&self, user_id: &str) {
        if !snowflake(user_id) {
            return;
        }
        let seq = self.purge_seq.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut purges = self.purges.lock().unwrap_or_else(PoisonError::into_inner);
            if purges.len() >= MAX_FAILURES {
                purges.clear();
            }
            purges.insert(user_id.to_owned(), seq);
        }
        let prefix = format!("{user_id}-");
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|stem, _| !stem.starts_with(&prefix));
        if let Some(dir) = self.dir.as_deref() {
            let _ = remove_user(dir, user_id, None);
        }
    }
}

/// Remove `<user>-*` files except the `keep` stem's.
fn remove_user(dir: &Path, user_id: &str, keep: Option<&str>) -> io::Result<()> {
    let prefix = format!("{user_id}-");
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((stem, _)) = name.rsplit_once('.') else {
            continue;
        };
        if stem.starts_with(&prefix) && Some(stem) != keep {
            match fs::remove_file(dir.join(name)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
    }
    Ok(())
}

/// The portrait (or the monogram of `name`) with a content ETag; a matching
/// `If-None-Match` is a 304. Served as an image type only, never sniffed.
pub fn respond(portrait: Option<Portrait>, name: &str, request: &HeaderMap) -> Response {
    let (content_type, bytes) =
        portrait.unwrap_or_else(|| ("image/svg+xml", monogram(name).into_bytes()));
    let etag = format!(
        "\"{}\"",
        digest(&SHA256, &bytes).as_ref()[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let fresh = request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == etag));
    if fresh {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (header::ETAG, etag),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(content_type: &str, bytes: &[u8]) -> Image {
        Image {
            content_type: content_type.into(),
            bytes: bytes.to_vec(),
        }
    }

    use std::sync::atomic::AtomicUsize;

    const HASH: &str = "0123456789abcdef0123456789abcdef";

    /// Counts fetches; with a gate, each fetch waits for it to open.
    struct Counting {
        calls: Arc<AtomicUsize>,
        gate: Option<Arc<tokio::sync::Notify>>,
    }

    impl AvatarFetch for Counting {
        fn fetch<'a>(&'a self, _: &'a str) -> FetchFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if let Some(gate) = &self.gate {
                    gate.notified().await;
                }
                Ok(image("image/png", b"\x89PNG\r\n\x1a\nface"))
            })
        }
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanade-avatars-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files_in(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unwritable_cache_holds_the_image_instead_of_refetching() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = AvatarCache::new(
            Some(dir.clone()),
            Some(Box::new(Counting {
                calls: calls.clone(),
                gate: None,
            })),
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let avatar = AvatarRef::user("1003", HASH).unwrap();
        for _ in 0..3 {
            let (kind, bytes) = cache.portrait(Some(&avatar)).await.expect("shown");
            assert_eq!((kind, bytes.ends_with(b"face")), ("image/png", true));
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one CDN fetch");
        assert!(files_in(&dir).is_empty());
        cache.purge("1003");
        assert_eq!(cache.held(&avatar.stem()), None, "a leaver is not held");
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_fetch_that_spans_the_members_departure_keeps_nothing() {
        let dir = temp_dir();
        let gate = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let cache = Arc::new(AvatarCache::new(
            Some(dir.clone()),
            Some(Box::new(Counting {
                calls: calls.clone(),
                gate: Some(gate.clone()),
            })),
        ));
        let avatar = AvatarRef::user("1003", HASH).unwrap();
        let fetching = tokio::spawn({
            let (cache, avatar) = (cache.clone(), avatar.clone());
            async move { cache.portrait(Some(&avatar)).await }
        });
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        cache.purge("1003");
        gate.notify_one();
        assert_eq!(fetching.await.unwrap(), None, "the monogram");
        assert!(files_in(&dir).is_empty(), "{:?}", files_in(&dir));

        // A later fetch (the member came back) caches again.
        gate.notify_one();
        assert!(cache.portrait(Some(&avatar)).await.is_some());
        assert_eq!(files_in(&dir), [format!("1003-{HASH}.png")]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refs_take_only_snowflakes_and_image_hashes() {
        let hash = "0123456789abcdef0123456789abcdef";
        let user = AvatarRef::user("1003", hash).unwrap();
        assert_eq!(
            user.path(),
            format!("/avatars/1003/{hash}.png?size=128").as_str()
        );
        let member = AvatarRef::member("900", "1003", &format!("a_{hash}")).unwrap();
        assert_eq!(
            member.path(),
            format!("/guilds/900/users/1003/avatars/a_{hash}.png?size=128").as_str()
        );
        for (user_id, hash) in [
            ("../1", hash),
            ("", hash),
            ("1003", "../../etc"),
            ("1003", "0123456789ABCDEF0123456789ABCDEF"),
            ("1003", "clyde"),
        ] {
            assert_eq!(AvatarRef::user(user_id, hash), None, "{user_id} {hash}");
        }
    }

    #[test]
    fn only_signed_small_images_are_accepted() {
        assert_eq!(
            accept(&image("image/png", b"\x89PNG\r\n\x1a\nrest")),
            Ok("png")
        );
        assert_eq!(accept(&image("image/gif", b"GIF89a....")), Ok("gif"));
        assert_eq!(
            accept(&image("image/webp", b"RIFF\0\0\0\0WEBPVP8 ")),
            Ok("webp")
        );
        assert_eq!(accept(&image("image/jpeg", b"\xff\xd8\xff\xe0")), Ok("jpg"));
        assert_eq!(
            accept(&image("image/png", b"<html><script>")),
            Err("signature")
        );
        assert_eq!(
            accept(&image("text/html", b"\x89PNG\r\n\x1a\n")),
            Err("content_type")
        );
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_AVATAR + 1, 0);
        assert_eq!(accept(&image("image/png", &big)), Err("size"));
    }
}
