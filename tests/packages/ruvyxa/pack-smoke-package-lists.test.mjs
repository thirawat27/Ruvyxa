import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { before, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const repoRoot = fileURLToPath(new URL('../../../', import.meta.url))
const scriptPath = fileURLToPath(new URL('../../../scripts/pack-smoke.mjs', import.meta.url))

/**
 * `pack-smoke.mjs` is a repository-root script: it discovers packages by
 * reading `packages/` relative to the working directory, the way pnpm does.
 * pnpm runs this suite from `packages/ruvyxa`, so the directory is moved before
 * the module is loaded. `node --test` gives each file its own process, so this
 * is scoped to this file.
 */
let smoke
let ruvyxaManifest
before(async () => {
  process.chdir(repoRoot)
  smoke = await import('../../../scripts/pack-smoke.mjs')
  ruvyxaManifest = JSON.parse(readFileSync(`${repoRoot}packages/ruvyxa/package.json`, 'utf8'))
})

/** The `@ruvyxa/*` workspace packages the published `ruvyxa` package needs. */
const scopedDependencies = () =>
  Object.keys(ruvyxaManifest.dependencies ?? {})
    .filter((name) => name.startsWith('@ruvyxa/'))
    .sort()

describe('the packages the release smoke packs', () => {
  it('is every workspace package except the native CLIs, plus this host’s', async () => {
    const { workspacePackageDirs } = await import('../../../scripts/workspace-packages.mjs')
    const discovered = workspacePackageDirs().dirs.map(
      (dir) => JSON.parse(readFileSync(`${repoRoot}${dir}/package.json`, 'utf8')).name,
    )

    for (const name of discovered) {
      if (name.startsWith('@ruvyxa/cli-')) continue
      assert.ok(
        smoke.packages.includes(name),
        `${name} is a workspace package and would not be packed. The list is derived so this ` +
          'cannot happen; if it has, the derivation stopped matching how pnpm discovers packages.',
      )
    }
  })

  it('packs exactly one native CLI package: the one for this host', () => {
    const cliPackages = smoke.packages.filter((name) => name.startsWith('@ruvyxa/cli-'))

    assert.deepEqual(cliPackages, [`@ruvyxa/cli-${process.platform}-${process.arch}`])
  })

  it('names each package once', () => {
    assert.equal(new Set(smoke.packages).size, smoke.packages.length)
  })
})

describe('the overrides that keep the scaffolded install off npm', () => {
  it('covers every @ruvyxa dependency of the ruvyxa package', () => {
    assert.deepEqual(
      smoke.workspaceOverridePackages,
      scopedDependencies(),
      'an @ruvyxa dependency with no override sends `pnpm install --no-lockfile` to npm for a ' +
        'version that is not published yet, and npm names neither this file nor the missing entry',
    )
  })

  it('has a freshly packed tarball behind every override it writes', () => {
    for (const name of smoke.workspaceOverridePackages) {
      assert.ok(
        smoke.packages.includes(name),
        `${name} is overridden to a tarball that nothing packs`,
      )
    }
  })

  it('includes @ruvyxa/core, which used to be written out on its own line', () => {
    assert.ok(smoke.workspaceOverridePackages.includes('@ruvyxa/core'))
  })

  it('leaves the native CLI package out, because it is an optional dependency', () => {
    assert.deepEqual(
      smoke.workspaceOverridePackages.filter((name) => name.startsWith('@ruvyxa/cli-')),
      [],
      'the platform binary is not in `dependencies`, so it keeps its own override line',
    )
  })
})

describe('no hand-maintained copy is left', () => {
  it('names no adapter as a literal', () => {
    const source = readFileSync(scriptPath, 'utf8')

    assert.doesNotMatch(
      source,
      /'@ruvyxa\/adapter-/,
      'a twelfth adapter must not need an edit here; both lists are derived',
    )
  })

  it('does nothing when imported, so a test can read the lists', () => {
    const source = readFileSync(scriptPath, 'utf8')

    assert.match(source, /import\.meta\.filename\)\s*\{\s*\n\s*await main\(\)/)
  })
})
