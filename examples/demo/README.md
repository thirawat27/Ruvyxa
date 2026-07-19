# Ruvyxa Demo

This integration fixture demonstrates Ruvyxa routing, rendering, content, API, actions, environment
variables, CSS, and framework-native plugins in one application. Use it to test a feature before
adopting the pattern in a production application, and use it to verify development/production parity
while contributing to the framework.

## Run the demo

From the monorepo root:

```bash
pnpm --dir examples/demo dev
```

Then open `http://localhost:3000`. For a standalone published-package application, see the
[User Guide](../../docs/guides/index.md), including the CI/Vercel-safe build scripts.

## Routes to explore

| URL                     | Source                            | Demonstrates                                               |
| ----------------------- | --------------------------------- | ---------------------------------------------------------- |
| `/`                     | `app/page.tsx`                    | Index and feature map                                      |
| `/about`                | `app/about/page.tsx`              | Static nested route                                        |
| `/blog/hello-world`     | `app/blog/[slug]/page.tsx`        | Dynamic route parameters                                   |
| `/catchall/one/two`     | `app/catchall/[...slug]/page.tsx` | Catch-all parameters                                       |
| `/content`              | `app/content/page.mdx`            | Markdown, MDX, and frontmatter                             |
| `/todos`                | `app/todos/action.ts`             | Validated server action                                    |
| `/api/health`           | `app/api/health/route.ts`         | Basic API route                                            |
| `/api/echo`             | `app/api/echo/route.ts`           | JSON POST API route                                        |
| `/env`                  | `app/env/page.tsx`                | `RUVYXA_PUBLIC_*` variables                                |
| `/static-page`          | `app/static-page/page.tsx`        | Static-generation candidate                                |
| `/ssg-blog/hello-world` | `app/ssg-blog/[slug]/page.tsx`    | SSG with `getStaticParams`                                 |
| `/isr-page`             | `app/isr-page/page.tsx`           | ISR with `revalidate`                                      |
| `/csr-page`             | `app/csr-page/page.tsx`           | Client-only rendering                                      |
| `/ppr-page`             | `app/ppr-page/page.tsx`           | PPR and `Suspense`                                         |
| `/plugin-lab`           | `app/plugin-lab/page.tsx`         | Plugin middleware, resolution, transforms, and build hooks |

## Demo plugins

The demo config loads `plugins/index.ts`, which contains intentionally different plugin scenarios:

- `page-observability.ts` uses the simple `plugin(name, middleware)` helper to add request and
  response headers on a page route.
- `render-mode-badges.ts` applies the same middleware to static, SSG, ISR, CSR, and PPR pages so
  each rendering strategy can be inspected independently.
- `build-pipeline.ts` uses advanced hooks to resolve `~demo-plugin`, transform a client module, and
  inspect the generated route manifest after a build.

These plugins are page/build integrations only; they do not depend on the demo API routes. Copy a
plugin file into a project `plugins/` directory, export it from `plugins/index.ts`, and add the
array to `ruvyxa.config.ts` to try the same patterns.

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
