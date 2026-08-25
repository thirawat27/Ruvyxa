#!/usr/bin/env node
/**
 * Materialize a deployable fixture for a target that cannot serve every route.
 *
 * `examples/deploy-smoke` is the app every self-hosted adapter deploys, and it
 * has an ISR route on purpose — that route is the only thing exercising a
 * pre-rendered read and a revalidated write together. An edge target has no ISR
 * (`adapter.supports` says so, and the build refuses such a route with
 * `RUV2202`), so the cloudflare adapter could not build that fixture at all.
 *
 * The result was that the one adapter whose bundles run with no `process` had
 * no deployment coverage of any kind: it was checked by asserting the shape of
 * the files it wrote. A `NODE_ENV` left at `"development"` in every worker's
 * server-components SSR pass lived there until somebody built one by hand.
 *
 * Copying and pruning rather than keeping a second app: two hand-maintained
 * fixtures of the same application drift, and the drift is invisible until the
 * day one of them is the only one that would have caught something.
 *
 * The routes to drop are named on the command line rather than detected. A
 * scanner here would be a second opinion about what makes a route ISR, and the
 * loud failure is better anyway: add another ISR route and the cloudflare build
 * fails with `RUV2202` naming it, which is a message, not a silent omission.
 *
 * usage: node scripts/edge-fixture.mjs <source-root> [--out <dir>] [--drop <route-dir>]...
 * prints: the fixture root, for the caller to build and smoke.
 */
import { cp, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import path from 'node:path'

const argv = process.argv.slice(2)
const source = path.resolve(argv[0] ?? '')
const drops = argv.flatMap((value, index) => (argv[index - 1] === '--drop' ? [value] : []))
// Named so two fixtures of the same app can exist at once: CI builds the edge
// one and the static one from `examples/deploy-smoke` in the same job, and a
// shared directory would mean each adapter smoked whichever ran last.
// Read by index rather than `argv[indexOf('--out') + 1]`: with the flag absent
// `indexOf` is -1 and that expression is `argv[0]`, the source root — so an
// invocation with no `--out` wrote the fixture to
// `examples/deploy-smoke/examples/deploy-smoke`, outside the gitignore rule and
// invisible until `prettier --check` walked into it.
const outIndex = argv.indexOf('--out')
const outName = outIndex === -1 ? undefined : argv[outIndex + 1]
if (!argv[0] || !existsSync(source) || (outIndex !== -1 && !outName?.startsWith('.'))) {
  console.error(
    'usage: node scripts/edge-fixture.mjs <source-root> [--out <dir>] [--drop <route-dir>]...',
  )
  console.error('  --out names a dot-directory inside the source app; it defaults to .edge')
  process.exit(2)
}

/**
 * Beside the app it was copied from, not in a temporary directory.
 *
 * pnpm writes **relative** symlinks — `node_modules/react` points at
 * `../../node_modules/.pnpm/react@19.2.8/node_modules/react` — so a copy or a
 * junction of `node_modules` somewhere else resolves nothing at all. Every
 * bare specifier failed, and the build stopped at `RUV1863` claiming
 * `react-server-dom-webpack` was not installed when it was.
 *
 * Placed here the fixture needs no `node_modules` of its own: resolution walks
 * up into the source app's, exactly as a nested directory should. `.edge/` is
 * gitignored for both fixtures, and the parent's `tsconfig.json` includes only
 * `app`, so neither `tsc` nor knip sees the copy.
 */
const root = path.join(source, outName ?? '.edge')
await rm(root, { recursive: true, force: true })

for (const entry of ['app', 'public', 'ruvyxa.config.ts', 'package.json']) {
  const from = path.join(source, entry)
  if (existsSync(from)) await cp(from, path.join(root, entry), { recursive: true })
}

for (const route of drops) {
  const directory = path.join(root, 'app', ...route.split('/'))
  if (!existsSync(directory)) throw new Error(`--drop ${route}: no such route directory`)
  await rm(directory, { recursive: true, force: true })
}

// Not the source's: that one `extends` a base config and names `paths` one
// directory further up than this copy sits. Bare specifiers do not need it —
// they resolve through the parent's `node_modules` — so this is the minimum the
// compiler needs to read rather than the app's editor settings.
await writeFile(
  path.join(root, 'tsconfig.json'),
  `${JSON.stringify(
    {
      extends: '../tsconfig.json',
      include: ['app', 'ruvyxa.config.ts', '.ruvyxa/types/**/*.d.ts'],
    },
    null,
    2,
  )}\n`,
)

process.stdout.write(`${root}\n`)
