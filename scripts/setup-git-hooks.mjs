import { chmodSync, existsSync } from 'node:fs'
import { resolve } from 'node:path'
import { spawnSync } from 'node:child_process'

const commandOptions = {
  encoding: 'utf8',
  shell: process.platform === 'win32',
}

function runGit(args) {
  const result = spawnSync('git', args, commandOptions)

  if (result.error || result.status !== 0) {
    return { ok: false }
  }

  return { ok: true, output: result.stdout.trim() }
}

const repositoryRootResult = runGit(['rev-parse', '--show-toplevel'])

if (!repositoryRootResult.ok) {
  process.exit(0)
}

const repositoryRoot = repositoryRootResult.output
const hookPath = resolve(repositoryRoot, '.githooks', 'pre-commit')

if (!existsSync(hookPath)) {
  console.error(`Git hook is missing: ${hookPath}`)
  process.exit(1)
}

chmodSync(hookPath, 0o755)

if (!runGit(['config', '--local', 'core.hooksPath', '.githooks']).ok) {
  console.error('Unable to configure the repository Git hooks path.')
  process.exit(1)
}
