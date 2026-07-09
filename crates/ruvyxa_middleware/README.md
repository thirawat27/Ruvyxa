# ruvyxa_middleware

Tower-based middleware system and Wasmtime WASI plugin runtime for the Ruvyxa framework.

## Overview

This crate provides:

- **Built-in middleware** — composable Tower layers for CORS, rate-limiting, request timing, logging, and custom response headers.
- **Middleware stack builder** — compiles `MiddlewareConfig` into axum-compatible layer stacks.
- **Wasm plugin runtime** — sandboxed WebAssembly plugin execution via Wasmtime 45 with hot-reload support.

## Built-in Middleware

| Middleware | Description |
|---|---|
| `TimingLayer` | Adds `X-Response-Time` header to all responses |
| `RequestLoggingLayer` | Structured logging: method, path, status, duration |
| `CorsLayer` | Configurable CORS with preflight handling |
| `CustomHeadersLayer` | Arbitrary response headers from config |
| `CompressionLayer` | Gzip + Brotli (via tower-http) |

All layers implement `tower::Layer` and can be composed with any axum/tower middleware.

## Wasm Plugin Security Model

Each plugin runs in an isolated Wasmtime `Store`:

- **Fuel-based execution limits** — prevents infinite loops
- **Memory bounds** — configurable max memory (default 64MB)
- **No filesystem access** unless explicitly granted via `permissions.fsRead`
- **No network access** unless explicitly granted via `permissions.net`
- **No environment access** unless explicitly granted via `permissions.env`
- **Hot-reload** — file watcher detects `.wasm` changes and reloads without server restart

## Plugin Phases

- `request` — intercepts before the route handler; can short-circuit with a direct response
- `response` — intercepts after the route handler; can modify the response

## Diagnostic Codes

| Code | Meaning |
|------|---------|
| `RUV2000` | Middleware configuration error |
| `RUV2001` | Middleware execution failed |
| `RUV2100` | Wasm plugin load error |
| `RUV2101` | Wasm plugin execution error |
| `RUV2102` | Wasm plugin hot-reload error |

## Feature Flags

- `wasm-plugins` (default) — enables `wasmtime` and `wasmtime-wasi` dependencies for the Wasm plugin runtime. Disable to reduce binary size if you only need built-in middleware.
- `debug-plugin` (disabled) — enables verbose logging for Wasm plugin execution and hot-reload events.

## License

Apache 2.0
