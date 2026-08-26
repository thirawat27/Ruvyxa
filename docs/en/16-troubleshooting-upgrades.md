# Troubleshooting and upgrade compatibility

> **Tutorial goal:** diagnose a failed command from evidence and upgrade without skipping
> compatibility checks. **Start from:** the command loop in [CLI](10-cli.md). **Checkpoint:**
> reproduce the symptom, apply the matching fix, then rerun the command that failed.

Run the narrowest diagnostic first, from the application root:

```bash
npm run routes
npm run check
npm run analyze
npm run doctor
npm run trace -- /
npm run test:parity
```

## Symptoms and evidence-backed fixes

| Symptom                                        | Likely condition                                                                                 | Check and remedy                                                                                |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| A route is absent                              | File does not follow discovered special-file/dynamic-segment rules.                              | Run `routes`; compare its directory/name with [Project structure](03-project-structure.md).     |
| Client build reports private import/env access | Boundary validation found a server-only import or non-public environment value in a client path. | Move the work server-side; expose only deliberately safe `RUVYXA_PUBLIC_*` values.              |
| Static build fails                             | Static adapter has no generated prerender pages, or the route needs a runtime-only behavior.     | Use a compatible target or supply static params/route strategy; inspect build output.           |
| `RUV2102`                                      | Plugin definition is missing a name/behavior or has invalid hook shape.                          | Ensure `definePlugin` has a non-empty `name` and a valid declaration/register callback.         |
| `RUV3001`–`RUV3003`                            | Database adapter input, mapping, or operation cannot be satisfied.                               | Inspect `DatabaseAdapterError` message and adapter model/table mapping.                         |
| `RUV3201`                                      | Native realtime was built for an unsupported target/adapter.                                     | Deploy long-lived Node/Bun output, or remove realtime.                                          |
| Actions/API reject a body                      | Body exceeds configured action/API limit or input parser throws.                                 | Review `security.actionLimit`/`apiLimit`; validate and return a safe application error.         |
| Cache seems stale                              | The entry is inside TTL/SWR or another process has its own memory cache.                         | Use `invalidateCache`, inspect strategy, and use shared infrastructure for multi-instance data. |
| `RUV1405`                                      | A PostCSS config was found, but a plugin or `postcss` itself could not be loaded.                | Install the packages the config names, or remove them from it.                                  |
| `RUV1014`                                      | `--root` points at a path that does not exist.                                                   | Fix the path, or run the command from inside the project.                                       |
| `RUV1015`                                      | `start`/`preview` was run before `build`, so there is no compiled app to serve.                  | Run `ruvyxa build` first, or use `ruvyxa dev` to serve from source.                             |
| `RUV1016`                                      | `RUVYXA_RUNTIME` holds something other than `node`, `bun`, or `deno`. The message quotes it.     | Fix or unset the variable; `--runtime` overrides it for one command.                            |
| `RUV1406`                                      | A PostCSS plugin threw, or a stylesheet has a syntax error the chain rejected.                   | Fix the reported plugin/stylesheet error; the build will not emit untransformed CSS.            |
| `RUV1805`                                      | An imported `.json` file is not valid JSON.                                                      | The message names the file and the parse position; fix the document.                            |
| `RUV1806`                                      | An import resolved to a file kind Ruvyxa does not compile (`.node`, `.wasm`, a binary asset).    | Add the package to `build.external` so the runtime loads the file instead of the bundler.       |
| `RUV1807`                                      | An import's case does not match the file on disk. This filesystem folds case; Linux does not.    | Spell the import the way the file is named; the message names both spellings.                   |

**Page renders with browser defaults while class names are correct.** The global stylesheet reached
the browser untransformed. Check for `@import "tailwindcss"` in the served CSS: a project using
Tailwind v4 needs a PostCSS config at the project root and `postcss` installed. See
[PostCSS and Tailwind CSS](06-ui-navigation-metadata-and-assets.md#postcss-and-tailwind-css).

**An adapter build fails inside a package you did not write.** An SDK reads a JSON file, a native
addon, or another non-JavaScript asset that the deployment bundle has to carry. JSON is compiled as
data; anything else reports `RUV1806` naming the file and the import that reached it. Serverless
adapters bundle route dependencies into the function, so the failure appears under
`ruvyxa build --adapter <name>` and not under a plain `ruvyxa build`.

## Common questions

**Which not-found helper do I want?** `notFound()` from `@ruvyxa/react` throws a tagged signal and
the nearest route boundary renders `not-found.tsx` — that is the one for a page, and the one that
matches Next.js. `status(404, message?)` from `ruvyxa/server` returns a `Response` for an `app/api/`
handler or a loader to return. Before they were named apart both were `notFound`, so a page that
imported the server half rendered a `Response` object and an API route that imported the browser
half threw instead of answering.

**Why is an environment value missing in the browser?** Only `RUVYXA_PUBLIC_*` is intentionally
available client-side. Move secrets or server-only computation out of client code rather than
changing the prefix.

**How do I upgrade safely?** Move `ruvyxa` and every `@ruvyxa/*` package together — they ship as one
set and expect matching versions. Then run `npm run check`, `npm run build`, and
`npm run test:parity` against your app, in that order. `check` typechecks first, so a config key or
an export that is no longer part of the public surface is reported by name rather than failing at
runtime; `build` and `test:parity` catch behaviour that moved.

This manual describes the framework as it is now, not as it was. `CHANGELOG.md` in the repository is
where release-to-release history lives.

**Previous:** [Deploy, run, and operate in production](15-deploy-run-and-operate.md) · **Next:**
[Public API reference](17-public-api-reference.md)
