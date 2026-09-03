import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

import { CONFIG_KEY_SCHEMA } from '../../../packages/ruvyxa/runtime/config-schema.mjs'

const run = promisify(execFile)
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const renderer = path.join(repoRoot, 'packages/ruvyxa/runtime/config-renderer.mjs')

/**
 * Render one `ruvyxa.config.ts` and hand back the config the CLI would read.
 *
 * The config exports a plain object rather than calling `config()`, which is
 * the identity function — so this needs no package resolution and the temporary
 * project can live anywhere.
 */
async function render(source) {
  const root = await mkdtemp(path.join(tmpdir(), 'ruvyxa-config-render-'))
  try {
    await writeFile(path.join(root, 'ruvyxa.config.ts'), source)
    const { stdout } = await run(process.execPath, [renderer, root], { cwd: repoRoot })
    const response = JSON.parse(stdout)
    assert.equal(response.ok, true, `the renderer refused the config: ${stdout}`)
    return response.config
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}

/**
 * A key is accepted by the renderer or it does not exist.
 *
 * `CONFIG_KEY_SCHEMA` accepted `cache.handler`, `cache.maxEntries`, and
 * `cache.maxBytes`; `RuvyxaConfig` declared all three; the Rust `ProjectConfig`
 * deserialized all three; the documentation described all three. And
 * `cacheValue()` — the function that decides what actually leaves this process
 * — copied `routes`, `css`, and `dir` and dropped the rest on the floor. So a
 * project could set a shared cache handler, pass every check, and have the
 * value reach no consumer on any host: the store was never loaded and both
 * memory bounds were lost on the way to the tier they bound.
 *
 * The schema test next door compares the declarations with each other. Only
 * rendering catches a key that every declaration agrees on and nothing emits.
 */
describe('the config renderer emits the cache keys it accepts', () => {
  it('carries handler, maxEntries, and maxBytes through to the CLI', async () => {
    const config = await render(
      `export default {
  appDir: 'app',
  cache: { handler: './cache-handler.mjs', maxEntries: 512, maxBytes: 1048576 },
}
`,
    )
    assert.deepEqual(config.cache, {
      handler: './cache-handler.mjs',
      maxEntries: 512,
      maxBytes: 1048576,
    })
  })

  // Zero is the value that carries the decision in both bounds — no local tier,
  // and no memory ceiling — so a carrier that treated it as "unset" would
  // answer the two questions the feature exists to answer with the default
  // while looking wired.
  it('keeps a zero bound, which is a decision rather than an absence', async () => {
    const config = await render(
      `export default { appDir: 'app', cache: { maxEntries: 0, maxBytes: 0 } }
`,
    )
    assert.equal(config.cache.maxEntries, 0)
    assert.equal(config.cache.maxBytes, 0)
  })

  // The keys this suite asserts are the ones the schema declares, read from the
  // schema rather than written out again: a key added there and forgotten here
  // fails this rather than shipping unrendered.
  it('emits every key the schema declares for the cache section', async () => {
    const declared = [...CONFIG_KEY_SCHEMA['config.cache']].sort()
    const config = await render(
      `export default {
  appDir: 'app',
  cache: {
    routes: false,
    css: false,
    dir: '.cache',
    handler: './cache-handler.mjs',
    maxEntries: 8,
    maxBytes: 16,
  },
}
`,
    )
    assert.deepEqual(Object.keys(config.cache).sort(), declared)
  })

  // A project that configures nothing must still render nothing, so the CLI
  // keeps every default rather than being handed an empty object to interpret.
  it('omits the section when the project configures none of it', async () => {
    const config = await render(`export default { appDir: 'app' }\n`)
    assert.equal(config.cache, undefined)
  })
})
