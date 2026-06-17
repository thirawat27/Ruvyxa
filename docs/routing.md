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

The manifest is written to `.ruvyxa/manifest.json` during build.
