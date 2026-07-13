import { spawnSync } from 'node:child_process'

const commandOptions = {
  shell: process.platform === 'win32',
  stdio: 'inherit',
}

function run(command, args) {
  const result = spawnSync(command, args, commandOptions)

  if (result.error) {
    console.error(`Unable to run ${command}: ${result.error.message}`)
    process.exit(1)
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
}

const stagedResult = spawnSync(
  'git',
  ['diff', '--cached', '--name-only', '--diff-filter=ACMR', '-z'],
  {
    encoding: 'buffer',
    shell: process.platform === 'win32',
  },
)

if (stagedResult.error) {
  console.error(`Unable to inspect staged files: ${stagedResult.error.message}`)
  process.exit(1)
}

if (stagedResult.status !== 0) {
  process.exit(stagedResult.status ?? 1)
}

const stagedFiles = stagedResult.stdout.toString('utf8').split('\0').filter(Boolean)

const prettierFiles = stagedFiles.filter((file) => !file.endsWith('.rs') && !file.endsWith('.toml'))
const rustFiles = stagedFiles.filter((file) => file.endsWith('.rs'))

if (prettierFiles.length > 0) {
  run('pnpm', ['exec', 'prettier', '--write', '--ignore-unknown', ...prettierFiles])
  run('git', ['add', '--', ...prettierFiles])
}

if (rustFiles.length > 0) {
  run('cargo', ['fmt', '--all', '--', '--check'])
}
