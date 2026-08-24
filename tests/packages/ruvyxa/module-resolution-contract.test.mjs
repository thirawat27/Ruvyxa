/**
 * The JavaScript graph's half of `tests/fixtures/module-resolution-conformance.json`.
 *
 * Ruvyxa resolves every import twice — once in `crates/ruvyxa_bundler` for
 * `ruvyxa build`, once in `packages/ruvyxa/runtime/compiler.mjs` for the dev
 * server, the prerender workers, and every function artifact. A specifier the
 * two answer differently is not a build error; it is a bundle that runs
 * different code than the one the build reported.
 *
 * The Rust half is `package_exports_resolution_matches_the_shared_table` in
 * `crates/ruvyxa_bundler/src/resolver.rs`.
 */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const modulePath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/package-exports.mjs')

const compilerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs')
const { probeFileCandidate } = await import(`file://${compilerPath.replaceAll('\\', '/')}`)

const {
  PACKAGE_EXPORT_TARGETS,
  isSafePackageRelativePath,
  legacyEntryCandidates,
  packageNameAndExportKey,
  resolveExportsEntry,
} = await import(`file://${modulePath.replaceAll('\\', '/')}`)

const contract = JSON.parse(
  readFileSync(
    path.join(workspaceRoot, 'tests/fixtures/module-resolution-conformance.json'),
    'utf8',
  ),
)

/** The fixture's shape for one outcome, so both halves compare the same thing. */
function describeOutcome(outcome) {
  if (outcome.kind === 'targets') return outcome.targets
  return outcome.kind
}

describe('package exports resolution', () => {
  for (const testCase of contract.exports) {
    it(testCase.name, () => {
      // Parsed here rather than stored as JSON: condition order is what the
      // rule reads, and a parsed-then-reserialised object would lose it.
      const exportsField = JSON.parse(testCase.exportsJson)
      for (const target of PACKAGE_EXPORT_TARGETS) {
        assert.deepEqual(
          describeOutcome(resolveExportsEntry(exportsField, testCase.key, target)),
          testCase.results[target],
          `${testCase.name} disagrees for target ${target}`,
        )
      }
    })
  }

  it('refuses a bundle target the shared rule does not define', () => {
    assert.throws(() => resolveExportsEntry({ '.': './a.js' }, '.', 'node'), /RUV1810/)
  })

  it('treats a missing exports field as unmatched rather than blocked', () => {
    // Blocked means "the package author took this away"; absent means "ask the
    // legacy fields". Collapsing the two would make every package without an
    // exports map unresolvable.
    for (const target of PACKAGE_EXPORT_TARGETS) {
      assert.equal(resolveExportsEntry(undefined, '.', target).kind, 'unmatched')
      assert.equal(resolveExportsEntry(null, '.', target).kind, 'blocked')
    }
  })
})

describe('package specifier splitting', () => {
  for (const testCase of contract.specifiers) {
    it(`splits ${JSON.stringify(testCase.specifier)}`, () => {
      const split = packageNameAndExportKey(testCase.specifier)
      if (testCase.package === null) {
        assert.equal(split, null)
        return
      }
      assert.deepEqual(split, { name: testCase.package, key: testCase.key })
    })
  }
})

describe('legacy entry fields', () => {
  for (const testCase of contract.legacyEntries) {
    it(testCase.name, () => {
      for (const target of PACKAGE_EXPORT_TARGETS) {
        assert.deepEqual(
          legacyEntryCandidates(testCase.manifest, testCase.key, target),
          testCase.results[target],
          `${testCase.name} disagrees for target ${target}`,
        )
      }
    })
  }
})

describe('package-relative path safety', () => {
  it('refuses every path the shared table calls unsafe', () => {
    for (const relative of contract.unsafeRelativePaths) {
      assert.equal(
        isSafePackageRelativePath(relative),
        false,
        `${JSON.stringify(relative)} must not be joined onto a package directory`,
      )
    }
  })

  it('accepts ordinary package-relative paths', () => {
    for (const relative of ['index.js', 'dist/index.mjs', 'a/b/c.js']) {
      assert.equal(isSafePackageRelativePath(relative), true, relative)
    }
  })
})

describe('file probing', () => {
  for (const testCase of contract.fileProbe) {
    it(testCase.name, () => {
      // Realpath once, and probe from the resolved form. On macOS `os.tmpdir()`
      // is `/var/folders/…`, a symlink to `/private/var/folders/…`: probing the
      // symlinked path and measuring the answer against the real one produced a
      // relative path of nothing but `../`, and the whole table failed on that
      // host alone.
      const directory = realpathSync(mkdtempSync(path.join(tmpdir(), 'ruvyxa-probe-')))
      try {
        for (const file of testCase.files) {
          const target = path.join(directory, file)
          mkdirSync(path.dirname(target), { recursive: true })
          writeFileSync(target, '')
        }

        const resolved = probeFileCandidate(path.resolve(directory, testCase.specifier))
        const answered = resolved ? path.relative(directory, resolved).replaceAll('\\', '/') : null
        assert.equal(answered, testCase.expect, `${testCase.name} disagrees with the shared table`)
      } finally {
        rmSync(directory, { recursive: true, force: true })
      }
    })
  }
})
