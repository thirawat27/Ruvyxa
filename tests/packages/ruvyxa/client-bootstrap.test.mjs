import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it } from 'node:test'

import {
  BOOTSTRAP_ELEMENT_ID,
  clientBootstrapPrelude,
} from '../../../packages/ruvyxa/runtime/entry-templates.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')

const fixture = JSON.parse(
  readFileSync(path.join(repoRoot, 'tests/fixtures/client-bootstrap-conformance.json'), 'utf8'),
)

const outputRs = readFileSync(path.join(repoRoot, 'crates/ruvyxa_bundler/src/output.rs'), 'utf8')

/**
 * Run a generated prelude against a stand-in document and report the globals.
 *
 * The prelude is source text destined for a browser bundle, so executing it is
 * the only check that means anything — matching it against a regular expression
 * would pass on a prelude that reads the wrong key.
 */
function runPrelude(source, blockText) {
  const globals = {}
  const document =
    blockText === null
      ? { getElementById: () => null }
      : {
          getElementById: (id) => (id === BOOTSTRAP_ELEMENT_ID ? { textContent: blockText } : null),
        }
  // `globalThis` inside the prelude has to resolve to the recording object, so
  // the source runs as a function body with both names bound as parameters.
  const run = new Function('globalThis', 'document', source)
  run(globals, document)
  return globals
}

describe('client bootstrap contract', () => {
  const { elementId, keys, globals: globalNames } = fixture

  it('publishes the element id the shared fixture names', () => {
    assert.equal(BOOTSTRAP_ELEMENT_ID, elementId)
  })

  it('reads the fixture keys into the fixture globals', () => {
    const block = JSON.stringify({ [keys.params]: { slug: 'a' }, [keys.path]: '/blog/a' })
    const result = runPrelude(clientBootstrapPrelude(), block)
    assert.deepEqual(result[globalNames.params], { slug: 'a' })
    assert.equal(result[globalNames.path], '/blog/a')
    assert.equal(result[globalNames.csr], undefined)
  })

  it('sets the CSR marker only when the block asks for it', () => {
    const shell = JSON.stringify({ [keys.params]: {}, [keys.path]: '/', [keys.csr]: true })
    assert.equal(runPrelude(clientBootstrapPrelude(), shell)[globalNames.csr], true)
  })

  it('does not overwrite params a soft navigation already wrote', () => {
    // The router writes the params for the route it is entering before that
    // route's bundle is evaluated. Assigning rather than defaulting here would
    // replace them with the ones the document was originally served with.
    const source = clientBootstrapPrelude()
    const globals = { [globalNames.params]: { slug: 'navigated' } }
    const block = JSON.stringify({ [keys.params]: { slug: 'served' }, [keys.path]: '/served' })
    new Function('globalThis', 'document', source)(globals, {
      getElementById: () => ({ textContent: block }),
    })
    assert.deepEqual(globals[globalNames.params], { slug: 'navigated' })
  })

  it('survives a missing or malformed block rather than throwing', () => {
    // A thrown error here would abort the module before hydration, turning a
    // stripped or truncated document into a blank page.
    assert.doesNotThrow(() => runPrelude(clientBootstrapPrelude(), null))
    assert.doesNotThrow(() => runPrelude(clientBootstrapPrelude(), 'not json'))
    assert.equal(runPrelude(clientBootstrapPrelude(), 'not json')[globalNames.params], undefined)
  })

  it('is mirrored byte-for-byte by the Rust bundler', () => {
    // Both module graphs emit a client entry, and a project renders through
    // whichever one built it. A prelude that drifted would read the block in
    // one and not the other, so hydration would start with empty parameters
    // depending only on how the bundle was produced.
    const rust = outputRs.match(/const CLIENT_BOOTSTRAP_PRELUDE: &str = r#"([\s\S]*?)"#;/)
    assert.ok(rust, 'CLIENT_BOOTSTRAP_PRELUDE not found in output.rs')
    assert.equal(rust[1], clientBootstrapPrelude())
  })

  it('is a data block rather than executable script in the Rust writers', () => {
    // `type="application/json"` is what keeps `script-src` from applying. A
    // writer that dropped it would reintroduce the inline script that a strict
    // Content-Security-Policy blocks.
    const htmlDocument = readFileSync(
      path.join(repoRoot, 'crates/ruvyxa_dev_server/src/html_document.rs'),
      'utf8',
    )
    assert.match(htmlDocument, new RegExp(`type="${fixture.scriptType}"`))

    // Every writer, matched on the globals rather than on one of them. A
    // fourth writer emitted only the path and the CSR flag, so a search for the
    // route-params global missed it and it kept shipping an inline script after
    // the other three were converted.
    const globalNamePattern = Object.values(fixture.globals).join('|')
    for (const file of [
      'crates/ruvyxa_dev_server/src/html_document.rs',
      'crates/ruvyxa_dev_server/src/render_pipeline.rs',
      'crates/ruvyxa_cli/src/prerender.rs',
    ]) {
      // Comments stripped first: these files explain in prose what the old
      // inline script looked like, and the pattern spans lines so that a
      // multi-line emitter cannot hide from it.
      const source = readFileSync(path.join(repoRoot, file), 'utf8')
        .split(/\r?\n/)
        .filter((line) => !line.trimStart().startsWith('//'))
        .join('\n')
      assert.doesNotMatch(
        source,
        new RegExp(`<script>[^<]*(${globalNamePattern})`),
        `${file} still writes the bootstrap as an executable inline script`,
      )
    }
  })
})
