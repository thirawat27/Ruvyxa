# Dev/Production Parity

Ruvyxa guarantees that `ruvyxa dev` and `ruvyxa start` share the same route semantics. What works in
development works in production — no surprises at deploy time.

---

## The Problem

Many frameworks have subtle differences between dev and production modes: different routing
algorithms, different module resolution, different layout behavior. Ruvyxa eliminates this class of
bug by:

1. Using the same route discovery algorithm in both modes.
2. Using the same SSR rendering pipeline via the persistent Node worker pool.
3. Using the same security headers and action protections.
4. Providing an automated parity check to verify consistency.

---

## Running the Parity Check

```bash
ruvyxa test:parity
```

This command (also aliased as `ruvyxa parity`):

1. Discovers routes from `app/` (the dev source).
2. Builds production output to `.ruvyxa/`.
3. Discovers routes from `.ruvyxa/server/app` (the production source).
4. Compares every route between dev and production.
5. Smoke-renders every page route in both modes.

---

## What's Compared

| Property       | Must match                        |
| -------------- | --------------------------------- |
| Route kind     | `page` or `api`                   |
| Route path     | The resolved URL pattern          |
| Layout chain   | All layouts that wrap the route   |
| Server modules | `server.ts`, `action.ts` siblings |
| Client modules | `client.tsx` siblings             |
| Runtime target | `node`, `edge`, or `static`       |

For page routes, it also renders a representative URL in both modes and fails if either returns an
error.

---

## Example Output

```
PASS  Page  /           dev/prod match
PASS  Page  /about      dev/prod match
PASS  Page  /blog/:slug dev/prod match
PASS  Api   /api/health dev/prod match
PASS  Render /          dev/prod smoke render
PASS  Render /blog/test dev/prod smoke render
Parity passed for 4 routes
```

If a mismatch is found:

```
FAIL  Page  /blog/:slug
  dev layout chain:  [app/layout, app/blog/layout]
  prod layout chain: [app/layout]
  Missing layout in production output.
```

---

## When to Run

Run the parity check after changing:

- Route discovery logic
- Build output structure
- Layout nesting rules
- Server module detection
- Client module detection

The CI and release workflows run `ruvyxa check` (which includes parity) as part of the quality gate.

---

## How It Works Internally

Both `ServerConfig::dev` and `ServerConfig::production` pass through the same `discover_routes()`
function, the same `RadixRouter`, and the same `render_request()` pipeline via the persistent Node
worker pool. The parity test verifies that route discovery output is identical for both source
directories (`app/` vs `.ruvyxa/server/app`) and then smoke-renders every page route in both modes.

---

## Related

- [Debugging](debugging.md) — diagnostics and tracing tools
- [Production Readiness](production-readiness.md) — full release checklist
- [Deployment](deployment.md) — build and serve
