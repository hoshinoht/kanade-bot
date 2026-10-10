//! Token buckets per client IP and per route, plus one global bucket per
//! route, and per member a write bucket for the public origin's member
//! writes and a read bucket for its data reads. An IPv6 client is keyed by
//! its /64 (one subscriber's prefix), so rotating addresses within it share
//! a bucket. Time comes from the auth clock so tests pin it; state is bounded.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
    sync::Mutex,
};

use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    DiscordStart,
    DiscordCallback,
    TokenLogin,
    /// Charged only when a bearer token is wrong.
    BearerFailure,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiscordStart => "discord_start",
            Self::DiscordCallback => "discord_callback",
            Self::TokenLogin => "token_login",
            Self::BearerFailure => "bearer_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub burst: f64,
    pub per_minute: f64,
}

/// The public origin's member writes, per member: 20 per 10 minutes.
pub const MEMBER_WRITES: Rule = Rule {
    burst: 20.0,
    per_minute: 2.0,
};

/// The audited route name of a refused member write.
pub const MEMBER_WRITE_ROUTE: &str = "member_write";

/// The public origin's data reads, per member: 120 a minute, generous for a
/// page's burst of reads and its live refreshes.
pub const MEMBER_READS: Rule = Rule {
    burst: 120.0,
    per_minute: 120.0,
};

/// The audited route name of a refused member read.
pub const MEMBER_READ_ROUTE: &str = "member_read";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub per_ip: Rule,
    pub global: Rule,
}

impl Limits {
    pub fn for_route(route: Route) -> Self {
        let (ip, global) = match route {
            Route::DiscordStart | Route::DiscordCallback => (10.0, 60.0),
            Route::TokenLogin => (5.0, 20.0),
            Route::BearerFailure => (5.0, 30.0),
        };
        Self {
            per_ip: Rule {
                burst: ip,
                per_minute: ip,
            },
            global: Rule {
                burst: global,
                per_minute: global,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Buckets {
    Client,
    Global,
    Both,
}

/// Bounds memory: beyond this many tracked clients, idle ones are dropped and
/// then unknown clients go unremembered (the global bucket still bounds work).
const MAX_CLIENTS: usize = 4096;

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at: DateTime<Utc>,
}

impl Bucket {
    fn full(rule: Rule, now: DateTime<Utc>) -> Self {
        Self {
            tokens: rule.burst,
            at: now,
        }
    }

    fn refill(&mut self, rule: Rule, now: DateTime<Utc>) {
        let minutes = (now - self.at).num_milliseconds().max(0) as f64 / 60_000.0;
        self.tokens = (self.tokens + minutes * rule.per_minute).min(rule.burst);
        self.at = now;
    }
}

#[derive(Default)]
struct State {
    clients: HashMap<(Route, Option<IpAddr>), Bucket>,
    global: HashMap<Route, Bucket>,
    /// [`MEMBER_WRITES`] buckets by member id.
    members: HashMap<String, Bucket>,
    /// [`MEMBER_READS`] buckets by member id.
    readers: HashMap<String, Bucket>,
}

/// The bucket key of a client address: IPv4 (and IPv4-mapped IPv6) as is,
/// other IPv6 by its /64.
fn client_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let prefix = u128::from(v6) & !((1u128 << 64) - 1);
            IpAddr::V6(Ipv6Addr::from(prefix))
        }
        v4 => v4,
    }
}

pub struct RateLimits {
    limits: fn(Route) -> Limits,
    state: Mutex<State>,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self::new(Limits::for_route)
    }
}

