import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import { repoPath } from '../../repo-root.ts'
import {
  FALLBACK_CONTENT_TYPE,
  STATIC_ASSET_EXTENSIONS,
  STATIC_CONTENT_TYPES,
} from '../../../packages/@ruvyxa/core/dist/utils.js'
const standaloneServerText = readFileSync(
  repoPath('packages/@ruvyxa/core/src/standalone-server.ts'),
  'utf8',
)

const fixture = JSON.parse(
  readFileSync(repoPath('tests/fixtures/static-asset-conformance.json'), 'utf8'),
) as {
  contentTypes: Record<string, string>
  fallbackContentType: string
  caseInsensitiveExamples: Record<string, string>
  staticAssetExtensions: string[]
}

/**
 * The Rust static handler replays this same file in
 * `crates/ruvyxa_dev_server/src/static_assets.rs`. The two tables were written
 * independently and had drifted — `.wasm` and the web fonts existed only in
 * JavaScript, `.webmanifest` only in Rust — so the same file was served with a
 * different Content-Type depending on where it was deployed.
 */
describe('static asset contract', () => {
  it('serves every extension the shared conformance table declares', () => {
    assert.deepEqual(
      STATIC_CONTENT_TYPES,
      fixture.contentTypes,
      'the fixture decides the table; Rust replays the same file',
    )
  })

  it('falls back rather than guessing at an unknown extension', () => {
    assert.equal(FALLBACK_CONTENT_TYPE, fixture.fallbackContentType)
    assert.equal(STATIC_CONTENT_TYPES['bin'], undefined)
  })

  it('resolves an upper-case extension the way a file system reports it', () => {
    for (const [spelling, expected] of Object.entries(fixture.caseInsensitiveExamples)) {
      assert.equal(
        STATIC_CONTENT_TYPES[spelling.toLowerCase()],
        expected,
        `content type for .${spelling}`,
      )
    }
  })

  it('keeps the asset-extension list identical to its two other copies', () => {
    assert.deepEqual([...STATIC_ASSET_EXTENSIONS], fixture.staticAssetExtensions)
  })

  /**
   * The two lists answer different questions — "is this URL an asset?" and "how
   * is this file served?" — but every answer to the first needs an answer to the
   * second. They had different membership: a video or font in `public/` was
   * recognised as an asset and then served as `application/octet-stream`, which
   * stops a `<video>` playing and makes a browser download the font.
   */
  it('gives every recognised asset extension a content type', () => {
    const missing = STATIC_ASSET_EXTENSIONS.filter(
      (extension) => !(extension in STATIC_CONTENT_TYPES),
    )
    assert.deepEqual(missing, [], 'these would be served as an opaque download')
  })

  /**
   * The table used to be a literal inside the string `standaloneServerSource`
   * returns, which put it beyond the reach of every check above — that is how it
   * drifted from Rust in the first place. This asserts the generated server
   * still has no table of its own to drift with.
   *
   * Read as text rather than imported: the module's own `./utils.js` specifier
   * only resolves against built output, and a contract test should not need a
   * build to run.
   */
  it('gives the generated standalone server no content-type table of its own', () => {
    assert.ok(
      standaloneServerText.includes('JSON.stringify(STATIC_CONTENT_TYPES)'),
      'the generated server must serialize the shared table',
    )
    assert.ok(
      standaloneServerText.includes('JSON.stringify(FALLBACK_CONTENT_TYPE)'),
      'the generated server must serialize the shared fallback',
    )
    assert.ok(
      standaloneServerText.includes('path.extname(hit.file).slice(1).toLowerCase()'),
      'lookup must strip the leading dot and lowercase, matching the table keys',
    )

    // An asset content type spelled out in this file is a second table forming.
    // `text/plain` is deliberately not matched: the 500 handler sets it for an
    // error body, which is a response this server writes rather than a file it
    // serves, so it is not a table entry.
    const inlineAssetTypes = [
      ...standaloneServerText.matchAll(/'(?:image|font)\/[^']+'|'application\/(?:wasm|manifest)/g),
    ]
    assert.deepEqual(
      inlineAssetTypes.map((match) => match[0]),
      [],
      'asset content types belong in STATIC_CONTENT_TYPES, not in the generated server',
    )
  })
})
