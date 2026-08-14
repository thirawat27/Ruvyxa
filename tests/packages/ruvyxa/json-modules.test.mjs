/**
 * Module-kind regression coverage for the bundler's dependency walk.
 *
 * The incident these tests exist for: a serverless adapter bundles an SDK with
 * `bundlePackages: true`, the SDK reads its own version through
 * `require('../../package.json')`, and the resolver handed that JSON file
 * straight to the JavaScript transform. Every adapter build that touched such an
 * SDK failed with a syntax error pointing inside someone else's package.
 *
 * The correction is a module-kind contract between "resolve a file" and
 * "transform a source", so these tests assert the contract, not the one SDK.
 */

import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { pathToFileURL } from 'node:url'

import { compileBundle } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaces = []
after(() =>
  Promise.all(workspaces.map((root) => rm(root, { recursive: true, force: true, maxRetries: 5 }))),
)

async function withProject(run) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-json-modules-'))
  workspaces.push(root)
  return run(root)
}

/** Bundle `entrySource` and import the result, returning its exports. */
async function bundleAndImport(root, entrySource, options = {}) {
  const outfile = path.join(root, `bundle-${Math.random().toString(36).slice(2)}.mjs`)
  await compileBundle({
    projectRoot: root,
    entrySource,
    sourcefile: path.join(root, 'entry.ts'),
    outfile,
    platform: 'node',
    ...options,
  })
  return import(pathToFileURL(outfile).href)
}

/** Await `run`, returning the error it threw. Fails the test if it resolves. */
async function rejection(run) {
  try {
    await run()
  } catch (error) {
    return error
  }
  assert.fail('expected the bundle to fail')
}

describe('JSON modules', () => {
  it('bundles a package that reads its own package.json through require()', () =>
    withProject(async (root) => {
      // The exact shape from the incident: a CommonJS dependency reaching for a
      // JSON file that sits outside its own source directory.
      const sdk = path.join(root, 'node_modules', 'fake-sdk')
      await mkdir(path.join(sdk, 'build', 'cjs'), { recursive: true })
      await writeFile(
        path.join(sdk, 'package.json'),
        JSON.stringify({ name: 'fake-sdk', version: '4.2.1', main: 'build/cjs/index.cjs' }),
      )
      await writeFile(
        path.join(sdk, 'build', 'cjs', 'index.cjs'),
        "const pkg = require('../../package.json')\n" +
          'module.exports = { userAgent: `fake-sdk/${pkg.version}` }\n',
      )

      const bundled = await bundleAndImport(
        root,
        "import sdk from 'fake-sdk'\nexport const agent = sdk.userAgent\n",
        { bundlePackages: true },
      )

      assert.equal(bundled.agent, 'fake-sdk/4.2.1')
    }))

  it('gives a default import the whole document, like Node', () =>
    withProject(async (root) => {
      await writeFile(
        path.join(root, 'config.json'),
        JSON.stringify({ name: 'ruvyxa', nested: { count: 2 } }),
      )

      const bundled = await bundleAndImport(
        root,
        "import config from './config.json'\nexport const value = config\n",
      )

      assert.deepEqual(bundled.value, { name: 'ruvyxa', nested: { count: 2 } })
    }))

  it('keeps a document that has its own `default` key readable through require()', () =>
    withProject(async (root) => {
      // Attaching the ESM default self-reference must never overwrite data the
      // application can read.
      await writeFile(path.join(root, 'keyed.json'), JSON.stringify({ default: 'mine', a: 1 }))

      const bundled = await bundleAndImport(
        root,
        "const keyed = require('./keyed.json')\nexport const value = keyed\n",
      )

      assert.equal(bundled.value.default, 'mine')
      assert.equal(bundled.value.a, 1)
    }))

  it('supports named imports and array documents', () =>
    withProject(async (root) => {
      await writeFile(path.join(root, 'data.json'), JSON.stringify({ version: '9.9.9' }))
      await writeFile(path.join(root, 'list.json'), JSON.stringify(['a', 'b']))

      const bundled = await bundleAndImport(
        root,
        "import { version } from './data.json'\n" +
          "import list from './list.json'\n" +
          'export const value = { version, list }\n',
      )

      assert.deepEqual(bundled.value, { version: '9.9.9', list: ['a', 'b'] })
    }))

  it('round-trips a document whose strings look like code', () =>
    withProject(async (root) => {
      // The payload is emitted as one string literal precisely so nothing in it
      // can be read as JavaScript.
      const tricky = {
        script: '</script><script>alert(1)</script>',
        quotes: `back\\slash "double" 'single'    `,
        code: "require('./nope.json'); import('./nope.js')",
        exports: 'module.exports = 1',
      }
      await writeFile(path.join(root, 'tricky.json'), JSON.stringify(tricky))

      const bundled = await bundleAndImport(
        root,
        "import tricky from './tricky.json'\nexport const value = tricky\n",
      )

      assert.deepEqual(bundled.value, tricky)
    }))

  it('reports invalid JSON as a JSON diagnostic, not a JavaScript syntax error', () =>
    withProject(async (root) => {
      await writeFile(path.join(root, 'broken.json'), '{ "a": }')

      const error = await rejection(() =>
        bundleAndImport(root, "import broken from './broken.json'\nexport const value = broken\n"),
      )
      assert.match(error.message, /RUV1805/)
      assert.match(error.message, /broken\.json/)
      assert.doesNotMatch(error.message, /RUV1802/)
    }))

  it('names an uncompilable module kind instead of failing inside the transform', () =>
    withProject(async (root) => {
      await writeFile(path.join(root, 'native.node'), 'not javascript at all')

      const error = await rejection(() =>
        bundleAndImport(
          root,
          "const native = require('./native.node')\nexport const value = native\n",
        ),
      )
      assert.match(error.message, /RUV1806/)
      assert.match(error.message, /native\.node/)
      // The importer has to be named: the point of the diagnostic is telling the
      // developer which import pulled the file in.
      assert.match(error.message, /entry\.ts/)
      assert.doesNotMatch(error.message, /RUV1802/)
    }))
})
