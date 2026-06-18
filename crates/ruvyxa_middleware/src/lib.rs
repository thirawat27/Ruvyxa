//! # Ruvyxa Middleware
//!
//! A composable middleware system built on Tower's `Service` and `Layer` traits,
//! with an optional WebAssembly plugin runtime powered by Wasmtime.
//!
//! ## Architecture
//!
//! - **Built-in middleware**: CORS, rate-limiting, request logging, response timing,
//!   custom headers — all configurable via `ruvyxa.config.ts`.
//! - **Tower Layer stack**: Middleware is applied as standard Tower layers, composable
//!   with any axum/tower ecosystem middleware.
//! - **Wasm Plugin Runtime** (feature `wasm-plugins`): Load `.wasm` modules as
//!   sandboxed plugins that can intercept requests/responses. Hot-reloadable via
//!   file watcher. Provides maximum security isolation (no filesystem, no network
//!   unless explicitly granted).
//!
//! ## Diagnostic Codes
//!
//! - `RUV2000`: Middleware configuration error
//! - `RUV2001`: Middleware execution failed
//! - `RUV2100`: Wasm plugin load error
//! - `RUV2101`: Wasm plugin execution error
//! - `RUV2102`: Wasm plugin hot-reload error

pub mod builtin;
pub mod config;
pub mod stack;

#[cfg(feature = "wasm-plugins")]
pub mod wasm;

pub use config::MiddlewareConfig;
pub use stack::MiddlewareStack;

#[cfg(feature = "wasm-plugins")]
pub use wasm::WasmPluginRuntime;
