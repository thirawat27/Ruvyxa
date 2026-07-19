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
//! ## Diagnostic Codes
//!
//! - `RUV2000`: Middleware configuration error
//! - `RUV2001`: Middleware execution failed
//! - `RUV1700`: TypeScript plugin execution failed
//! - `RUV1701`: TypeScript plugin protocol error

pub mod builtin;
pub mod config;
pub mod plugin_host;
pub mod stack;

pub use config::MiddlewareConfig;
pub use plugin_host::{
    MiddlewareRequestResult, PluginHost, PluginHttpRequest, PluginHttpResponse,
    PluginRegistryDescriptor,
};
pub use stack::MiddlewareStack;
