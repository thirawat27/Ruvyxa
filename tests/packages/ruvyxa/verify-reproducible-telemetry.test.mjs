import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { RUST_CONST, declarations } from '../../../scripts/check-cross-language-constants.mjs'
import {
  TELEMETRY_FIELDS,
  differsOnlyInTelemetry,
  isTelemetryField,
  parseArgs,
  withoutTelemetry,
} from '../../../scripts/verify-reproducible.mjs'

const repoFile = (relative) => fileURLToPath(new URL(`../../../${relative}`, import.meta.url))
const scriptPath = repoFile('scripts/verify-reproducible.mjs')

/** The field names `bench.rs` treats as "how the build ran". */
function rustTelemetryFields() {
  const found = declarations([repoFile('crates/ruvyxa_cli/src/bench.rs')], RUST_CONST, true)
  const [copy] = found.get('TELEMETRY_FIELDS') ?? []
  assert.ok(copy, 'bench.rs still declares TELEMETRY_FIELDS')
  return [...copy.value.matchAll(/"([^"]+)"/g)].map((match) => match[1])
}

describe('the telemetry list this script strips', () => {
  /**
   * The two lists are `unrelated` in the cross-language registry — they
   * normalize different files for different comparisons — but "unrelated" is
   * not "unconstrained". A field `bench.rs` calls telemetry describes how a
   * build ran, and that is true whichever comparison is asking, so this list
   * has to cover that one. What it may not do is shrink to match: it compares
   * two cold builds across every emitted JSON file, and `bench.rs` compares one
   * file across a cold and a warm build.
   */
  it('covers every field the Rust half calls telemetry', () => {
    for (const field of rustTelemetryFields()) {
      assert.ok(
        isTelemetryField(field),
        `bench.rs strips ${field} and this script would report it as a non-reproducible ` +
          'difference. The lists differ by design; this direction is not one of the differences.',
      )
    }
  })

  /**
   * `bench.rs` removes the whole `cache` object and so never names what is in
   * it. This script keeps the object — a deployed build reads it — so every
   * counter inside has to be named here one at a time. That is the substance of
   * the registry's `unrelated` reason, and it is what makes the extra entries
   * correct rather than drift.
   */
  it('names the per-build counters that bench.rs drops with the whole cache object', () => {
    for (const counter of ['graphHits', 'hits', 'misses']) {
      assert.ok(TELEMETRY_FIELDS.has(counter), `${counter} lives inside the kept cache object`)
    }
  })

  it('matches every duration by shape rather than by name', () => {
    assert.ok(isTelemetryField('durationMs'))
    assert.ok(isTelemetryField('someBrandNewPhaseMs'))
    assert.equal(isTelemetryField('chunks'), false)
    assert.equal(isTelemetryField('MsPrefixed'), false)
  })

  it('strips the build clock, which only this comparison sees', () => {
    assert.ok(
      isTelemetryField('createdAtUnix'),
      'build.json carries a wall-clock stamp and is in this comparison; two builds of identical ' +
        'source would otherwise never be reproducible',
    )
  })
})

describe('classifying one file as telemetry-only', () => {
  const stringify = (value) => Buffer.from(JSON.stringify(value), 'utf8')

  it('folds a nested duration and a build clock away', () => {
    const before = stringify({
      createdAtUnix: 1,
      routes: 4,
      timing: { compileMs: 120, prerenderMs: 40 },
      cache: { hits: 0, misses: 9 },
    })
    const after = stringify({
      createdAtUnix: 2,
      routes: 4,
      timing: { compileMs: 999, prerenderMs: 1 },
      cache: { hits: 9, misses: 0 },
    })

    assert.equal(differsOnlyInTelemetry('build.json', before, after), true)
  })

  it('refuses to fold an emitted artifact name away', () => {
    const before = stringify({ createdAtUnix: 1, entry: 'client/app-a1b2.js' })
    const after = stringify({ createdAtUnix: 2, entry: 'client/app-c3d4.js' })

    assert.equal(differsOnlyInTelemetry('client-report.json', before, after), false)
  })

  it('refuses anything that is not JSON, whatever its bytes look like', () => {
    assert.equal(
      differsOnlyInTelemetry('assets/sw.js', Buffer.from('const CACHE = "a"'), Buffer.from('b')),
      false,
    )
    assert.equal(differsOnlyInTelemetry('broken.json', Buffer.from('{'), Buffer.from('{')), false)
  })

  it('walks into an array, because `routes` is one', () => {
    assert.deepEqual(withoutTelemetry([{ path: '/', durationMs: 3 }]), [{ path: '/' }])
  })
})

describe('the arguments the CI lane passes', () => {
  it('defaults to the broad feature fixture', () => {
    assert.deepEqual(parseArgs([]), { root: 'examples/demo', keep: false, strict: false })
  })

  it('accepts the root the workflow names', () => {
    assert.equal(parseArgs(['--root', 'examples/deploy-smoke']).root, 'examples/deploy-smoke')
  })

  it('accepts --keep and --strict', () => {
    const options = parseArgs(['--keep', '--strict'])
    assert.equal(options.keep, true)
    assert.equal(options.strict, true)
  })

  it('refuses a --root with no value, rather than building the default one', () => {
    assert.throws(() => parseArgs(['--root']), /--root needs a project directory/)
  })

  it('refuses an argument it does not know', () => {
    assert.throws(() => parseArgs(['--fast']), /unknown argument/)
  })
})

describe('the script itself', () => {
  it('builds nothing when imported, so this file can read its pieces', () => {
    const source = readFileSync(scriptPath, 'utf8')

    assert.match(source, /import\.meta\.filename\)\s*\{/)
    assert.doesNotMatch(
      source,
      /^try \{\n {2}main\(\)/m,
      'an unguarded `main()` here runs two full cargo builds on import',
    )
  })
})
