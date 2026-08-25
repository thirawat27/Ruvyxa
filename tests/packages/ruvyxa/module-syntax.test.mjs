/**
 * Every source shape the linker has to carry, compiled and then run.
 *
 * Ruvyxa's linker rewrites ESM one statement at a time rather than parsing to an
 * AST and printing it back. That is a deliberate trade with a narrow, quiet
 * failure mode: a construct the rewriter does not recognise is copied through,
 * and the bundle either refuses to parse or — the expensive case — parses and
 * means something else.
 *
 * So each case is *executed*. Asserting that a build succeeded would have missed
 * half the defects this table was written from: an export dropped silently, an
 * identifier truncated at a non-ASCII character, a `Proxy` stepped over. The
 * only question that catches those is what the module evaluates to.
 *
 * The table is `tests/fixtures/module-syntax-conformance.json`. Adding a shape
 * is one entry there; nothing in this file needs to change.
 *
 * The project is written under `.test-build/` rather than into the OS temp
 * directory on purpose: the compiler resolves bare specifiers by walking up to
 * the nearest `node_modules`, and a project outside the workspace has none to
 * find.
 */
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { compileBundleWithMetadata } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const scratchRoot = path.join(workspaceRoot, '.test-build')
const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/module-syntax-conformance.json'), 'utf8'),
)

const projects = []
after(async () => {
  for (const project of projects) {
    await rm(project, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }).catch(
      () => {},
    )
  }
})

/**
 * The dependency's extension, chosen by what the entry imports.
 *
 * TypeScript-only syntax has to arrive in a file the compiler will treat as
 * TypeScript, and a case that imports `./dep.ts` is asking for exactly that.
 */
function dependencyName(entry) {
  if (entry.includes('./dep.tsx')) return 'dep.tsx'
  if (entry.includes('./dep.ts')) return 'dep.ts'
  return 'dep.js'
}

async function linkAndRun(testCase) {
  await mkdir(scratchRoot, { recursive: true })
  const root = await mkdtemp(path.join(scratchRoot, 'module-syntax-'))
  projects.push(root)
  await writeFile(path.join(root, dependencyName(testCase.entry)), testCase.dependency)
  const outfile = path.join(root, 'out.mjs')
  await compileBundleWithMetadata({
    projectRoot: root,
    entrySource: testCase.entry,
    sourcefile: 'entry.tsx',
    outfile,
    platform: 'node',
    bundleTarget: 'ssr',
    jsxRuntime: 'automatic',
  })
  return import(pathToFileURL(outfile).href)
}

describe('module syntax the linker carries', () => {
  assert.ok(fixture.cases.length > 0, 'the fixture must carry cases')

  for (const testCase of fixture.cases) {
    it(testCase.name, async () => {
      const module = await linkAndRun(testCase)
      assert.equal(module.value, testCase.expect, `${testCase.name} — ${testCase.why}`)
    })
  }
})
