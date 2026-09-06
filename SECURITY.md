# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 1.x     | Yes       |

## Reporting a Vulnerability

Report vulnerabilities privately through GitHub security advisories for:

https://github.com/thirawat27/Ruvyxa

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
- Action body size limits (1 MB default, hard ceiling 16 MB)
- API route body size limits (10 MB default, hard ceiling 256 MB via `security.apiLimit`)
- Configurable per-client/action rate limiting (600 req/min default via `security.actionRateLimit`)
- Content-Type enforcement (JSON or form-encoded only)
- Strict malformed JSON/UTF-8 rejection (no type-confusion fallback)
- Markdown raw HTML rendered as escaped text instead of executable HTML
- Default security headers on all responses (`X-Content-Type-Options`, `Referrer-Policy`,
  `Permissions-Policy`, `Cross-Origin-Opener-Policy`, `Cross-Origin-Resource-Policy`,
  `X-Frame-Options`, `X-Permitted-Cross-Domain-Policies`, WebSocket upgrade preservation)
- Optional CORS middleware with origin allowlist
- `ruvyxa.config.ts` — including `proxy.handler` and the Markdown pipeline — runs as trusted
  application code with the selected JavaScript runtime's process, filesystem, environment, and
  network access; Ruvyxa does not sandbox it
- Deterministic BLAKE3-256 client asset hashes (immutable caching with ETag/304 support)
- Bounded `X-Request-ID` correlation values in request logs and responses for incident tracing
- Ruvyxa CLI packages for supported OS/CPU targets (no Rust toolchain required)
- Scheduled and change-triggered RustSec plus pnpm production dependency advisory scans in CI

Apps should still add deployment-layer controls such as TLS termination, CDN/WAF rules, secret
rotation, CSP headers, and database access policies.

`proxy.handler` runs on every matching request with the process's full access; treat the code it
imports as part of the application, not as an isolated extension.
