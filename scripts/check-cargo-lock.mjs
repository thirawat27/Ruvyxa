import { spawnSync } from 'node:child_process'

const result = spawnSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1'], {
  stdio: ['ignore', 'ignore', 'inherit'],
  shell: process.platform === 'win32',
})

if (result.error) {
  console.error(`Unable to run cargo: ${result.error.message}`)
  process.exit(1)
}

if (result.status !== 0) {
  console.error('Cargo.lock is out of date. Run: cargo generate-lockfile')
  process.exit(result.status ?? 1)
}

console.log('Cargo.lock is synchronized with the workspace manifests.')
