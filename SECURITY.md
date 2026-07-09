# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| 1.x | Yes |

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
- Action body size limits (64 KB default, configurable)
- Content-Type enforcement (JSON or form-encoded only)
- In-process action rate limiting (60 req/min per client)
- Default security headers on all responses (`X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy`, `Cross-Origin-Opener-Policy`)
- Optional CORS middleware with origin allowlist
- Wasm plugin sandboxing (fuel-based execution limits, memory bounds, no implicit FS/net/env access)
- Deterministic BLAKE3 client asset hashes (immutable caching with ETag/304 support)
- Native CLI packages for supported OS/CPU targets (no Rust toolchain required)

Apps should still add deployment-layer controls such as TLS termination, CDN/WAF rules, secret rotation, CSP headers, and database access policies.
