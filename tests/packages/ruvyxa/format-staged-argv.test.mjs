import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  COREPACK_PNPM,
  resolveCommand,
  stagedCommands,
  stagedFiles,
} from '../../../scripts/format-staged.mjs'

const scriptPath = fileURLToPath(new URL('../../../scripts/format-staged.mjs', import.meta.url))

/**
 * The two filenames this test exists for.
 *
 * Both are legal on every platform this repository builds on, both arrive from
 * `git diff --cached`, and both used to be joined into one string and handed to
 * `cmd.exe`: the first split into two paths that do not exist, and the second
 * ran `whoami` as a separate command on `git commit`.
 */
const HOSTILE = 'docs/a&whoami.md'
const SPACED = 'docs/my notes.md'

describe('the argv the pre-commit hook builds', () => {
  it('keeps a filename containing a space as one argv member', () => {
    const [prettier, add] = stagedCommands([SPACED])

    assert.deepEqual(prettier, {
      command: 'pnpm',
      args: ['exec', 'prettier', '--write', '--ignore-unknown', SPACED],
    })
    assert.deepEqual(add, { command: 'git', args: ['add', '--', SPACED] })
  })

  it('keeps a filename containing a shell operator as one argv member', () => {
    const [prettier, add] = stagedCommands([HOSTILE])

    assert.equal(
      prettier.args.at(-1),
      HOSTILE,
      'the whole name is one argument; a shell would have split it at the ampersand',
    )
    assert.equal(add.args.at(-1), HOSTILE)
    for (const argument of [...prettier.args, ...add.args]) {
      assert.equal(typeof argument, 'string')
      assert.doesNotMatch(
        argument,
        / /,
        'no argument may be a joined string; joining is exactly what put filenames through cmd.exe',
      )
    }
  })

  it('passes both hostile shapes through in one commit, in order', () => {
    const [prettier, add, ...rest] = stagedCommands([SPACED, 'src/lib.rs', HOSTILE, 'Cargo.toml'])

    assert.deepEqual(prettier.args.slice(-2), [SPACED, HOSTILE])
    assert.deepEqual(add.args, ['add', '--', SPACED, HOSTILE])
    assert.deepEqual(
      rest,
      [{ command: 'cargo', args: ['fmt', '--all', '--', '--check'] }],
      'Rust files are formatted workspace-wide and never named on an argv, and `Cargo.toml` ' +
        'reaches neither tool',
    )
  })

  it('issues nothing when nothing is staged', () => {
    assert.deepEqual(stagedCommands([]), [])
  })

  it('splits the NUL-separated list `git -z` writes, keeping every byte of a name', () => {
    const stdout = Buffer.from(`${SPACED}\0${HOSTILE}\0`, 'utf8')

    assert.deepEqual(stagedFiles(stdout), [SPACED, HOSTILE])
  })
})

describe('resolving a command without a shell', () => {
  const pnpm = { command: 'pnpm', args: ['exec', 'prettier', '--write', SPACED] }

  it('runs Windows pnpm as Corepack’s .js entry under this same Node', () => {
    const resolved = resolveCommand(pnpm, { platform: 'win32', pnpmEntry: scriptPath })

    assert.equal(resolved.file, process.execPath)
    assert.deepEqual(resolved.args, [scriptPath, 'exec', 'prettier', '--write', SPACED])
  })

  it('leaves pnpm alone everywhere else', () => {
    const resolved = resolveCommand(pnpm, { platform: 'linux' })

    assert.equal(resolved.file, 'pnpm')
    assert.deepEqual(resolved.args, pnpm.args)
  })

  it('leaves git and cargo alone on Windows, because they are .exe', () => {
    for (const command of ['git', 'cargo']) {
      const resolved = resolveCommand({ command, args: ['--version'] }, { platform: 'win32' })
      assert.equal(resolved.file, command)
    }
  })

  it('says so when a standalone pnpm install has no Corepack entry', () => {
    assert.throws(
      () =>
        resolveCommand(pnpm, {
          platform: 'win32',
          pnpmEntry: `${scriptPath}.does-not-exist`,
        }),
      /missing Corepack pnpm/,
      'falling back silently would mean falling back to the shell this replaced',
    )
  })

  it('points at a real file on the host running this test, when that host is Windows', () => {
    if (process.platform !== 'win32') return
    const resolved = resolveCommand(pnpm)
    assert.equal(resolved.file, process.execPath)
    assert.equal(resolved.args[0], COREPACK_PNPM)
  })
})

describe('the hook itself', () => {
  it('spawns nothing through a command shell', () => {
    const source = readFileSync(scriptPath, 'utf8')

    assert.doesNotMatch(
      source,
      /^\s*shell:/m,
      'a `shell` option here re-flattens every argv into a string for cmd.exe; the PATHEXT ' +
        'problem it used to solve is solved by resolveCommand instead',
    )
  })

  it('does nothing when imported, so the index is only ever touched on purpose', () => {
    const source = readFileSync(scriptPath, 'utf8')

    assert.match(source, /import\.meta\.filename\)\s*\{\s*\n\s*main\(\)/)
  })
})
