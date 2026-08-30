import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  JS_CONST,
  REGISTRY,
  RUST_CONST,
  SOURCE_PATHSPEC,
  declarations,
  inspect,
  scalar,
  trackedSources,
} from '../../../scripts/check-cross-language-constants.mjs'

const repoFile = (relative) => fileURLToPath(new URL(`../../../${relative}`, import.meta.url))

const workspace = mkdtempSync(path.join(tmpdir(), 'ruvyxa-cross-language-'))
after(() => rmSync(workspace, { recursive: true, force: true }))

/** Write a source file into the scratch workspace and return its path. */
function write(name, source) {
  const file = path.join(workspace, name)
  writeFileSync(file, source)
  return file
}

/** A declaration map with one entry per name, for the comparison tests. */
const single = (entries) =>
  new Map(Object.entries(entries).map(([name, copies]) => [name, [].concat(copies)]))

describe('collecting declarations', () => {
  it('keeps every copy of a name, not only the first file to declare it', () => {
    const first = write('first.mjs', "export const SHARED_THING = 'one'\n")
    const second = write('second.mjs', "const SHARED_THING = 'one'\n")

    const found = declarations([first, second], JS_CONST, false)

    assert.deepEqual(
      found.get('SHARED_THING').map((copy) => copy.file),
      [first, second],
      'both declarations must survive collection; the gate cannot compare what it discarded',
    )
    assert.deepEqual(
      found.get('SHARED_THING').map((copy) => copy.value),
      ["'one'", "'one'"],
    )
  })

  it('keeps a copy that disagrees with the first one', () => {
    const first = write('agrees.mjs', 'export const CONTRACT_VERSION = 2\n')
    const second = write('drifted.mjs', 'const CONTRACT_VERSION = 1\n')

    const found = declarations([first, second], JS_CONST, false)

    assert.deepEqual(
      found.get('CONTRACT_VERSION').map((copy) => copy.value),
      ['2', '1'],
    )
  })

  it('still cuts Rust declarations at the test module and skips short names', () => {
    const source = write(
      'source.rs',
      [
        'pub const REAL_CONSTANT: u32 = 7;',
        'const ID: u32 = 3;',
        '',
        '#[cfg(test)]',
        'mod tests {',
        '    const REAL_CONSTANT: u32 = 9;',
        '}',
        '',
      ].join('\n'),
    )

    const found = declarations([source], RUST_CONST, true)

    assert.deepEqual(
      found.get('REAL_CONSTANT').map((copy) => copy.value),
      ['7'],
    )
    assert.equal(found.has('ID'), false)
  })
})

