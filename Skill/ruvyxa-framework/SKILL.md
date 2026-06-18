---
name: ruvyxa-framework
description: Provider-neutral operating guide for any AI coding agent that needs to create, modify, debug, document, or verify user applications built with the Ruvyxa framework. Use for Ruvyxa app routes, layouts, pages, API routes, server actions, config, middleware settings, deployment configuration, app diagnostics, dev/build/start/parity workflows, and generated starter apps. This skill is only for the application/user side of Ruvyxa, not for maintaining or changing Ruvyxa framework source code.
---

# Ruvyxa Framework

## Provider-Neutral Contract

This skill is plain Markdown and does not depend on any AI provider, IDE, MCP server, plugin system, or proprietary agent feature. Apply it with any coding assistant, local automation runner, or manual engineering workflow that can read Markdown and operate on a Ruvyxa application project.

- Use the local filesystem and shell tools available in the current environment.
- Prefer `rg`/`rg --files` for search when available; otherwise use the closest equivalent.
- If an agent supports plans, use a short checklist. If it does not, proceed sequentially and report the same milestones.
- If an agent supports subagents or parallel sessions, parallelize only independent read/check work. Do not split edits across the same files.
- If an agent lacks tool execution, provide exact commands and file edits instead of inventing unverifiable results.
- Keep outputs, docs, and verification commands portable across Windows, macOS, and Linux where practical.

## Systematic Workflow

1. Detect the app root and read `package.json`, `ruvyxa.config.ts`, and the nearest `AGENTS.md` if present.
2. Inspect the current worktree before editing. Do not overwrite unrelated user changes.
3. Identify the route, API handler, server action, config block, component, or asset that owns the requested behavior.
4. Implement with the existing app patterns and the public Ruvyxa APIs.
5. Run the relevant verification commands and report exact results.
6. Keep generated build output out of source edits unless the user explicitly asks for artifacts.

## Non-Negotiables

- Keep Node.js 22 as the JavaScript runtime requirement. Do not lower package `engines.node`, docs, CI, or template requirements.
- Use Ruvyxa's public APIs and CLI. Do not modify Ruvyxa framework source code, package internals, native implementation files, renderer scripts, or adapter package internals.
- If the task requires changing Ruvyxa framework source code, stop and ask for a framework-development guide instead of continuing with this app-user skill.
- Preserve dev/prod parity from the app side: verify with `pnpm check` before handoff, and use `pnpm parity` when route/runtime behavior needs focused debugging.
- Keep server-only imports out of browser-reachable code.
- Keep private environment variables out of client graphs; expose only `RUVYXA_PUBLIC_*` values to browser code.
- Keep `analyze` output machine-readable. Do not work around validation by hiding unsafe imports.

## App Work

- Use `app/**/page.tsx` for page routes and `app/api/**/route.ts` for API routes.
- Keep browser-reachable code free of private env reads and server-only imports.
- Put route-local server logic in `server.ts` or `action.ts`; put browser modules in `client.tsx`.
- Prefix browser-safe env vars with `RUVYXA_PUBLIC_`.
- Run `pnpm check` after changing routes, imports, env usage, or server/client boundaries.

## Common App Tasks

- Add pages by creating `app/<route>/page.tsx` with a default export.
- Add API endpoints by creating `app/api/<name>/route.ts`.
- Add server mutations with `action()` from `ruvyxa/server`.
- Add server data loading with `loader()` from `ruvyxa/server`.
- Configure app paths, server defaults, build output, cache, middleware, and debug settings in `ruvyxa.config.ts`.
- Use `public/` for static assets.
- Configure deployment through public adapter packages and app config.

## Verification

For normal app changes, run:

```bash
pnpm check
```

For route, runtime, middleware, or deployment-sensitive failures that need drill-down, also run:

```bash
pnpm parity
pnpm analyze
pnpm doctor
```

For local smoke testing, use:

```bash
pnpm dev
pnpm start
```

## References

Read `references/app-guide.md` when you need deeper guidance for Ruvyxa app structure, routing, config, server/client boundaries, middleware, adapters, and verification choices.
