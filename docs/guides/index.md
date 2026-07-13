# Ruvyxa User Guide

Ruvyxa is a React web framework with file-system routing. Its CLI runs the development server,
validates the application graph, builds production output, and checks development/production parity.

---

**[English](en/getting-started.md)** | **[ภาษาไทย](th/getting-started.md)**

---

## Table of Contents

| Chapter                             | EN                                   | TH                                   |
| ----------------------------------- | ------------------------------------ | ------------------------------------ |
| 1. Getting Started                  | [EN](en/getting-started.md)          | [TH](th/getting-started.md)          |
| 2. Routing                          | [EN](en/routing.md)                  | [TH](th/routing.md)                  |
| 3. Server & Client Components       | [EN](en/server-client-components.md) | [TH](th/server-client-components.md) |
| 4. API Routes                       | [EN](en/api-routes.md)               | [TH](th/api-routes.md)               |
| 5. Data Loading & Cache             | [EN](en/data-loading-and-cache.md)   | [TH](th/data-loading-and-cache.md)   |
| 6. Server Actions                   | [EN](en/server-actions.md)           | [TH](th/server-actions.md)           |
| 7. Rendering Strategies             | [EN](en/rendering-strategies.md)     | [TH](th/rendering-strategies.md)     |
| 8. Markdown, MDX, Images & Metadata | [EN](en/markdown-mdx-images.md)      | [TH](th/markdown-mdx-images.md)      |
| 9. Environment Variables            | [EN](en/environment-variables.md)    | [TH](th/environment-variables.md)    |
| 10. Configuration Reference         | [EN](en/configuration.md)            | [TH](th/configuration.md)            |
| 11. CLI Commands                    | [EN](en/cli-commands.md)             | [TH](th/cli-commands.md)             |
| 12. Deployment                      | [EN](en/deployment.md)               | [TH](th/deployment.md)               |

---

## Quick Navigation

### For Application Authors

Start here:

1. [Getting Started](en/getting-started.md) — requirements, create a project, application structure
2. [Routing](en/routing.md) — file-system routes, dynamic segments, catch-all, route groups
3. [CLI Commands](en/cli-commands.md) — `dev`, `build`, `start`, `analyze`, `doctor`, and more

Then explore by topic:

- **Build pages**: [Rendering Strategies](en/rendering-strategies.md) — SSR, SSG, ISR, CSR, PPR
- **Fetch data**: [Data Loading & Cache](en/data-loading-and-cache.md) — loader, cache, SWR
- **Handle mutations**: [Server Actions](en/server-actions.md) — validated mutations, form
  integration
- **Create endpoints**: [API Routes](en/api-routes.md) — HTTP method handlers
- **Style content**: [Markdown, MDX, Images & Metadata](en/markdown-mdx-images.md) — content routes,
  images, SEO
- **Manage secrets**: [Environment Variables](en/environment-variables.md) — public vs private,
  boundary safety
- **Secure & tune**: [Configuration](en/configuration.md) — full `ruvyxa.config.ts` reference
- **Ship**: [Deployment](en/deployment.md) — Vercel, adapters, CI, production checklist

### For Framework Contributors

See the [Developer Guide](../developer-guide.md) for the Rust workspace, npm package layout,
verification commands, and change maps.
