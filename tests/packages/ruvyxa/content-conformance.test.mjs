/**
 * The `.mdx` shapes the content compiler has to read as code, compiled and run.
 *
 * A Markdown or MDX page is lowered to an ESM module, and the compiler appends
 * `frontmatter`, `meta`, `headings` and `contentFormat` only when the author did
 * not already export them. That decision used to be made by a private tokenizer
 * — one in `compiler.mjs`, a character-for-character copy of it in
 * `crates/ruvyxa_bundler/src/content.rs` — which knew line comments, block
 * comments and the three quote characters but had no regular-expression branch.
 * A page whose ESM block wrote `/['"]/` above its own `export const frontmatter`
 * lost the rest of the file to a string skip, so the generated export was
 * appended beside the author's and the module refused to parse.
 *
 * Each case is *executed*, for the same reason `module-syntax.test.mjs` executes
 * its output: the failure this table was written from is a duplicate top-level
 * declaration, which is not a syntax error the compiler can see and not
 * something an "it built" assertion would notice. Only evaluating the module
 * asks the question.
 *
 * The table is `tests/fixtures/content-conformance.json` and it is replayed from
 * Rust too, by `content::tests::compiles_the_shared_content_conformance_cases`.
 * Adding a shape is one entry there; nothing in this file needs to change.
 *
 * The project is written under `.test-build/` rather than into the OS temp
 * directory on purpose: the compiled module imports `react/jsx-runtime`, and a
 * directory outside the workspace has no `node_modules` to find it in.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { compileContentSource } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const scratchRoot = path.join(workspaceRoot, '.test-build')
const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/content-conformance.json'), 'utf8'),
)

const projects = []
after(async () => {
  for (const project of projects) {
    await rm(project, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }).catch(
      () => {},
    )
  }
})

/** How many times the compiled module declares `export const name`. */
function declarationCount(module, name) {
  return module.split(`export const ${name}`).length - 1
}

async function compileAndRun(testCase) {
  await mkdir(scratchRoot, { recursive: true })
  const root = await mkdtemp(path.join(scratchRoot, 'content-conformance-'))
  projects.push(root)

  const pageFile = path.join(root, testCase.file)
  const { source } = await compileContentSource(testCase.source, pageFile, root, false)

  for (const [name, contents] of Object.entries(testCase.dependencies ?? {})) {
    await writeFile(path.join(root, name), contents)
  }
  const modulePath = path.join(root, 'page.mjs')
  await writeFile(modulePath, source)
  return { source, module: await import(pathToFileURL(modulePath).href) }
}

describe('content modules the compiler generates exports for', () => {
  assert.ok(fixture.cases.length > 0, 'the fixture must carry cases')

  for (const testCase of fixture.cases) {
    it(testCase.name, async () => {
      const { source, module } = await compileAndRun(testCase)

      for (const [name, expected] of Object.entries(testCase.declarations)) {
        assert.equal(
          declarationCount(source, name),
          expected,
          `${name} declarations — ${testCase.why}\n${source}`,
        )
      }
      for (const [name, expected] of Object.entries(testCase.exports ?? {})) {
        assert.deepEqual(module[name], expected, `${name} export — ${testCase.why}`)
      }
      assert.equal(typeof module.default, 'function', 'the page must still export a component')
    })
  }
})
