//! # Ruvyxa Middleware
//!
//! A composable middleware system built on Tower's `Service` and `Layer` traits,
//! plus the bridge to the project worker that runs `proxy.handler` from `ruvyxa.config.ts`.
//!
//! ## Architecture
//!
//! - **Built-in middleware**: CORS, rate-limiting, request logging, response timing,
//!   custom headers — all configurable via `ruvyxa.config.ts`.
//! - **Tower Layer stack**: Middleware is applied as standard Tower layers, composable
//!   with any axum/tower ecosystem middleware.
//! - **Project worker**: `proxy.handler`, the Markdown pipeline, and the content engine are
//!   code in `ruvyxa.config.ts`; the native server runs them in a persistent JavaScript
//!   worker and validates what comes back.
//!
//! ## Rate limiting: four limiters, one contract
//!
//! Ruvyxa refuses excess traffic in five places, and they do not share an
//! algorithm. That is deliberate — each guards a different shape of traffic and
//! pays a different price for memory — but it was never written down, so
//! `rateLimit.max = 10` meant something slightly different depending on which
//! path a request took. What every one of them *does* promise:
//!
//! - **Bounded memory.** None grows with the number of distinct clients seen,
//!   and none grows with the *size* of the identity a client sends: a tracked
//!   key is fixed-width whatever the caller wrote. That second half used to be
//!   untrue — the key was a header value bounded only by the server's header
//!   size limit, so the promise held for the count and not for the bytes.
//! - **Fail closed.** A limiter that cannot answer refuses rather than admits.
//!   Running out of *slots* is not that. A full map is out of room, not out of
//!   answers, and a slot can be taken back, so [`builtin::RateLimitLayer`]
//!   evicts its least recently refilled bucket and admits the new client;
//!   the evicted one is re-admitted with a full allowance the moment it
//!   returns. Refusing there was an outage with a lock on it: one client
//!   sending a distinct key per request filled the map with buckets no sweep
//!   could free, and every visitor the map had not already seen got a 429
//!   until the window rolled. The action replay guard is the limiter that
//!   genuinely does fail closed at saturation, because evicting a live nonce
//!   would accept the replay it exists to refuse.
//! - **One identity per bucket.** Who the client is comes from
//!   [`client_ip()`], which believes a forwarded header only from a peer that is
//!   loopback or listed in `security.trustedProxyIps`.
//!
//! Where they differ, and why:
//!
//! | Limiter | Guards | Algorithm | Memory bound |
//! | --- | --- | --- | --- |
//! | [`builtin::RateLimitLayer`] | every HTTP request | fixed window | `MAX_TRACKED_RATE_LIMIT_KEYS` hashed fixed-width keys; swept under pressure, then the least recently refilled bucket is evicted |
//! | `ruvyxa_dev_server::action_security::ActionRateLimiter` | server actions | sliding-window counter (previous window weighted by overlap) | a fixed 8192-slot array; hash collisions merge two clients, which can only refuse more |
//! | `ruvyxa_dev_server::collab::FrameRateLimiter` | collab cursor frames | fixed one-second window | one counter per connection |
//! | `packages/ruvyxa/runtime/serverless-handler.mjs` | every request in a deployed build | fixed window, mirrors `RateLimitLayer` | same cap, same bound, same eviction |
//!
//!
//! The serverless row is a claim about another language, and it was false for as
//! long as nothing checked it: the capacity refusal and the unbounded key were
//! fixed here first and every deployed build kept both. All three fixed-window
//! copies replay `tests/fixtures/rate-limit-conformance.json` — how a key is bounded
//! stays host-local, because blake3 is not reachable from a module copied into
//! an edge bundle, but the bound, the identity each key is derived from, and
//! what capacity pressure costs are one answer.
//!
//! The consequence worth stating rather than discovering: a **fixed** window
//! lets a client spend its whole allowance at the end of one window and again
//! at the start of the next, so a burst of up to `2 * max` is reachable across
//! a boundary. The limit those enforce is a sustained rate, not an
//! instantaneous one. The action limiter is the one that does not have that
//! edge, because actions are the path where a burst is worth paying for.
//!
//! `@ruvyxa/auth` is outside this table on purpose: its store is supplied by
//! the application, because a login attempt must be counted across every
//! process serving the deployment, not per instance.
//!
//! ## Diagnostic Codes
//!
//! - `RUV2000`: Middleware configuration error
//! - `RUV2001`: Middleware execution failed
//! - `RUV1700`: project worker call failed
//! - `RUV1701`: project worker protocol error

pub mod builtin;
pub mod client_ip;
pub mod config;
pub mod route_rules;
pub mod stack;
pub mod worker_host;

pub use client_ip::{
    IpPrefix, TrustedProxies, client_ip, forwarded_client_ip, is_trusted_proxy_ip, unmap_v4,
};
pub use config::MiddlewareConfig;
pub use route_rules::{
    CompiledProxy, HeaderRule, MatcherEntry, ProxyConfig, RedirectRule, RewritePhases, RewriteRule,
    RouteCondition, RouteRules, RuleRequest,
};
pub use stack::MiddlewareStack;
pub use worker_host::{
    ContentArtifact, WireRequest, WireRequestResult, WireResponse, WorkerDescriptor, WorkerHost,
};