describe('sameValue comparison', () => {
  const registry = [
    {
      name: 'DEPLOY_MANIFEST_VERSION',
      kind: 'sameValue',
      why: 'The contract version the writer stamps and the reader refuses to exceed.',
    },
  ]

  it('passes when a repeated JavaScript copy says the same thing', () => {
    const { failures, compared } = inspect(
      single({ DEPLOY_MANIFEST_VERSION: [{ file: 'crates/a.rs', value: '1' }] }),
      single({
        DEPLOY_MANIFEST_VERSION: [
          { file: 'packages/@ruvyxa/core/src/deploy-manifest.ts', value: '1' },
          { file: 'packages/ruvyxa/runtime/adapter-runner.mjs', value: '1' },
        ],
      }),
      registry,
    )

    assert.deepEqual(failures, [])
    assert.equal(compared, 3, 'all three declarations are in reach of the comparison')
  })

  it('reports a divergent second JavaScript copy and names every file', () => {
    const { failures } = inspect(
      single({ DEPLOY_MANIFEST_VERSION: [{ file: 'crates/a.rs', value: '2' }] }),
      single({
        DEPLOY_MANIFEST_VERSION: [
          { file: 'packages/@ruvyxa/core/src/deploy-manifest.ts', value: '2' },
          { file: 'packages/ruvyxa/runtime/adapter-runner.mjs', value: '1' },
        ],
      }),
      registry,
    )

    assert.equal(failures.length, 1)
    assert.match(failures[0], /DEPLOY_MANIFEST_VERSION says more than one thing/)
    assert.match(failures[0], /packages\/@ruvyxa\/core\/src\/deploy-manifest\.ts: 2/)
    assert.match(
      failures[0],
      /packages\/ruvyxa\/runtime\/adapter-runner\.mjs: 1/,
      'the copy that drifted must appear, or the reader cannot tell which one to fix',
    )
    assert.match(failures[0], /crates\/a\.rs: 2/)
  })

  it('reports a divergent second Rust copy', () => {
    const { failures } = inspect(
      single({
        DEPLOY_MANIFEST_VERSION: [
          { file: 'crates/writer.rs', value: '1' },
          { file: 'crates/reader.rs', value: '2' },
        ],
      }),
      single({ DEPLOY_MANIFEST_VERSION: [{ file: 'packages/a.mjs', value: '1' }] }),
      registry,
    )

    assert.equal(failures.length, 1)
    assert.match(failures[0], /crates\/reader\.rs: 2/)
  })

  it('folds the two spellings of one number across every copy', () => {
    const { failures } = inspect(
      single({ DEPLOY_MANIFEST_VERSION: [{ file: 'crates/a.rs', value: '52_428_800' }] }),
      single({
        DEPLOY_MANIFEST_VERSION: [
          { file: 'packages/a.mjs', value: '50 * 1024 * 1024' },
          { file: 'packages/b.mjs', value: '52428800' },
        ],
      }),
      registry,
    )

    assert.deepEqual(failures, [])
  })

  /**
   * An initializer this cannot finish reading is not a scalar, and saying so is
   * the answer -- not a crash.
   *
   * `JS_CONST` captures an initializer to the end of its line, so a constant
   * whose arithmetic wraps across two lines arrives here as `(50 *`. That
   * satisfies the character guard (digits, spaces, `*`, `+`, parens) while being
   * syntactically incomplete, and `Function()` threw a `SyntaxError` straight out
   * of `pnpm release:validate` — a Node stack trace naming neither the constant
   * nor its file. The guard is doing its real job either way: no identifier can
   * reach `Function()`, so this was never a way to execute a source file.
   */
  it('answers null for an arithmetic initializer it cannot finish reading', () => {
    for (const partial of ['(50 *', '((1 +', '1 +', '50 * (1024', ')']) {
      let answer
      assert.doesNotThrow(
        () => {
          answer = scalar(partial)
        },
        `scalar(${JSON.stringify(partial)}) must not throw`,
      )
      assert.equal(answer, null, `scalar(${JSON.stringify(partial)})`)
    }

    // Still reads the complete spellings it exists for.
    assert.equal(scalar('50 * 1024 * 1024'), 'number:52428800')
    assert.equal(scalar('42'), 'number:42')
  })

  it('refuses a copy that has stopped being a scalar', () => {
    const { failures } = inspect(
      single({ DEPLOY_MANIFEST_VERSION: [{ file: 'crates/a.rs', value: '1' }] }),
      single({
        DEPLOY_MANIFEST_VERSION: [
          { file: 'packages/a.mjs', value: '1' },
          { file: 'packages/b.mjs', value: 'MANIFEST_VERSIONS.current' },
        ],
      }),
      registry,
    )

    assert.equal(failures.length, 1)
    assert.match(failures[0], /no longer a scalar/)
    assert.match(failures[0], /packages\/b\.mjs/)
  })
})

describe('registry bookkeeping', () => {
  it('still fails a shared name that is registered nowhere, listing every copy', () => {
    const { failures } = inspect(
      single({ UNREGISTERED_RULE: [{ file: 'crates/a.rs', value: '1' }] }),
      single({
        UNREGISTERED_RULE: [
          { file: 'packages/a.mjs', value: '1' },
          { file: 'packages/b.mjs', value: '1' },
        ],
      }),
      [],
    )

    assert.equal(failures.length, 1)
    assert.match(failures[0], /registered nowhere/)
    assert.match(failures[0], /packages\/b\.mjs/)
  })

  it('still fails a registry entry that no longer applies', () => {
    const { failures } = inspect(
      single({ GONE_FROM_JS: [{ file: 'crates/a.rs', value: '1' }] }),
      new Map(),
      [{ name: 'GONE_FROM_JS', kind: 'sameValue', why: 'stale' }],
    )

    assert.equal(failures.length, 1)
    assert.match(failures[0], /no longer declared in both languages/)
    assert.match(failures[0], /Only Rust/)
  })
})

