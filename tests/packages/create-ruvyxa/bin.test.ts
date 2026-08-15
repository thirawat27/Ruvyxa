/**
 * The scaffolder binary, exercised on a terminal that animates.
 *
 * The colour branch is the only one that redraws, and it is unreachable through
 * a pipe: without a TTY the banner is printed once and the code that builds the
 * redrawable region never runs. That is how a missing `createFrame` import
 * shipped — every non-interactive check took the print-once path, the scaffold
 * succeeded, and the failure only appeared for a user on a real terminal.
 *
 * So this runs the real bin in a child process with `process.stdout` made to
 * look like a terminal, and asserts on what that child wrote.
 */

import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { promisify } from 'node:util'

import { repoPath } from '../../repo-root.ts'

const run = promisify(execFile)

const binPath = repoPath('packages/create-ruvyxa/bin/create-ruvyxa.js')

/**
 * Run the bin with a faked TTY, returning everything it wrote to stdout.
 *
 * The bootstrap goes to a file rather than `node -e` so the escaping is one
 * layer deep, and the captured output goes to a file rather than back through
 * stderr so no framing has to be parsed out of it.
 */
function childEnvironment(noColor: boolean): NodeJS.ProcessEnv {
  const environment = { ...process.env }
  delete environment.NO_COLOR
  if (noColor) environment.NO_COLOR = '1'
  return environment
}

async function runAnimated(target: string, cwd: string, noColor = false): Promise<string> {
  const capturePath = join(cwd, 'captured-stdout.txt')
  const bootstrapPath = join(cwd, 'bootstrap.mjs')

  await writeFile(
    bootstrapPath,
    `import { writeFileSync } from 'node:fs'

for (const [key, value] of [['isTTY', true], ['columns', 120], ['rows', 40]]) {
  Object.defineProperty(process.stdout, key, { value, configurable: true })
}

const written = []
process.stdout.write = (chunk) => {
  written.push(String(chunk))
  return true
}
process.on('exit', () => {
  writeFileSync(${JSON.stringify(capturePath)}, written.join(''), 'utf8')
})

process.argv = [process.argv[0], 'create-ruvyxa', ${JSON.stringify(target)}, '--template', 'minimal']
await import(${JSON.stringify(pathToFileURL(binPath).href)})
`,
    'utf8',
  )

  // `execFile` rejects on a non-zero exit, so a `createFrame is not defined`
  // surfaces here as a rejection rather than as a silently odd assertion.
  await run(process.execPath, [bootstrapPath], { cwd, env: childEnvironment(noColor) })
  return readFile(capturePath, 'utf8')
}

describe('the scaffolder binary on an animating terminal', () => {
  it('scaffolds, and redraws its banner with relative cursor movement', async (t) => {
    const cwd = await mkdtemp(join(tmpdir(), 'create-ruvyxa-tty-'))
    t.after(() => rm(cwd, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

    const output = await runAnimated('animated-app', cwd)

    assert.ok((await readdir(cwd)).includes('animated-app'), 'the project should exist')
    assert.doesNotMatch(output, /is not defined/)

    // The bug: absolute save/restore names a screen row, and scrolling makes
    // that row hold something else, so every frame appended a fresh banner.
    assert.doesNotMatch(
      output,
      /\[[su]/,
      'absolute save/restore is what stacked the banner; it must not come back',
    )
    assert.match(
      output,
      /\[\d+A/,
      'the banner should rewind by moving the cursor up, which survives scrolling',
    )

    // Every rewind should walk back over the same banner height. A varying
    // count would mean the frame and the rewind disagree about how tall it is.
    const rewinds = [...output.matchAll(/\[(\d+)A/g)].map((match) => match[1])
    assert.ok(rewinds.length > 0)
    assert.equal(new Set(rewinds).size, 1, `rewinds disagreed on the banner height: ${rewinds}`)

    // The cursor is hidden while animating and must be given back.
    assert.match(output, /\[\?25l/)
    assert.match(output, /\[\?25h/)
  })

  it('respects NO_COLOR even when stdout is a terminal', async (t) => {
    const cwd = await mkdtemp(join(tmpdir(), 'create-ruvyxa-no-color-'))
    t.after(() => rm(cwd, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

    const output = await runAnimated('plain-app', cwd, true)

    assert.ok((await readdir(cwd)).includes('plain-app'), 'the project should exist')
    assert.match(output, /Created plain-app/)
    assert.doesNotMatch(output, /\[/, 'NO_COLOR must suppress terminal escape sequences')
  })

  it('prints the completion line when there is no terminal to animate on', async (t) => {
    // A piped or redirected run draws once. It still has to say that the
    // scaffold finished — returning a no-op there reported only that it began.
    const cwd = await mkdtemp(join(tmpdir(), 'create-ruvyxa-piped-'))
    t.after(() => rm(cwd, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

    const { stdout } = await run(
      process.execPath,
      [binPath, 'piped-app', '--template', 'minimal'],
      {
        cwd,
      },
    )

    assert.match(stdout, /Scaffolding piped-app/)
    assert.match(stdout, /Created piped-app/)
    assert.doesNotMatch(stdout, /\[/, 'no escape sequences belong in piped output')
  })
})
