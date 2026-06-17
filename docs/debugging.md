# Debugging

Ruvyxa diagnostics are structured around five questions:

1. What happened?
2. Where did it happen?
3. Why did it happen?
4. How do you fix it?
5. Which routes or modules are affected?

The MVP includes diagnostics for missing app directories, invalid route segments, duplicate route paths, pages without default exports, SSR renderer failures, API route execution failures, server action failures, Tailwind CSS compilation failures, and client bundle boundary violations.

## Client Boundary Errors

Browser bundles cannot import server-only modules or private environment variables.

```ts
import "server-only"
```

This marker is valid for server files, but it fails with `RUV1007` if the file is reachable from a hydrated page.

Private env access also fails in the client bundle:

```ts
process.env.DATABASE_URL
```

Use `RUVYXA_PUBLIC_*` only for values that are safe to expose to browsers.

Server-side renderers load `.env` and `.env.local`. `doctor` reports whether `.env.example` exists so required keys are documented for other developers.

## Tailwind Errors

CSS files that import Tailwind require `@tailwindcss/cli` in the app's `node_modules`.

```css
@import "tailwindcss";
```

If the CLI is missing, Ruvyxa reports `RUV1401`. If Tailwind cannot compile the CSS, Ruvyxa reports `RUV1400` with the Tailwind stderr output.

## Runtime Trace

Inspect the same route matching data used by dev and production:

```bash
curl "http://localhost:3000/__ruvyxa/trace?path=/blog/hello"
```

The response includes the matched route, params, layout chain, server modules, client modules, runtime mode, and asset directories. Use it when a route renders differently than expected.

## Parity Check

Compare development and production route graphs:

```bash
ruvyxa test:parity
```

Use this after changing route discovery, build output, layouts, server modules, or actions.

## Benchmark

Measure framework hot paths locally:

```bash
ruvyxa bench --root examples/basic-app --samples 3
ruvyxa bench --root examples/basic-app --samples 3 --json
```

The benchmark suite measures route discovery, analyze validation, and production build timings. Use it before and after optimizer, route graph, HMR, or build changes.

## Security Headers

Runtime responses include conservative defaults:

- `x-content-type-options: nosniff`
- `referrer-policy: strict-origin-when-cross-origin`
- `permissions-policy: camera=(), microphone=(), geolocation=()`
- `cross-origin-opener-policy: same-origin`

Use this as a baseline. Apps that need a custom CSP can add it at the deployment layer until Ruvyxa exposes a typed security config.

## Doctor

Run `doctor` when setup or dependency state looks wrong:

```bash
ruvyxa doctor
```

It checks app files, package manager, Node/Bun/Deno availability, React and ReactDOM versions, duplicate dependency versions, route diagnostics, env schema presence, and native binary status.
