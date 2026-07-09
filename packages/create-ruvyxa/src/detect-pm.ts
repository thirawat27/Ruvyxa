import { existsSync } from 'node:fs'
import { execSync } from 'node:child_process'
import { resolve } from 'node:path'

/**
 * Supported package managers.
 */
export type PackageManager = 'pnpm' | 'yarn' | 'npm' | 'bun'

/**
 * Result of package manager detection, including the detected manager
 * and the corresponding commands for install and dev.
 */
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
  bun: { install: 'bun install', dev: 'bun dev', exec: 'bunx', lockfile: 'bun.lockb' },
  npm: { install: 'npm install', dev: 'npm run dev', exec: 'npx', lockfile: 'package-lock.json' },
}

/**
 * Detect the user's preferred package manager.
 *
 * Detection strategy (in priority order):
 * 1. `npm_config_user_agent` environment variable (set by all package managers when running scripts)
 * 2. Lockfile in the current working directory (user might be in a monorepo)
 * 3. Binary availability on PATH (which commands are installed)
 * 4. Falls back to `npm` (always available with Node.js)
 *
 * This approach is fast (no network calls, no heavy I/O) and respects the
 * user's actual setup without assumptions.
 */
export function detectPackageManager(cwd = process.cwd()): PackageManagerInfo {
  // 1. Check npm_config_user_agent (most reliable — set by the pm that ran us)
  const userAgent = process.env.npm_config_user_agent
  if (userAgent) {
    if (userAgent.startsWith('pnpm/')) return info('pnpm')
    if (userAgent.startsWith('yarn/')) return info('yarn')
    if (userAgent.startsWith('bun/')) return info('bun')
    if (userAgent.startsWith('npm/')) return info('npm')
  }

  // 2. Check lockfiles in cwd (user may be running from an existing project)
  if (existsSync(resolve(cwd, 'pnpm-lock.yaml'))) return info('pnpm')
  if (existsSync(resolve(cwd, 'yarn.lock'))) return info('yarn')
  if (existsSync(resolve(cwd, 'bun.lockb'))) return info('bun')
  if (existsSync(resolve(cwd, 'package-lock.json'))) return info('npm')

  // 3. Check binary availability on PATH (fast sync check)
  if (hasCommand('pnpm')) return info('pnpm')
  if (hasCommand('yarn')) return info('yarn')
  if (hasCommand('bun')) return info('bun')

  // 4. Default to npm (ships with Node.js)
  return info('npm')
}

function info(name: PackageManager): PackageManagerInfo {
  return { name, ...PM_INFO[name] }
}

function hasCommand(cmd: string): boolean {
  try {
    // Use 'where' on Windows, 'which' on Unix
    const check = process.platform === 'win32' ? `where ${cmd}` : `which ${cmd}`
    execSync(check, { stdio: 'ignore', timeout: 2000 })
    return true
  } catch {
    return false
  }
}
