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
- `middleware.builtin.rate` rate-limits every route, not only actions. Its default key, `ip`, is the
  transport peer unless that peer is loopback or listed in `security.trustedProxyIps`, in which case
  the forwarded chain is scanned from the right for the first address that is not one of your
  proxies — a client that is not a proxy cannot rename itself. Behind a reverse proxy, `ip` is the
  mode you want: name your proxies in `trustedProxyIps` and leave the key alone. `header:<name>` is
  the escape hatch for an application-defined identity such as an API key and is used verbatim, so
  pointing it at `x-forwarded-for` hands the bucket key to the caller and one client can rotate it
  for an unlimited allowance; the server warns at startup when it sees that.
- A deployed build answers the same question without a transport peer to weigh, so the adapter that
  emitted it declares which header its own platform ingress writes and overwrites:
  `CF-Connecting-IP` on Cloudflare Workers and `X-Vercel-Forwarded-For` on Vercel. Every other
  target — the standalone server the node, bun, deno, aws, railway, and render adapters emit, plus
  the Netlify and Firebase functions — declares none and scans `X-Forwarded-For` by the rule above,
  because nothing in front of them guarantees such a header was written by a proxy rather than typed
  by the caller. Behind nginx, Traefik, Cloudflare, or any other proxy, name it in
  `security.trustedProxyIps`: without that the rightmost hop is the proxy itself and every client
  shares one bucket.
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
`trustedProxyIps`. Test authentication redirects with production origins. Cross-site protection for
route handlers is the `originGuard` plugin and is opt-in per route scope; general rate limiting is
`middleware.builtin.rate` and is off until configured. No codebase evidence establishes malware
scanning, WAF, or automatic dependency-vulnerability remediation; add those controls where your
threat model needs them.

**Previous:** [Development and testing](12-development-testing.md) · **Next:**
[Observability and performance](14-observability-performance.md)
