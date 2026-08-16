import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { prerenderRelativePath } from '../../../packages/ruvyxa/runtime/serverless-handler.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))

describe('prerendered document path safety', () => {
  it('replays the cross-language table the native server also replays', async () => {
    const fixture = JSON.parse(
      await readFile(
        path.join(workspaceRoot, 'tests/fixtures/prerender-path-conformance.json'),
        'utf8',
      ),
    )
    assert.ok(Array.isArray(fixture.cases) && fixture.cases.length > 0)

    for (const testCase of fixture.cases) {
      // The fixture stores paths as the native server receives them — relative,
      // no leading slash. The handler takes a request pathname.
      const accepted = prerenderRelativePath(`/${testCase.path}`) !== null
      assert.equal(accepted, testCase.safe, `/${testCase.path} — ${testCase.why}`)
    }
  })

  it('maps an accepted path to a document inside the prerender directory', () => {
    assert.equal(prerenderRelativePath('/blog/hello'), 'blog/hello/index.html')
    assert.equal(prerenderRelativePath('/'), 'index.html')
  })
})
