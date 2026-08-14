import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import { repoPath } from '../../repo-root.ts'
import {
  DEFAULT_SECURITY_HEADERS,
  headersFileContents,
} from '../../../packages/@ruvyxa/core/dist/utils.js'

const fixture = JSON.parse(
  readFileSync(repoPath('tests/fixtures/security-headers-conformance.json'), 'utf8'),
) as { headers: Record<string, string> }

/**
 * `crates/ruvyxa_dev_server/src/response.rs` replays this same file. The two
 * lists cannot import each other, so a header added to one language and not the
 * other would leave the same site protected differently depending on where it
 * is deployed — with no build failure and nothing in a log to notice.
 */
describe('default security headers', () => {
  it('serves exactly the shared conformance list', () => {
    assert.deepEqual(
      Object.fromEntries(
        Object.entries(DEFAULT_SECURITY_HEADERS).map(([name, value]) => [
          name.toLowerCase(),
          value,
        ]),
      ),
      Object.fromEntries(
        Object.entries(fixture.headers).map(([name, value]) => [name.toLowerCase(), value]),
      ),
      'the fixture decides the list; Rust replays the same file',
    )
  })

  it('writes every header into the _headers file hosts read', () => {
    const contents = headersFileContents()
    for (const [name, value] of Object.entries(fixture.headers)) {
      assert.ok(
        contents.includes(`  ${name}: ${value}\n`),
        `_headers must carry ${name}; a host that reads this file is the only place it gets applied`,
      )
    }
  })
})
