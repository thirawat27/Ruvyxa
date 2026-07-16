import { spawnSync } from 'node:child_process'
import process from 'node:process'

const command = (name, args) => {
  console.log(`\n[Ruvyxa] ${name}`)
  const result = spawnSync(args[0], args.slice(1), { stdio: 'inherit', shell: true })
  if (result.error) {
    console.error(`[ERROR] ${result.error.message}`)
    process.exit(1)
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
}

command('Installing workspace dependencies...', ['pnpm', 'install', '--frozen-lockfile'])
command('Building workspace packages...', ['pnpm', '-r', 'build'])
command('Compiling the Ruvyxa CLI...', ['cargo', 'build', '--locked', '-p', 'ruvyxa_cli'])

console.log('\nSetup complete. Start developing with:')
console.log('  cd examples/demo')
console.log('  pnpm dev')
