//! The bot's avatar and banner for the admin/public masthead (v4
//! `cache_identity`): after each `READY` and on the bot's own profile
//! changes, fetch them from Discord's CDN into `KANADE_IDENTITY_DIR`, which
//! `api::assets` serves. Purely cosmetic: failures are logged and leave the
//! last cached files in place.

mod cdn;
mod files;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use twilight_model::user::CurrentUser;
use twilight_model::util::ImageHash;

use crate::bot::guild_cache::{GuildCache, SelfAvatar};
use crate::bot::transport::{DiscordTransport, Outcome};
use crate::runtime::logging;

pub use cdn::Cdn;

/// Largest accepted image.
pub const MAX_IMAGE: usize = 8 * 1024 * 1024;
/// A `READY` is followed by `GUILD_CREATE` (guild avatar, nickname): one refresh for both.
const SETTLE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// The bot's profile and its CDN images.
pub trait IdentitySource: Send + Sync + 'static {
    fn current_user(&self) -> impl Future<Output = Option<CurrentUser>> + Send;
    /// A CDN path with query; errors are short log reasons.
    fn image(&self, path: &str) -> impl Future<Output = Result<Image, &'static str>> + Send;
}

/// Production source: the REST transport and the CDN.
pub struct DiscordSource<T> {
    pub transport: Arc<T>,
    pub cdn: Cdn,
}

impl<T: DiscordTransport + 'static> IdentitySource for DiscordSource<T> {
    async fn current_user(&self) -> Option<CurrentUser> {
        match self.transport.current_user().await {
            Outcome::Delivered(user) => Some(user),
            _ => None,
        }
    }

    async fn image(&self, path: &str) -> Result<Image, &'static str> {
        self.cdn.get(path).await
    }
}

/// The file extension for an accepted image.
pub fn accept(image: &Image) -> Result<&'static str, &'static str> {
    if image.bytes.is_empty() || image.bytes.len() > MAX_IMAGE {
        return Err("size");
    }
    let mime = image.content_type.split(';').next().unwrap_or_default();
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => Ok("png"),
        "image/webp" => Ok("webp"),
        "image/gif" => Ok("gif"),
        "image/jpeg" => Ok("jpg"),
        _ => Err("content_type"),
    }
}

fn file(hash: ImageHash) -> String {
    let ext = if hash.is_animated() { "gif" } else { "png" };
    format!("{hash}.{ext}")
}

fn failed(kind: &'static str, reason: &'static str) {
    logging::event(
        "WARN",
        "identity_refresh_failed",
        json!({"kind": kind, "reason": reason}),
    );
}

/// One refresh: guild avatar else user avatar (none: the monogram), and the
/// banner (none: the wash).
pub async fn refresh(source: &impl IdentitySource, cache: &GuildCache, dir: &Path) {
    let Some(user) = source.current_user().await else {
        failed("profile", "unavailable");
        return;
    };
    let avatar = match cache.self_avatar() {
        Some(SelfAvatar::Guild(hash)) => Some(format!(
            "/guilds/{}/users/{}/avatars/{}?size=256",
            cache.guild_id(),
            user.id,
            file(hash)
        )),
        _ => user
            .avatar
            .map(|hash| format!("/avatars/{}/{}?size=256", user.id, file(hash))),
    };
    let banner = user
        .banner
        .map(|hash| format!("/banners/{}/{}?size=600", user.id, file(hash)));
    for (stem, path) in [("avatar", avatar), ("banner", banner)] {
        let written = match path {
            None => files::clear(dir, stem, None),
            Some(path) => match source.image(&path).await.and_then(|image| {
                let ext = accept(&image)?;
                Ok((ext, image.bytes))
            }) {
                Ok((ext, bytes)) => files::store(dir, stem, ext, &bytes),
                Err(reason) => {
                    failed(stem, reason);
                    continue;
                }
            },
        };
        if written.is_err() {
            failed(stem, "write");
        }
    }
    logging::event(
        "INFO",
        "identity_cached",
        json!({"avatar": files::cached(dir, "avatar"), "banner": files::cached(dir, "banner")}),
    );
}

/// Refresh on each profile change until `stopped`.
pub async fn run(
    source: impl IdentitySource,
    cache: Arc<GuildCache>,
    dir: PathBuf,
    mut stopped: watch::Receiver<bool>,
) {
    let mut changes = cache.profile_changes();
    loop {
        tokio::select! {
            _ = stopped.wait_for(|stop| *stop) => return,
            changed = changes.changed() => if changed.is_err() { return },
        }
        tokio::select! {
            _ = stopped.wait_for(|stop| *stop) => return,
            () = tokio::time::sleep(SETTLE) => {}
        }
        changes.mark_unchanged();
        tokio::select! {
            _ = stopped.wait_for(|stop| *stop) => return,
            () = refresh(&source, &cache, &dir) => {}
        }
    }
}

/// The refresh worker, or `None` without `KANADE_IDENTITY_DIR` (or when the
/// directory cannot be created).
pub fn spawn<T: DiscordTransport + 'static>(
    dir: Option<&Path>,
    transport: Arc<T>,
    cache: Arc<GuildCache>,
    stopped: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let dir = dir?.to_path_buf();
    if files::ensure_dir(&dir).is_err() {
        failed("dir", "create");
        return None;
    }
    let Some(cdn) = Cdn::new() else {
        failed("tls", "no_provider");
        return None;
    };
    let source = DiscordSource { transport, cdn };
    Some(tokio::spawn(run(source, cache, dir, stopped)))
}

#[cfg(test)]
mod tests;
