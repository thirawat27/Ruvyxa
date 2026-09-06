# Ruvyxa Demo

This integration fixture demonstrates Ruvyxa routing, rendering, content, API, actions, environment
variables, CSS, and the config-declared request pipeline in one application. Use it to test a
feature before adopting the pattern in a production application, and use it to verify
development/production parity while contributing to the framework.

## Run the demo

From the monorepo root:

```bash
pnpm --dir examples/demo dev
```

Then open `http://localhost:3000`. For a standalone published-package application, see the
[User Guide](../../docs/README.md), including the CI/Vercel-safe build scripts.

## Routes to explore

| URL                     | Source                            | Demonstrates                                          |
| ----------------------- | --------------------------------- | ----------------------------------------------------- |
| `/`                     | `app/page.tsx`                    | Index and feature map                                 |
| `/about`                | `app/about/page.tsx`              | Static nested route                                   |
| `/blog/hello-world`     | `app/blog/[slug]/page.tsx`        | Dynamic route parameters                              |
| `/catchall/one/two`     | `app/catchall/[...slug]/page.tsx` | Catch-all parameters                                  |
| `/content`              | `app/content/page.mdx`            | Markdown, MDX, and frontmatter                        |
| `/todos`                | `app/todos/action.ts`             | Validated server action                               |
| `/api/health`           | `app/api/health/route.ts`         | Basic API route                                       |
| `/api/echo`             | `app/api/echo/route.ts`           | JSON POST API route                                   |
| `/env`                  | `app/env/page.tsx`                | `RUVYXA_PUBLIC_*` variables                           |
| `/static-page`          | `app/static-page/page.tsx`        | Static-generation candidate                           |
| `/ssg-blog/hello-world` | `app/ssg-blog/[slug]/page.tsx`    | SSG with `getStaticParams`                            |
| `/isr-page`             | `app/isr-page/page.tsx`           | ISR with `revalidate`                                 |
| `/csr-page`             | `app/csr-page/page.tsx`           | Client-only rendering                                 |
| `/ppr-page`             | `app/ppr-page/page.tsx`           | PPR and `Suspense`                                    |
| `/proxy-lab`            | `app/proxy-lab/page.tsx`          | `headers()` rules and `proxy.handler` from the config |

## Request pipeline

`ruvyxa.config.ts` declares the whole request pipeline:

- `headers` labels each render strategy's page with `x-demo-render-mode`, and every path under
  `/proxy-lab` with `x-demo-headers-rule`.
- `proxy` runs ahead of routing for `/proxy-lab/:path*`: it forwards the page with an added request
  header and answers `/proxy-lab/blocked` with a 403 before any route is matched.

Both run natively on every host — `ruvyxa dev`/`start` and every deployed build — from the same
declarations.

## Diagnose and verify

```bash
pnpm --dir examples/demo doctor    # tools, packages, routes, validation summary
pnpm --dir examples/demo routes    # route table and detected rendering strategies
pnpm --dir examples/demo analyze   # route/import/server-client diagnostics
pnpm --dir examples/demo typecheck
pnpm --dir examples/demo check     # typecheck + build + parity + page smoke render
pnpm --dir examples/demo parity    # parity only
pnpm --dir examples/demo trace /blog/[slug]
```

Start with `analyze` after adding or moving a route, import, environment variable, or configuration
value. Run `check` before handing off a feature. The health endpoint deliberately returns only
stable service information; use `routes` and `doctor` for the actual route count and framework
version.
