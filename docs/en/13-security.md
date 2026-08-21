# Security

> **Tutorial goal:** turn framework safeguards into an application security routine. **Start from:**
> your configuration in [Configuration](07-configuration.md). **Checkpoint:** review the application
> checklist and prove one protected boundary in your app.

Security is layered: framework validation and defaults reduce risk, but application authorization,
secret storage, upstream network controls, and infrastructure policy remain your responsibility.

## Framework-enforced controls

- Boundary validation rejects private environment access and server-only imports in client code;
  browser-safe values must be prefixed `RUVYXA_PUBLIC_`.
- Action and API body limits are configurable in `security`. Action rate limiting is configurable
  with a maximum/window; action input schemas run before action handlers.
- `security.trustedProxyIps` is an allow-list for forwarded IP/protocol headers; loopback proxies
  are trusted by default. Do not trust forwarded headers from arbitrary clients.
- The first-party `redirects` plugin validates destinations against unsafe scheme-relative,
  backslash, and invalid-origin forms. `securityHeaders` validates CSP directive maps and defaults
  HSTS.
- Auth code defines a signed/session/provider runtime and rate-limit store contracts, but durable
  storage and deployment-specific cookie/origin decisions are application work.

## Application checklist

- Validate every API body, route parameter, and action input. A size limit is not semantic
  validation.
- Authorize every data read/write in the handler; `ActionContext.user` is optional and does not
  itself authenticate a caller.
- Store `RUVYXA_AUTH_SECRET`, OAuth secrets, and database credentials outside source control. Never
  use `RUVYXA_PUBLIC_` for them.
- Define CORS origins/methods/headers explicitly when using `middleware.builtin.cors`; do not enable
  credentialed cross-origin access without a reviewed origin list. None of the three defaults to a
  value — an unset `methods` or `headers` sends no `Access-Control-Allow-Methods` or
  `Access-Control-Allow-Headers`, so a cross-origin request using anything beyond a simple method is
  blocked until you name it. Credentials alongside `origins: ['*']` are refused outright.
- Use route-scoped CSP, frame, referrer, COOP/COEP/CORP, and permissions policies via
  `securityHeaders` after verifying required assets.
- Keep structured logs free of tokens, cookies, authorization headers, request bodies, and personal
  data. The observability plugin logs method/path/status/timing, not a general redaction solution.

## Infrastructure checklist

Terminate TLS, restrict inbound network access, set process memory/time limits, patch
Node/Rust/dependencies, and provide a secret manager. Place only known proxy addresses/CIDRs in
`trustedProxyIps`. Test authentication redirects with production origins. No codebase evidence
establishes built-in CSRF middleware, generic rate limiting for arbitrary API routes, malware
scanning, WAF, or automatic dependency-vulnerability remediation; add those controls where your
threat model needs them.

**Previous:** [Development and testing](12-development-testing.md) · **Next:**
[Observability and performance](14-observability-performance.md)