describe('which files the gate can see', () => {
  /**
   * `git ls-files` matches a pathspec with `*` crossing `/`.
   *
   * That is why `packages/**` works and why `scripts/**\/*.mjs` — the obvious
   * spelling, and the one first reached for — matches *nothing*: the `/` after
   * `**` is literal, and every script sits one level down. A pathspec that
   * silently selects zero files is the worst shape a gate can have, so this
   * asserts on the files it reaches rather than on the pattern.
   */
  it('reaches the repository scripts, where two drifted lists sat unseen', () => {
    const tracked = trackedSources()

    assert.ok(
      tracked.includes('scripts/verify-reproducible.mjs'),
      `the pathspec ${JSON.stringify(SOURCE_PATHSPEC)} selects no repository script. ` +
        '`scripts/**/*.mjs` is the spelling that matches nothing: git crosses `/` with a plain ' +
        '`*`, so the literal slash after `**` needs a directory no script is in.',
    )
    assert.ok(tracked.some((file) => file.startsWith('crates/')))
    assert.ok(tracked.some((file) => file.startsWith('packages/')))
  })

  it('sees both halves of the telemetry pair the audit found', () => {
    const tracked = trackedSources()
    const rust = declarations(
      tracked.filter((file) => file.endsWith('.rs')),
      RUST_CONST,
      true,
    )
    const js = declarations(
      tracked.filter((file) => !file.endsWith('.rs')),
      JS_CONST,
      false,
    )

    assert.deepEqual(
      rust.get('TELEMETRY_FIELDS')?.map((copy) => copy.file),
      ['crates/ruvyxa_cli/src/bench.rs'],
    )
    assert.deepEqual(
      js.get('TELEMETRY_FIELDS')?.map((copy) => copy.file),
      ['scripts/verify-reproducible.mjs'],
    )
  })

  it('registers every name the widened pathspec brings into reach', () => {
    const tracked = trackedSources()
    const { failures } = inspect(
      declarations(
        tracked.filter((file) => file.endsWith('.rs')),
        RUST_CONST,
        true,
      ),
      declarations(
        tracked.filter((file) => !file.endsWith('.rs')),
        JS_CONST,
        false,
      ),
    )

    assert.deepEqual(failures, [])
  })

  it('gives the telemetry pair a disposition and a reason', () => {
    const entry = REGISTRY.find((candidate) => candidate.name === 'TELEMETRY_FIELDS')

    assert.ok(entry, 'TELEMETRY_FIELDS is declared in both languages and must be registered')
    assert.equal(entry.kind, 'unrelated')
    // `unrelated` is the one kind this script cannot enforce, so the reason is
    // the whole gate. It has to say why the two lists legitimately differ —
    // they normalize different files for different comparisons — not that
    // somebody looked once.
    assert.ok(entry.why.length > 200, 'an `unrelated` entry without a real reason is an excuse')
    for (const term of ['bench.rs', 'verify-reproducible.mjs', 'cache', 'createdAtUnix']) {
      assert.ok(entry.why.includes(term), `the reason must account for ${term}`)
    }
  })
})

describe('the deploy-manifest contract this gate was blind to', () => {
  it('sees the adapter runner copy the deployed build is judged by', () => {
    const found = declarations(
      [
        repoFile('packages/@ruvyxa/core/src/deploy-manifest.ts'),
        repoFile('packages/ruvyxa/runtime/adapter-runner.mjs'),
      ],
      JS_CONST,
      false,
    )

    for (const name of ['DEPLOY_MANIFEST_KEY', 'DEPLOY_MANIFEST_VERSION']) {
      assert.equal(
        found.get(name)?.length,
        2,
        `${name} is declared by both the typed reader and the executed adapter runner; ` +
          'the gate must hold every copy, because the runner is the one that rejects a deploy',
      )
    }
  })
})
