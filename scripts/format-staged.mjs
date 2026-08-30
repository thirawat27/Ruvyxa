#!/usr/bin/env node
// The pre-commit hook. Every argument it passes on is a filename somebody else
// chose, and on Windows it used to hand those filenames to `cmd.exe`.
//
// `spawnSync(command, args, { shell: true })` does not keep `args` an array.
// Node joins it with single spaces, sets `windowsVerbatimArguments`, and gives
// `cmd.exe /d /s /c` the joined string — no quoting, no escaping. A staged file
// named `a&whoami.md` therefore ran `whoami` on `git commit`, and the mundane
// half of the same defect — a path containing a space — broke the hook for
// everyone on Windows.
//
// `shell: true` was there to solve a *command resolution* problem: `pnpm` on
// Windows is `pnpm.cmd`, and Node refuses to spawn a `.cmd` without a shell.
// That is a different problem from argument passing, and solving it with the
// shell put every argument through `cmd.exe`'s parser to fix the name of the
// program. `pack-smoke.mjs` and `test-package.mjs` already solve it the right
// way, by spawning `process.execPath` against Corepack's own `pnpm.js`, which
// is a plain `.js` file and needs no shell at all. `git` and `cargo` are
// `.exe` and libuv finds them without one.
//
// So no command here runs through a shell, and the argv stays an array from
// `git diff` to the process that receives it.
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import path from 'node:path'

/** Corepack's pnpm entry: the one pnpm on Windows that is not a `.cmd` shim. */
export const COREPACK_PNPM = path.resolve(
  path.dirname(process.execPath),
  'node_modules/corepack/dist/pnpm.js',
)

/**
 * The staged paths `git` reported, NUL separated.
 *
 * `-z` is what makes this safe to split: with it `git` writes the bytes of the
 * path verbatim, so a name containing a space, a quote, or a newline arrives
 * whole rather than quoted for a shell that is no longer there.
 */
export function stagedFiles(stdout) {
  return stdout.toString('utf8').split('\0').filter(Boolean)
}

/**
 * The commands one run of the hook issues, as `{ command, args }` pairs.
 *
 * Built apart from running them so a test can look at the argv. That is not
 * ceremony: the defect this replaces was invisible in every argv and appeared
 * only once the array had been flattened into a string by somebody else.
 */
export function stagedCommands(files) {
  const prettierFiles = files.filter((file) => !file.endsWith('.rs') && !file.endsWith('.toml'))
  const rustFiles = files.filter((file) => file.endsWith('.rs'))
  const commands = []

  if (prettierFiles.length > 0) {
    commands.push({
      command: 'pnpm',
      args: ['exec', 'prettier', '--write', '--ignore-unknown', ...prettierFiles],
    })
    // `--` so a path that begins with a dash is a path and not a flag; the
    // shell was never what protected this one.
    commands.push({ command: 'git', args: ['add', '--', ...prettierFiles] })
  }
  if (rustFiles.length > 0) {
    commands.push({ command: 'cargo', args: ['fmt', '--all', '--', '--check'] })
  }
  return commands
}

/**
 * The executable and argv a logical command becomes on this platform.
 *
 * Only `pnpm` on Windows needs anything: it is `pnpm.cmd`, which Node has
 * refused to spawn without a shell since the argument-injection fix in Node 18,
 * so it is run as Corepack's `pnpm.js` under this same Node. A standalone
 * (non-Corepack) pnpm install has no such file, and that is said out loud
 * rather than falling back to something that would reintroduce the shell.
 */
export function resolveCommand(
  { command, args },
  { platform = process.platform, pnpmEntry = COREPACK_PNPM } = {},
) {
  if (command !== 'pnpm' || platform !== 'win32') return { file: command, args }
  if (!existsSync(pnpmEntry)) {
    throw new Error(
      `Node installation is missing Corepack pnpm: ${pnpmEntry}. ` +
        'The pre-commit hook runs pnpm through it so no filename reaches a command shell.',
    )
  }
  return { file: process.execPath, args: [pnpmEntry, ...args] }
}

function run(logical) {
  let resolved
  try {
    resolved = resolveCommand(logical)
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }

  const result = spawnSync(resolved.file, resolved.args, { stdio: 'inherit' })

  if (result.error) {
    console.error(`Unable to run ${logical.command}: ${result.error.message}`)
    process.exit(1)
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
}

function main() {
  const staged = spawnSync('git', ['diff', '--cached', '--name-only', '--diff-filter=ACMR', '-z'], {
    encoding: 'buffer',
  })

  if (staged.error) {
    console.error(`Unable to inspect staged files: ${staged.error.message}`)
    process.exit(1)
  }

  if (staged.status !== 0) {
    process.exit(staged.status ?? 1)
  }

  for (const command of stagedCommands(stagedFiles(staged.stdout))) run(command)
}

// Running the file formats the staged set; importing it hands the argv builder
// to a test. Nothing here may run on import — the hook's job is to modify the
// index, and a module that did that when required would be the worse defect.
if (process.argv[1] !== undefined && path.resolve(process.argv[1]) === import.meta.filename) {
  main()
}
