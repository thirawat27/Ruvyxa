import { existsSync, readFileSync, statSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'

/** Supported package managers. */
export type PackageManager = 'pnpm' | 'yarn' | 'npm' | 'bun'

/** Commands and lockfile associated with a package manager. */
export interface PackageManagerInfo {
  name: PackageManager
  install: string
  dev: string
  exec: string
  lockfile: string
}

const PM_INFO: Record<PackageManager, Omit<PackageManagerInfo, 'name'>> = {
  pnpm: { install: 'pnpm install', dev: 'pnpm dev', exec: 'pnpm dlx', lockfile: 'pnpm-lock.yaml' },
  yarn: { install: 'yarn', dev: 'yarn dev', exec: 'yarn dlx', lockfile: 'yarn.lock' },
  bun: { install: 'bun install', dev: 'bun dev', exec: 'bunx', lockfile: 'bun.lock' },
  npm: { install: 'npm install', dev: 'npm run dev', exec: 'npx', lockfile: 'package-lock.json' },
}

const LOCKFILES: ReadonlyArray<readonly [PackageManager, string]> = [
  ['pnpm', 'pnpm-lock.yaml'],
  ['yarn', 'yarn.lock'],
  ['bun', 'bun.lock'],
  ['bun', 'bun.lockb'],
  ['npm', 'package-lock.json'],
]

const CONVENTION_FILES: ReadonlyArray<readonly [PackageManager, string]> = [
  ['pnpm', 'pnpm-workspace.yaml'],
  ['yarn', '.yarnrc.yml'],
  ['bun', 'bunfig.toml'],
]

const TIE_BREAK_ORDER: ReadonlyArray<PackageManager> = ['pnpm', 'yarn', 'bun', 'npm']

/**
 * Detect the package manager most likely intended for the project containing `cwd`.
 *
 * Evidence is considered in descending order of intent: the process that invoked the
 * generator, Corepack's `packageManager` declaration, workspace/tooling conventions,
 * then lockfiles. The search walks to the filesystem root, so an app inside a workspace
 * inherits its workspace manager without allowing an unrelated parent lockfile to win.
 * When stale lockfiles coexist, the newest one wins; an exact timestamp tie uses a stable
 * order so the result is reproducible.
 */
export function detectPackageManager(
  cwd = process.cwd(),
  environment: NodeJS.ProcessEnv = process.env,
): PackageManagerInfo {
  const invokedBy = packageManagerFromUserAgent(environment.npm_config_user_agent)
  if (invokedBy) return info(invokedBy)

  for (const directory of ancestors(cwd)) {
    const declared = packageManagerFromManifest(directory)
    if (declared) return info(declared)

    const convention = packageManagerFromConvention(directory)
    if (convention) return info(convention)

    const lockfile = packageManagerFromLockfiles(directory)
    if (lockfile) return info(lockfile)
  }

  for (const manager of TIE_BREAK_ORDER) {
    if (hasCommand(manager)) return info(manager)
  }
  return info('npm')
}

function packageManagerFromUserAgent(userAgent: string | undefined): PackageManager | undefined {
  if (!userAgent) return undefined
  return packageManagerFromIdentifier(userAgent)
}

function packageManagerFromManifest(directory: string): PackageManager | undefined {
  try {
    const packageJson = JSON.parse(readFileSync(resolve(directory, 'package.json'), 'utf8')) as {
      packageManager?: unknown
    }
    return typeof packageJson.packageManager === 'string'
      ? packageManagerFromIdentifier(packageJson.packageManager)
      : undefined
  } catch {
    return undefined
  }
}

function packageManagerFromConvention(directory: string): PackageManager | undefined {
  for (const [manager, filename] of CONVENTION_FILES) {
    if (existsSync(resolve(directory, filename))) return manager
  }
  return undefined
}

function packageManagerFromLockfiles(directory: string): PackageManager | undefined {
  const candidates = LOCKFILES.flatMap(([name, filename]) => {
    const path = resolve(directory, filename)
    if (!existsSync(path)) return []
    try {
      return [{ name, modifiedAtMs: statSync(path).mtimeMs }]
    } catch {
      return []
    }
  })
  if (candidates.length === 0) return undefined

  candidates.sort(
    (left, right) =>
      right.modifiedAtMs - left.modifiedAtMs ||
      TIE_BREAK_ORDER.indexOf(left.name) - TIE_BREAK_ORDER.indexOf(right.name),
  )
  return candidates[0].name
}

function packageManagerFromIdentifier(value: string): PackageManager | undefined {
  const normalized = value.trim().toLowerCase()
  return (['pnpm', 'yarn', 'bun', 'npm'] as const).find(
    (manager) =>
      normalized === manager ||
      normalized.startsWith(`${manager}@`) ||
      normalized.startsWith(`${manager}/`),
  )
}

function* ancestors(cwd: string): Generator<string> {
  let current = resolve(cwd)
  while (true) {
    yield current
    const parent = dirname(current)
    if (parent === current) return
    current = parent
  }
}

function info(name: PackageManager): PackageManagerInfo {
  return { name, ...PM_INFO[name] }
}

function hasCommand(command: string): boolean {
  const result = spawnSync(command, ['--version'], { stdio: 'ignore', timeout: 2000 })
  return !result.error && result.status === 0
}
