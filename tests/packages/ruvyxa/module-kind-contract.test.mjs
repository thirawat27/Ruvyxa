/**
 * The server graph's half of `tests/fixtures/module-kind-conformance.json`.
 *
 * Which extensions compile is decided twice — by `MODULE_KIND_EXTENSIONS` in
 * `crates/ruvyxa_bundler/src/compiler.rs` for the client graph, and by the
 * list of the same name in `packages/ruvyxa/runtime/compiler.mjs` for the
 * server and prerender graph. Neither can import the other's, and until this
 * fixture the only thing asking them to agree was a doc comment on each. An
 * extension one accepts and the other refuses is a build that passes on the
 * client and fails at prerender with RUV1806 naming a dependency the
 * application never wrote.
 *
 * The Rust half is `compilable_module_kinds_match_the_shared_conformance_table`
 * in `crates/ruvyxa_bundler/src/compiler.rs`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const compilerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs')

const { assertSupportedModuleKind } = await import(`file://${compilerPath.replaceAll('\\', '/')}`)

const contract = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/module-kind-conformance.json'), 'utf8'),
)

/** Run the real guard the compiler runs, the way it calls it. */
function check(extension) {
  assertSupportedModuleKind(
    `/project/node_modules/pkg/file${extension}`,
    'pkg/file',
    '/project/app/page.tsx',
  )
}

describe('compilable module kinds', () => {
  it('compiles every extension in the shared fixture', () => {
    for (const extension of contract.extensions) {
      assert.doesNotThrow(() => check(extension), `${extension} must compile`)
    }
  })

  it('folds case, so an upper-case source is not refused by one graph alone', () => {
    for (const extension of contract.acceptedCasing) {
      assert.doesNotThrow(() => check(extension), `${extension} must compile`)
    }
  })

  it('refuses every extension the shared fixture rejects', () => {
    for (const extension of contract.rejected) {
      assert.throws(() => check(extension), /RUV1806/, `${extension} must be refused`)
    }
  })

  it('accepts an extensionless package entry point', () => {
    assert.equal(contract.extensionlessIsAccepted, true)
    assert.doesNotThrow(() => check(''))
  })
})
