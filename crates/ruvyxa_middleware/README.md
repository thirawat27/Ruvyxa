# ruvyxa_middleware

Ruvyxa's Tower-based middleware stack and plugin bridge.

Built-in middleware remains native Rust and can be configured through
`config.middleware.builtin`. Plugins are application code loaded from
`ruvyxa.config.ts`; their `setup` function registers request/response middleware
and build hooks. The Rust server forwards validated Fetch-style request and
response payloads to the persistent Node/Bun runtime.

The bridge is deliberately small: callbacks stay in JavaScript, while Rust
owns routing, limits, ordering, process lifetime, and conversion to Axum
responses. Response middleware is bounded by `security.pluginLimit`.

Plugin failures are reported as normal Ruvyxa diagnostics. There is no separate
feature flag or custom middleware-layer ABI.
