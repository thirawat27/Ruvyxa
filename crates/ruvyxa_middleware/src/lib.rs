//! # Ruvyxa Middleware
//!
//! A composable middleware system built on Tower's `Service` and `Layer` traits,
//! with TypeScript plugin middleware executed by Ruvyxa's selected JavaScript runtime.
//!
//! ## Architecture
//!
//! - **Built-in middleware**: CORS, rate-limiting, request logging, response timing,
//!   custom headers — all configurable via `ruvyxa.config.ts`.
//! - **Tower Layer stack**: Middleware is applied as standard Tower layers, composable
//!   with any axum/tower ecosystem middleware.
//! - **TypeScript plugin host**: The native server validates and applies request/response
//!   results from the unified Node/Bun plugin registry.
//!
//! ## Rate limiting: four limiters, one contract
//!
//! Ruvyxa refuses excess traffic in four places, and they do not share an
//! algorithm. That is deliberate — each guards a different shape of traffic and
//! pays a different price for memory — but it was never written down, so
//! `rateLimit.max = 10` meant something slightly different depending on which
//! path a request took. What every one of them *does* promise:
//!
//! - **Bounded memory.** None grows with the number of distinct clients seen.
//! - **Fail closed.** A limiter that cannot answer refuses rather than admits.
//! - **One identity per bucket.** Who the client is comes from
//!   [`client_ip()`], which believes a forwarded header only from a peer that is
//!   loopback or listed in `security.trustedProxyIps`.
//!
//! Where they differ, and why:
//!
//! | Limiter | Guards | Algorithm | Memory bound |
//! | --- | --- | --- | --- |
//! | [`builtin::RateLimitLayer`] | every HTTP request | fixed window | `MAX_TRACKED_RATE_LIMIT_KEYS` exact keys, swept only under pressure |
//! | `ruvyxa_dev_server::action_security::ActionRateLimiter` | server actions | sliding-window counter (previous window weighted by overlap) | a fixed 8192-slot array; hash collisions merge two clients, which can only refuse more |
//! | `ruvyxa_dev_server::collab::FrameRateLimiter` | collab cursor frames | fixed one-second window | one counter per connection |
//! | `packages/ruvyxa/runtime/serverless-handler.mjs` | every request in a deployed build | fixed window, mirrors `RateLimitLayer` | same key cap |
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
//! - `RUV1700`: TypeScript plugin execution failed
//! - `RUV1701`: TypeScript plugin protocol error

pub mod builtin;
pub mod client_ip;
pub mod config;
pub mod plugin_host;
pub mod stack;

pub use client_ip::{
    IpPrefix, TrustedProxies, client_ip, forwarded_client_ip, is_trusted_proxy_ip, unmap_v4,
};
pub use config::MiddlewareConfig;
pub use plugin_host::{
    NativeCapabilityDescriptor, PluginBuildDescriptor, PluginDevDescriptor,
    PluginDiagnosticDescriptor, PluginEnvironment, PluginHost, PluginHttpDescriptor,
    PluginHttpRequest, PluginHttpRequestResult, PluginHttpResponse, PluginRegistryDescriptor,
    RealtimeDescriptor,
};
pub use stack::MiddlewareStack;
