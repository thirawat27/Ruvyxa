# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 1.x     | Yes       |

## Reporting a Vulnerability

Report vulnerabilities privately through GitHub security advisories for:

https://github.com/thirawat27/ruvyxa

Do not open a public issue for suspected vulnerabilities.

Please include:

- affected package and version
- operating system and Node version
- reproduction steps
- expected impact
- whether a workaround exists

## Security Baseline

Ruvyxa 1.x includes:

- Server/client boundary validation (`server-only`, `client-only`, private env detection)
- Private environment variable blocking in client bundles (`RUV1008`)
- Same-origin and Fetch Metadata (`Sec-Fetch-Site`) checks for server actions
- Action body size limits (1 MB default, configurable)
- API route body size limits (10 MB default, configurable via `security.apiLimit`)
- Response-phase Wasm plugin buffering limited to 32 MB by default, configurable through
  `security.pluginLimit` up to 256 MB, to prevent unbounded server memory use
- Configurable per-client/action rate limiting (600 req/min default via `security.actionRateLimit`)
- Content-Type enforcement (JSON or form-encoded only)
- Default security headers on all responses (`X-Content-Type-Options`, `Referrer-Policy`,
  `Permissions-Policy`, `Cross-Origin-Opener-Policy`, WebSocket upgrade preservation)
- Optional CORS middleware with origin allowlist
- Wasm plugin sandboxing (fuel-based execution limits, memory bounds, no implicit FS/net/env access)
- Deterministic BLAKE3-256 client asset hashes (immutable caching with ETag/304 support)
- Native CLI packages for supported OS/CPU targets (no Rust toolchain required)

Apps should still add deployment-layer controls such as TLS termination, CDN/WAF rules, secret
rotation, CSP headers, and database access policies.
