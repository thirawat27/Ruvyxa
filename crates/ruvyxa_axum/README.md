# ruvyxa-axum

Serve a [Ruvyxa](https://github.com/thirawat27/Ruvyxa) static build from your own
[axum](https://crates.io/crates/axum) application, as one binary.

`ruvyxa build` with `@ruvyxa/adapter-static` writes a publish directory — one pre-rendered
document per route, the hashed client bundles, everything from `public/`, a `404.html`, and a
`_headers` file carrying the cache and security policy. This crate serves that directory from an
`axum::Router`, so a Rust API and the React front end it serves ship as a single executable with
nothing else installed on the host: no Node at runtime, no second process, no reverse proxy.

```rust
use axum::{Router, routing::get};
use ruvyxa_axum::StaticSite;

let site = StaticSite::open(".ruvyxa/static")?;
let app = Router::new()
    .route("/api/health", get(|| async { "ok" }))
    .layer(site.headers_layer())      // the `_headers` policy on your routes too
    .fallback_service(site.router()); // pages, assets, and the site's own 404
```

In a Ruvyxa project, `ruvyxa adds axum` writes a `Cargo.toml` and this `src/main.rs`.

## What it serves

Everything the static target holds: SSG and CSR routes, assets, `public/`. The build refuses a
server-rendered, ISR, PPR, or API route for this target with a per-route error, so nothing this
crate opens needs a renderer. Projects that need those routes run `ruvyxa start` or a deploy
adapter.

`_headers` is matched as Netlify and Cloudflare Pages match it, a pre-rendered route is served at
its own URL without a redirect, a miss is the project's `404.html` with a 404 status, and content
types come from the same table every other Ruvyxa host uses. `GET` and `HEAD` only; conditional
and range requests are honoured; files stream. `_headers` and `_redirects` themselves are not
served.

The security headers in `_headers` are set on every response, errors included. Caching headers
(`Cache-Control`, `Expires`, the `CDN-Cache-Control` family) are set only on a 2xx or 304
response — a 404 for a bundle that has not shipped must not be cached as `immutable` — and never
replace caching headers your own route set, so a `Cache-Control: private` answer stays private.

## License

Apache-2.0, as the rest of the framework.
