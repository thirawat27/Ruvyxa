/**
 * The JavaScript half of the plugin-transform lane contract.
 *
 * `runtime/compiler.mjs` builds every server render; the Rust bundler builds the
 * browser bundle. Both run `build.onTransform` now, and both have to name the
 * lanes the same way — a plugin that branches on `environment` is choosing a
 * side, and it can only choose correctly if the two compilers agree on what the
 * sides are called.
 *
 * The Rust half replays the same file in `crates/ruvyxa_cli/src/plugins.rs`.
 */
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { readFileSync, realpathSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { compileBundleWithMetadata } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const fixture = JSON.parse(
  readFileSync(
    path.join(workspaceRoot, 'tests/fixtures/plugin-transform-lane-conformance.json'),
    'utf8',
  ),
)

/**
 * A project whose config pointer declares one plugin.
 *
 * The pointer module is what the compiler reads to find project plugins, and
 * writing it directly is what keeps this test about the compiler rather than
 * about the config renderer that normally produces it.
 */
async function project(pluginSource) {
  const root = await mkdtemp(path.join(realpathSync(os.tmpdir()), 'ruvyxa-lane-'))
  await mkdir(path.join(root, '.ruvyxa', 'cache', 'config'), { recursive: true })
  await writeFile(path.join(root, 'marker.js'), "export const MARKER = 'untouched'\n")
  await writeFile(path.join(root, 'plugin.mjs'), pluginSource)
  await writeFile(
    path.join(root, '.ruvyxa', 'cache', 'config', 'runtime-config.mjs'),
    "import plugin from '../../../plugin.mjs'\n" +
      'export default undefined\n' +
      'export const plugins = [plugin]\n',
  )
  return root
}

const RECORDING_PLUGIN = `
export default {
  name: 'lane-recorder',
  register({ build }) {
    build.onTransform(({ code, id, environment }) => {
      if (!id.endsWith('marker.js')) return undefined
      return code.replace("'untouched'", JSON.stringify('lane:' + environment))
    })
  },
}
`

async function compileFor({ root, platform, bundleTarget }) {
  const outfile = path.join(root, `out-${bundleTarget}.mjs`)
  await compileBundleWithMetadata({
    projectRoot: root,
    entrySource: "export { MARKER } from './marker.js'\n",
    sourcefile: 'entry.ts',
    outfile,
    platform,
    bundleTarget,
  })
  return readFileSync(outfile, 'utf8')
}

describe('plugin transform lanes', () => {
  it('tells a hook which lane it is transforming for, per the shared table', async () => {
    const root = await project(RECORDING_PLUGIN)
    try {
      for (const testCase of fixture.environments.cases) {
        const code = await compileFor({
          root,
          platform: testCase.platform,
          bundleTarget: testCase.bundleTarget,
        })
        assert.ok(
          code.includes(`lane:${testCase.expect}`),
          `${testCase.bundleTarget}/${testCase.platform} must report "${testCase.expect}" — ${testCase.why}`,
        )
      }
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('applies the same rewrite to a server compile that the browser compile gets', async () => {
    // The defect this covers produced no error anywhere: the browser bundle and
    // the server document simply disagreed, React discarded the server tree,
    // and the page flickered into correctness.
    const root = await project(`
export default {
  name: 'unguarded',
  register({ build }) {
    build.onTransform(({ code, id }) =>
      id.endsWith('marker.js') ? code.replace("'untouched'", "'rewritten'") : undefined,
    )
  },
}
`)
    try {
      const browser = await compileFor({ root, platform: 'browser', bundleTarget: 'client' })
      const server = await compileFor({ root, platform: 'node', bundleTarget: 'ssr' })
      assert.ok(browser.includes('rewritten'), 'the browser bundle must carry the rewrite')
      assert.ok(server.includes('rewritten'), 'so must the server render')
      assert.ok(!server.includes('untouched'), 'the original must not survive on the server')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('leaves a project with no plugins exactly as it was', async () => {
    const root = await mkdtemp(path.join(realpathSync(os.tmpdir()), 'ruvyxa-lane-'))
    try {
      await writeFile(path.join(root, 'marker.js'), "export const MARKER = 'untouched'\n")
      const code = await compileFor({ root, platform: 'node', bundleTarget: 'ssr' })
      assert.ok(code.includes('untouched'))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})