impl RateLimits {
    pub fn new(limits: fn(Route) -> Limits) -> Self {
        Self {
            limits,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take one token from the client's and the global bucket; `false` (and
    /// nothing taken) when either is empty.
    pub fn take(&self, route: Route, client: Option<IpAddr>, now: DateTime<Utc>) -> bool {
        self.admit(route, client, now, true, Buckets::Both)
    }

    /// Whether [`Self::take`] would succeed, without taking.
    pub fn allows(&self, route: Route, client: Option<IpAddr>, now: DateTime<Utc>) -> bool {
        self.admit(route, client, now, false, Buckets::Both)
    }

    /// The client's bucket only.
    pub fn take_client(&self, route: Route, client: Option<IpAddr>, now: DateTime<Utc>) -> bool {
        self.admit(route, client, now, true, Buckets::Client)
    }

    pub fn allows_client(&self, route: Route, client: Option<IpAddr>, now: DateTime<Utc>) -> bool {
        self.admit(route, client, now, false, Buckets::Client)
    }

    /// The route's global bucket only.
    pub fn take_global(&self, route: Route, now: DateTime<Utc>) -> bool {
        self.admit(route, None, now, true, Buckets::Global)
    }

    /// Take one of `member`'s [`MEMBER_WRITES`] tokens; `false` (and nothing
    /// taken) when they are spent.
    pub fn take_member_write(&self, member: &str, now: DateTime<Utc>) -> bool {
        self.take_member(MEMBER_WRITES, |state| &mut state.members, member, now)
    }

    /// Take one of `member`'s [`MEMBER_READS`] tokens; `false` (and nothing
    /// taken) when they are spent.
    pub fn take_member_read(&self, member: &str, now: DateTime<Utc>) -> bool {
        self.take_member(MEMBER_READS, |state| &mut state.readers, member, now)
    }

    fn take_member(
        &self,
        rule: Rule,
        table: fn(&mut State) -> &mut HashMap<String, Bucket>,
        member: &str,
        now: DateTime<Utc>,
    ) -> bool {
        let mut state = self.state();
        let buckets = table(&mut state);
        let known = buckets.contains_key(member);
        if !known && buckets.len() >= MAX_CLIENTS {
            buckets.retain(|_, bucket| {
                bucket.refill(rule, now);
                bucket.tokens < rule.burst
            });
        }
        let mut bucket = buckets
            .get(member)
            .copied()
            .unwrap_or_else(|| Bucket::full(rule, now));
        bucket.refill(rule, now);
        let allowed = bucket.tokens >= 1.0;
        if allowed {
            bucket.tokens -= 1.0;
        }
        // As for clients: a still-full table serves a newcomer unremembered.
        if known || buckets.len() < MAX_CLIENTS {
            buckets.insert(member.to_owned(), bucket);
        }
        allowed
    }

    fn admit(
        &self,
        route: Route,
        client: Option<IpAddr>,
        now: DateTime<Utc>,
        take: bool,
        buckets: Buckets,
    ) -> bool {
        let limits = (self.limits)(route);
        let mut state = self.state();
        let key = (route, client.map(client_key));
        let use_client = buckets != Buckets::Global;
        let use_global = buckets != Buckets::Client;
        let mut remember = true;
        if use_client && !state.clients.contains_key(&key) && state.clients.len() >= MAX_CLIENTS {
            state.clients.retain(|(route, _), bucket| {
                let rule = (self.limits)(*route).per_ip;
                bucket.refill(rule, now);
                bucket.tokens < rule.burst
            });
            // Still full: serve this client from an unremembered full bucket
            // rather than refusing it, so a flood of addresses cannot lock out
            // a correct token; the global bucket bounds wrong guesses.
            remember = state.clients.len() < MAX_CLIENTS;
        }
        let mut global = *state
            .global
            .entry(route)
            .or_insert_with(|| Bucket::full(limits.global, now));
        let mut own = state
            .clients
            .get(&key)
            .copied()
            .unwrap_or_else(|| Bucket::full(limits.per_ip, now));
        global.refill(limits.global, now);
        own.refill(limits.per_ip, now);
        let allowed = (!use_global || global.tokens >= 1.0) && (!use_client || own.tokens >= 1.0);
        if allowed && take {
            if use_global {
                global.tokens -= 1.0;
            }
            if use_client {
                own.tokens -= 1.0;
            }
        }
        state.global.insert(route, global);
        if use_client && remember {
            state.clients.insert(key, own);
        }
        allowed
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn tiny(_: Route) -> Limits {
        Limits {
            per_ip: Rule {
                burst: 2.0,
                per_minute: 1.0,
            },
            global: Rule {
                burst: 3.0,
                per_minute: 1.0,
            },
        }
    }

    #[test]
    fn per_ip_then_global_buckets_limit_and_refill() {
        let limits = RateLimits::new(tiny);
        let now = DateTime::UNIX_EPOCH;
        let (a, b, c) = (
            Some([10, 0, 0, 1].into()),
            Some([10, 0, 0, 2].into()),
            Some([10, 0, 0, 3].into()),
        );
        assert!(limits.take(Route::TokenLogin, a, now));
        assert!(limits.take(Route::TokenLogin, a, now));
        assert!(!limits.take(Route::TokenLogin, a, now), "per-IP burst");
        assert!(limits.take(Route::TokenLogin, b, now));
        assert!(!limits.take(Route::TokenLogin, c, now), "global burst");
        assert!(
            limits.take(Route::DiscordStart, c, now),
            "routes are separate"
        );
        assert!(!limits.allows(Route::TokenLogin, a, now));
        let later = now + TimeDelta::minutes(1);
        assert!(limits.allows(Route::TokenLogin, a, later));
        assert!(limits.take(Route::TokenLogin, a, later));
    }

    #[test]
    fn a_full_client_table_serves_new_clients_without_remembering_them() {
        let limits = RateLimits::new(tiny);
        let now = DateTime::UNIX_EPOCH;
        for n in 0..MAX_CLIENTS as u32 {
            let ip = Some(IpAddr::from(n.to_be_bytes()));
            assert!(limits.take_client(Route::TokenLogin, ip, now));
        }
        let newcomer = Some([192, 168, 9, 9].into());
        assert!(limits.take_client(Route::TokenLogin, newcomer, now));
        assert!(limits.take_client(Route::TokenLogin, newcomer, now));
        assert_eq!(limits.state().clients.len(), MAX_CLIENTS);
    }

    #[test]
    fn member_writes_are_limited_per_member_and_refill() {
        let limits = RateLimits::default();
        let now = DateTime::UNIX_EPOCH;
        for _ in 0..20 {
            assert!(limits.take_member_write("1001", now));
        }
        assert!(!limits.take_member_write("1001", now), "20 per 10 minutes");
        assert!(
            limits.take_member_write("1002", now),
            "members are separate"
        );
        assert!(
            limits.take(Route::TokenLogin, None, now),
            "sign-in buckets are separate"
        );
        let later = now + TimeDelta::seconds(30);
        assert!(limits.take_member_write("1001", later), "two a minute");
        assert!(!limits.take_member_write("1001", later));
    }

    #[test]
    fn member_reads_are_limited_per_member_apart_from_writes() {
        let limits = RateLimits::default();
        let now = DateTime::UNIX_EPOCH;
        for _ in 0..120 {
            assert!(limits.take_member_read("1001", now));
        }
        assert!(!limits.take_member_read("1001", now), "120 a minute");
        assert!(limits.take_member_read("1002", now), "members are separate");
        assert!(
            limits.take_member_write("1001", now),
            "writes have their own bucket"
        );
        let later = now + TimeDelta::seconds(1);
        assert!(limits.take_member_read("1001", later), "two a second");
        assert!(limits.take_member_read("1001", later));
        assert!(!limits.take_member_read("1001", later));
    }

    #[test]
    fn ipv6_clients_share_their_slash_64_and_ipv4_stays_exact() {
        let limits = RateLimits::new(tiny);
        let now = DateTime::UNIX_EPOCH;
        let ip = |text: &str| Some(text.parse::<IpAddr>().unwrap());
        assert!(limits.take_client(Route::DiscordStart, ip("2001:db8:1:2::1"), now));
        assert!(limits.take_client(Route::DiscordStart, ip("2001:db8:1:2:ffff::9"), now));
        assert!(
            !limits.take_client(Route::DiscordStart, ip("2001:db8:1:2:abcd::"), now),
            "one /64, one bucket"
        );
        assert!(
            limits.take_client(Route::DiscordStart, ip("2001:db8:1:3::1"), now),
            "the next /64 is another client"
        );
        assert!(limits.take_client(Route::DiscordStart, ip("192.0.2.1"), now));
        assert!(limits.take_client(Route::DiscordStart, ip("192.0.2.1"), now));
        assert!(limits.take_client(Route::DiscordStart, ip("192.0.2.2"), now));
        assert!(
            !limits.take_client(Route::DiscordStart, ip("::ffff:192.0.2.1"), now),
            "a mapped IPv4 address is that address"
        );
        assert_eq!(
            client_key("2001:db8:1:2:3:4:5:6".parse().unwrap()),
            "2001:db8:1:2::".parse::<IpAddr>().unwrap()
        );
    }
}
