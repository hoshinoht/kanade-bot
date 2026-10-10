//! `GET /api/public/events`: member-scoped change hints for an open portal
//! (`text/event-stream`, public-portal-plan § Live updates). The stream opens
//! with a `ready` event (`{boot}`), then sends `{topic}` messages and a
//! comment each heartbeat. Topics are decided here, per member, from the
//! admin hub's hints: `schedule` when the history head moved, `mine` when a
//! change touched the member's own runs, answers or timings (read from the
//! history records' rows), their requests or their ownership asks, and
//! `allowance` when their chat allowance row changed. No frame carries data,
//! a `seq` or anyone's id.
//!
//! As on the admin stream, holding it open is not activity: it is admitted by
//! [`MemberSession::require_quietly`] (no touch, no id rotation, so neither it
//! nor its reconnects ever extend the idle window), and every heartbeat only
//! reads the session row (sign-out, sign-out everywhere, idle and absolute
//! expiry end it) and asks the eligibility gate (a lost role ends every
//! session of the member, as the request path does). It also ends after the
//! hub's lifetime, on shutdown, when the portal closes and when the client
//! goes away.

use std::{
    collections::{BTreeSet, hash_map::DefaultHasher},
    convert::Infallible,
    hash::{Hash, Hasher},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    body::{Body, Bytes, HttpBody},
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use hyper::body::Frame;
use serde::Serialize;
use tokio::{
    sync::{
        broadcast::error::{RecvError, TryRecvError},
        mpsc,
    },
    time::{Instant, interval_at, sleep},
};

use crate::{
    api::{
        admin::{
            context::{state, unavailable},
            limits::{allowance_row, allowance_snapshot},
        },
        auth::{
            audit::AuditContext,
            member::{Eligibility, MemberAuth, MemberSession},
        },
        dto::events::{MemberHint, MemberReady, MemberTopic, Topic},
        error::ApiError,
        events::{EventsConfig, MemberSubscription},
        listeners::Site,
        state::ApiState,
    },
    domain::{
        history::{ChangeFilter, MAX_PAGE, RowChange, RowKey, RowValue},
        scheduler::{Scope, StoreError},
    },
    infrastructure::store::web_sessions::{LoginMethod, SessionOrigin},
};

/// How long the browser waits before reconnecting a stream that ended.
const RETRY_MS: u32 = 3_000;

pub(super) async fn events(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
) -> Response {
    let Some(auth) = site.member.clone() else {
        return ApiError::AUTH_UNAVAILABLE.into_response();
    };
    let state = match state(&site) {
        Ok(state) => Arc::clone(state),
        Err(error) => return error.into_response(),
    };
    if state.events.is_closed() {
        return ApiError::UNAVAILABLE.into_response();
    }
    // Subscribed before the baseline reads, so a change in between is both
    // in the baseline and hinted, never lost.
    let Some(subscription) = state.events.subscribe_member(&session.user_id) else {
        return ApiError::TOO_MANY_STREAMS.into_response();
    };
    let config = state.events.config();
    let watch = match Watch::open(state, auth, audit, &session).await {
        Ok(watch) => watch,
        Err(error) => return error.into_response(),
    };
    let (frames, body) = mpsc::channel(8);
    tokio::spawn(pump(frames, subscription, watch, config));
    let mut response = Body::new(Stream(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    // Cache-Control (`no-store, no-transform`) comes from the header layer,
    // which sets it on every event stream.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn data<T: Serialize>(event: Option<&str>, value: &T) -> Bytes {
    let json = serde_json::to_string(value).unwrap_or_default();
    match event {
        Some(name) => format!("event: {name}\ndata: {json}\n\n"),
        None => format!("data: {json}\n\n"),
    }
    .into()
}

/// The hub hint kinds a member stream looks into.
#[derive(Clone, Copy, Default)]
struct Kinds {
    schedule: bool,
    inbox: bool,
    allowance: bool,
    members: bool,
}

impl Kinds {
    /// After skipped hints: anything may have changed.
    const ALL: Self = Self {
        schedule: true,
        inbox: true,
        allowance: true,
        members: true,
    };

    fn add(&mut self, topic: Topic) {
        match topic {
            Topic::Schedule => self.schedule = true,
            Topic::Inbox => self.inbox = true,
            Topic::Chat | Topic::Settings => self.allowance = true,
            Topic::Members => {
                self.members = true;
                self.allowance = true;
            }
            Topic::Extraction | Topic::Rewrite | Topic::Delivery | Topic::Rescan => {}
        }
    }
}

/// One stream's member and what they last saw of each topic.
struct Watch {
    state: Arc<ApiState>,
    auth: Arc<MemberAuth>,
    audit: AuditContext,
    member: String,
    /// The session row the stream rides on (its id hash).
    session: String,
    /// The history head the member's views were last read at.
    head: u64,
    /// The weekly timings the member owns (their asks are the member's).
    owned: BTreeSet<String>,
    requests: u64,
    asks: u64,
    allowance: u64,
}

impl Watch {
    async fn open(
        state: Arc<ApiState>,
        auth: Arc<MemberAuth>,
        audit: AuditContext,
        session: &MemberSession,
    ) -> Result<Self, ApiError> {
        let head = state.store.head().await.map_err(unavailable)?.seq;
        let mut watch = Self {
            state,
            auth,
            audit,
            member: session.user_id.clone(),
            session: session.id_hash().to_owned(),
            head,
            owned: BTreeSet::new(),
            requests: 0,
            asks: 0,
            allowance: 0,
        };
        watch.owned = watch.owned().await.map_err(unavailable)?;
        watch.requests = watch.requests().await.map_err(unavailable)?;
        watch.asks = watch.asks().await.map_err(unavailable)?;
        watch.allowance = watch.allowance().await.map_err(unavailable)?;
        Ok(watch)
    }

    /// The session still stands: not signed out (here or everywhere), not
    /// past its idle or absolute expiry, the portal still open. A read only:
    /// it never touches the row.
    async fn live(&self) -> bool {
        if !self.auth.is_open() {
            return false;
        }
        let Ok(Some(row)) = self.auth.sessions().load_session(&self.session).await else {
            return false;
        };
        row.origin == SessionOrigin::Public
            && row.method == LoginMethod::Discord
            && row.subject == self.member
            && self.auth.live(&row, self.auth.now())
    }

    /// `false` once the member lost access; their sessions end with it, as
    /// on the request path. An unreadable gate keeps the stream.
    async fn eligible(&self) -> bool {
        if self.auth.gate().check(&self.member).await != Eligibility::NotEligible {
            return true;
        }
        let _ = self
            .auth
            .end_all(&self.audit, &self.member, "not_eligible")
            .await;
        false
    }

    /// The topics `kinds` changed for this member. A read that fails keeps
    /// the old baseline, so the next hint looks again.
    async fn changes(&mut self, kinds: Kinds) -> BTreeSet<MemberTopic> {
        let mut topics = BTreeSet::new();
        if kinds.schedule {
            self.schedule(&mut topics).await;
        }
        if kinds.inbox
            && let Ok(requests) = self.requests().await
            && requests != self.requests
        {
            self.requests = requests;
            topics.insert(MemberTopic::Mine);
        }
        if kinds.schedule || kinds.inbox {
            self.check_asks(&mut topics).await;
        }
        if kinds.allowance {
            self.check_allowance(&mut topics).await;
        }
        topics
    }

    /// Ownership asks write no history and send no hint, so each heartbeat
    /// looks at them.
    async fn check_asks(&mut self, topics: &mut BTreeSet<MemberTopic>) {
        if let Ok(asks) = self.asks().await
            && asks != self.asks
        {
            self.asks = asks;
            topics.insert(MemberTopic::Mine);
        }
    }

    /// The allowance row also moves without a hint the stream hears (its
    /// window and the role-driven quota), so each heartbeat looks at it too.
    async fn check_allowance(&mut self, topics: &mut BTreeSet<MemberTopic>) {
        if let Ok(allowance) = self.allowance().await
            && allowance != self.allowance
        {
            self.allowance = allowance;
            topics.insert(MemberTopic::Allowance);
        }
    }

    /// A failed look still hints `schedule` and `mine` (re-reading costs the
    /// member little; missing their own change costs more) and keeps the old
    /// head, so the next look tries again.
    async fn schedule(&mut self, topics: &mut BTreeSet<MemberTopic>) {
        let Ok(head) = self.state.store.head().await else {
            return;
        };
        if head.seq == self.head {
            return;
        }
        topics.insert(MemberTopic::Schedule);
        // A rewound history (a restore) may have changed anything.
        let mine = if head.seq < self.head {
            Ok(true)
        } else {
            self.touched(head.seq).await
        };
        match mine {
            Ok(false) => self.head = head.seq,
            Ok(true) => {
                topics.insert(MemberTopic::Mine);
                if let Ok(owned) = self.owned().await {
                    self.owned = owned;
                    self.head = head.seq;
                }
            }
            Err(_) => {
                topics.insert(MemberTopic::Mine);
            }
        }
    }

    /// Whether a record after the baseline and up to `head` wrote a row of
    /// the member's: a run or timing they are (or were) on, a timing they
    /// own, or one of their answers.
    async fn touched(&self, head: u64) -> Result<bool, StoreError> {
        let mut before = Some(head + 1);
        while let Some(cursor) = before {
            let left = usize::try_from(cursor - 1 - self.head).unwrap_or(MAX_PAGE);
            let page = self
                .state
                .store
                .history_page(ChangeFilter::All, Some(cursor), left.min(MAX_PAGE))
                .await?;
            for record in &page.records {
                if record.seq <= self.head {
                    return Ok(false);
                }
                if record.rows.iter().any(|row| concerns(row, &self.member)) {
                    return Ok(true);
                }
            }
            before = page.next_before.filter(|next| *next > self.head + 1);
        }
        Ok(false)
    }

    async fn owned(&self) -> Result<BTreeSet<String>, StoreError> {
        let snapshot = self.state.store.snapshot(Scope::Weeks(Vec::new())).await?;
        Ok(snapshot
            .fixed_runs
            .into_iter()
            .filter(|fixed| fixed.owner() == self.member)
            .map(|fixed| fixed.id)
            .collect())
    }

    /// The member's requests as they read them.
    async fn requests(&self) -> Result<u64, StoreError> {
        let requests = self
            .state
            .store
            .member_requests(self.member.clone())
            .await?;
        let mut print = DefaultHasher::new();
        for loaded in &requests {
            let draft = &loaded.draft;
            (&draft.id, draft.version, draft.status, draft.updated_at).hash(&mut print);
        }
        Ok(print.finish())
    }

    /// The open asks the member made, and those on timings they own.
    async fn asks(&self) -> Result<u64, StoreError> {
        let now = self.state.now();
        let open = self.state.store.open_owner_requests().await?;
        let theirs: BTreeSet<(&str, bool)> = open
            .iter()
            .filter(|ask| ask.requester == self.member || self.owned.contains(&ask.fixed_run_id))
            .map(|ask| (ask.id.as_str(), ask.live(now)))
            .collect();
        let mut print = DefaultHasher::new();
        theirs.hash(&mut print);
        Ok(print.finish())
    }

    /// The member's Limits row, without its clock-driven `resets_at`.
    async fn allowance(&self) -> Result<u64, StoreError> {
        let profile = self.state.store.member(self.member.clone()).await?;
        let (snapshot, now) = allowance_snapshot(&self.state);
        let row = profile.and_then(|profile| allowance_row(&self.state, &snapshot, now, &profile));
        let mut print = DefaultHasher::new();
        if let Some(row) = row {
            (row.staff, row.used, row.overridden).hash(&mut print);
            if let Some(quota) = row.allowance {
                (quota.count, quota.per_s.to_bits()).hash(&mut print);
            }
        }
        Ok(print.finish())
    }
}

/// Whether one written row is `member`'s: their answer, or a run or timing
/// naming them (party, owner or standing answer) before or after.
fn concerns(row: &RowChange, member: &str) -> bool {
    if let RowKey::Rsvp { user_id, .. } = &row.key {
        return user_id == member;
    }
    row.before
        .iter()
        .chain(row.after.iter())
        .any(|value| match value {
            RowValue::Run(run) => run.participants.iter().any(|id| id == member),
            RowValue::FixedRun(fixed) => {
                fixed.owner_id == member
                    || fixed.participants.iter().any(|id| id == member)
                    || fixed
                        .standing
                        .iter()
                        .any(|standing| standing.user_id == member)
            }
            RowValue::Rsvp(rsvp) => rsvp.user_id == member,
            RowValue::Reminder(_) => false,
        })
}

/// The frames for `topics`, in topic order.
fn hints(topics: &BTreeSet<MemberTopic>) -> Vec<u8> {
    let mut out = Vec::new();
    for &topic in topics {
        out.extend_from_slice(&data(None, &MemberHint { topic }));
    }
    out
}

/// Writes one stream until something ends it; dropping `frames` ends the body.
async fn pump(
    frames: mpsc::Sender<Bytes>,
    subscription: MemberSubscription,
    mut watch: Watch,
    config: EventsConfig,
) {
    let MemberSubscription {
        slot: _slot,
        hints: mut hints_in,
        boot,
        mut closed,
    } = subscription;
    let mut first = format!("retry: {RETRY_MS}\n").into_bytes();
    first.extend_from_slice(&data(Some("ready"), &MemberReady { boot }));
    if frames.send(first.into()).await.is_err() {
        return;
    }
    let lifetime = sleep(config.max_lifetime);
    tokio::pin!(lifetime);
    let mut heartbeat = interval_at(Instant::now() + config.heartbeat, config.heartbeat);
    let closing = async move {
        let _ = closed.wait_for(|closed| *closed).await;
    };
    tokio::pin!(closing);
    loop {
        let frame = tokio::select! {
            () = frames.closed() => return,
            () = &mut closing => return,
            () = &mut lifetime => return,
            _ = heartbeat.tick() => {
                if !watch.live().await || !watch.eligible().await {
                    return;
                }
                let mut topics = BTreeSet::new();
                watch.check_asks(&mut topics).await;
                watch.check_allowance(&mut topics).await;
                let mut frame = hints(&topics);
                frame.extend_from_slice(b": keep-alive\n\n");
                frame
            }
            hint = hints_in.recv() => {
                let mut kinds = Kinds::default();
                match hint {
                    Ok(hint) => kinds.add(hint.topic),
                    // Skipped hints: look at everything.
                    Err(RecvError::Lagged(_)) => kinds = Kinds::ALL,
                    Err(RecvError::Closed) => return,
                }
                // One look for a burst of commits.
                loop {
                    match hints_in.try_recv() {
                        Ok(hint) => kinds.add(hint.topic),
                        Err(TryRecvError::Lagged(_)) => kinds = Kinds::ALL,
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
                if kinds.members && !watch.eligible().await {
                    return;
                }
                let topics = watch.changes(kinds).await;
                if topics.is_empty() {
                    continue;
                }
                hints(&topics)
            }
        };
        // A reader that stops reading must not hold the shutdown drain.
        tokio::select! {
            sent = frames.send(frame.into()) => if sent.is_err() { return },
            () = &mut closing => return,
            () = &mut lifetime => return,
        }
    }
}

/// The response body: frames as the pump writes them, ending when it stops.
struct Stream(mpsc::Receiver<Bytes>);

impl HttpBody for Stream {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0
            .poll_recv(cx)
            .map(|bytes| bytes.map(|bytes| Ok(Frame::data(bytes))))
    }
}
