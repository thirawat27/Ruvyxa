# Claude Instructions

Read `AGENTS.md` first. This is the Ruvyxa kitchen-sink example app demonstrating all framework
features.

Available scripts:

- `pnpm dev` — `cargo run -p ruvyxa_cli -- dev --root .`
- `pnpm build` — `cargo run -p ruvyxa_cli -- build --root .`
- `pnpm start` — `cargo run -p ruvyxa_cli -- start --root .`
- `pnpm check` — `cargo run -p ruvyxa_cli -- check --root .`
- `pnpm analyze` — `cargo run -p ruvyxa_cli -- analyze --root .`

The app demonstrates: static pages, dynamic routes (`[slug]`), catch-all (`[...slug]`), route
groups, API routes (`GET`, `POST`), server actions (`action.ts`), server-only modules (`server.ts`),
public env vars, layouts, and global CSS.
