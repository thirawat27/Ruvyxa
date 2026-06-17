# File Routing

Ruvyxa discovers routes under `app/`.

| File | Route |
| --- | --- |
| `app/page.tsx` | `/` |
| `app/about/page.tsx` | `/about` |
| `app/blog/[slug]/page.tsx` | `/blog/:slug` |
| `app/docs/[...slug]/page.tsx` | `/docs/*slug` |
| `app/(marketing)/pricing/page.tsx` | `/pricing` |
| `app/api/health/route.ts` | `/api/health` |

Run:

```bash
cargo run -p ruvyxa_cli -- routes --root examples/basic-app
```

The manifest is written to `.ruvyxa/manifest.json` during build. Production source files are emitted under `.ruvyxa/server/app`.

Inspect a live route match while the server is running:

```bash
curl "http://localhost:3000/__ruvyxa/trace?path=/blog/hello"
```
