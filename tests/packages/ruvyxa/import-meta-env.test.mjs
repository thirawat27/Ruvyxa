/**
 * `import.meta.env` becomes a frozen literal in every module kind the compiler
 * emits — the JavaScript half of the rule.
 *
 * The substitution was written as a step of the oxc transform, and the Rust
 * `compile_module` returns *before* that transform for `.js`, `.mjs`, `.cjs`
 * and for a `ruvyxa:` virtual entry. So the browser bundle kept the expression
 * verbatim for plain JavaScript — an app written in `.js`, and every
 * client-bundled dependency authored for Vite, because for the Client target
 * `node_modules` is not external. In a browser `import.meta` has no `env`, so
 * the module threw `TypeError` while it was being evaluated and took the whole
 * bundle with it rather than one expression.
 *
 * The Node graph never had that hole: it runs every module through
 * `transformModuleSource`, which always ends in `substitutePublicEnv`. That is
 * exactly why the divergence was invisible — `ruvyxa dev` rendered the page
 * perfectly and only the built bundle was broken.
 *
 * So this file is not a second copy of the Rust test. It is the other half of a
 * two-language contract: the table is
 * `tests/fixtures/env-policy-conformance.json` under `importMetaEnv`, and
 * `compiler::tests::import_meta_env_is_substituted_for_every_module_kind`
 * replays the same entries through the Rust graph. Adding an extension or a
 * case is one entry there and nothing in either replay changes.
 *
 * The project is written under `.test-build/` rather than the OS temp directory
 * for the reason `content-conformance.test.mjs` gives: the compiled module
 * resolves real dependencies, and a directory outside the workspace has no
 * `node_modules` to find them in.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { compileBundleWithMetadata } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const scratchRoot = path.join(workspaceRoot, '.test-build')
const section = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/env-policy-conformance.json'), 'utf8'),
).importMetaEnv

// `substitutePublicEnv` reads `process.env` directly, which is also how a real
// build supplies these values — there is no options field to pass instead.
process.env.RUVYXA_PUBLIC_API_URL = 'https://api.example.test'

const projects = []
after(async () => {
  for (const project of projects) {
    await rm(project, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }).catch(
      () => {},
    )
  }
})

/**
 * Compile one module of one extension and hand back the emitted bundle.
 *
 * The entry is always `.ts` so the extension under test is exercised as an
 * *imported* module rather than as the entry itself — which is the shape the
 * defect actually took, since a Vite-authored `.mjs` arrives as a dependency.
 */
async function compileWithExtension(extension, source) {
  await mkdir(scratchRoot, { recursive: true })
  const root = await mkdtemp(path.join(scratchRoot, 'import-meta-env-'))
  projects.push(root)

  await writeFile(path.join(root, `subject${extension}`), source)
  const outfile = path.join(root, 'bundle.js')

  await compileBundleWithMetadata({
    projectRoot: root,
    entrySource: `export * from './subject${extension}'\n`,
    sourcefile: 'ruvyxa:entry.ts',
    outfile,
    platform: 'browser',
    sourceMap: false,
  })
  return readFileSync(outfile, 'utf8')
}

describe('import.meta.env is substituted for every module kind', () => {
  assert.ok(section, 'the fixture must carry an importMetaEnv section')
  assert.ok(section.extensions.length > 0, 'the fixture must name extensions')
  assert.ok(section.cases.length > 0, 'the fixture must carry cases')

  for (const { extension, why: extensionWhy } of section.extensions) {
    for (const testCase of section.cases) {
      it(`${testCase.name} in a ${extension} module`, async () => {
        const emitted = await compileWithExtension(extension, testCase.source)
        const where = `${extension} (${extensionWhy}): ${testCase.why}`

        if (testCase.substituted) {
          assert.ok(
            !emitted.includes(section.marker),
            `${section.marker} survived into the bundle — ${where}\n${emitted}`,
          )
          assert.ok(
            emitted.includes(section.literalPrefix),
            `the frozen literal is missing — ${where}\n${emitted}`,
          )
        } else {
          assert.ok(
            emitted.includes(section.marker),
            `the expression should have been left alone — ${where}\n${emitted}`,
          )
        }
      })
    }
  }
})
