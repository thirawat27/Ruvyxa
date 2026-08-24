#!/usr/bin/env node
import { spawnSync } from 'node:child_process'
import { chmodSync, existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  currentPlatformKey,
  exitCodeForSpawnResult,
  nativeBinaryPackageName,
  nativeBinaryVersionError,
} from '../scripts/native-platform.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const packageRoot = resolve(here, '..')
const monorepoRoot = resolve(here, '../../..')
const executable = process.platform === 'win32' ? 'ruvyxa.exe' : 'ruvyxa'
const platformKey = currentPlatformKey()

const binary = findBinary()
const invokerRuntime = detectInvokerRuntime()

/** Which JavaScript runtime is running this shim, so the CLI can report it back. */
function detectInvokerRuntime() {
  if (globalThis.Bun) return 'bun'
  if (globalThis.Deno) return 'deno'
  return 'node'
}

if (!binary) {
  console.error(`Ruvyxa CLI binary was not found for ${platformKey}.`)
  if (optionalBinaryPackageName()) {
    console.error('Reinstall ruvyxa, or install the matching @ruvyxa/cli-* optional package.')
  } else {
    console.error(
      'Prebuilt binaries support darwin-arm64, linux-arm64, linux-x64, win32-arm64, and win32-x64.',
    )
  }
  console.error('When working from source, run `cargo build -p ruvyxa_cli` first.')
  process.exit(1)
}

const result = spawnSync(binary, process.argv.slice(2), {
  cwd: process.cwd(),
  stdio: 'inherit',
  env: { ...process.env, RUVYXA_INVOKER_RUNTIME: invokerRuntime },
})

if (result.error) {
  console.error(`Failed to run Ruvyxa CLI at ${binary}: ${result.error.message}`)
  process.exit(1)
}

process.exit(exitCodeForSpawnResult(result))

function findBinary() {
  const sourceBinary = findSourceBinary()
  if (sourceBinary) return sourceBinary

  const bundled = resolve(packageRoot, 'native-bin', platformKey, executable)
  if (existsSync(bundled)) return prepareExecutable(bundled)

  const optionalPackage = optionalBinaryPackageName()
  if (optionalPackage) {
    const optionalRoot = resolveOptionalPackageRoot(optionalPackage)
    if (optionalRoot) {
      const optionalBinary = join(optionalRoot, 'bin', executable)
      if (existsSync(optionalBinary)) {
        // Outside the resolution `try` on purpose: a catch wide enough to
        // cover this refusal would turn it into "binary not found", which is
        // a different problem with a different fix.
        refuseOnVersionMismatch(optionalRoot, optionalPackage)
        return prepareExecutable(optionalBinary)
      }
    }
  }

  return null
}

/** Directory of the installed platform package, or null when it is absent. */
function resolveOptionalPackageRoot(optionalPackage) {
  try {
    return dirname(fileURLToPath(import.meta.resolve(`${optionalPackage}/package.json`)))
  } catch {
    // Absent on an unsupported platform; `findBinary` reports that itself.
    return null
  }
}

/**
 * Stop when the platform package and this one are from different releases.
 *
 * Only this path can drift. A binary found in `target/` or in `native-bin/`
 * was produced from this package's own version; one resolved through
 * `optionalDependencies` is whatever the registry and the lockfile agreed on.
 */
function refuseOnVersionMismatch(optionalRoot, optionalPackage) {
  const message = nativeBinaryVersionError(
    readPackageVersion(packageRoot),
    readPackageVersion(optionalRoot),
    optionalPackage,
  )
  if (!message) return
  console.error(message)
  process.exit(1)
}

function readPackageVersion(root) {
  try {
    return JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).version ?? null
  } catch {
    // Unreadable: `nativeBinaryVersionError` declines to guess.
    return null
  }
}

function findSourceBinary() {
  if (!existsSync(resolve(monorepoRoot, 'Cargo.toml'))) {
    return null
  }

  for (const profile of ['debug', 'release']) {
    const sourceBinary = resolve(monorepoRoot, 'target', profile, executable)
    if (existsSync(sourceBinary)) return prepareExecutable(sourceBinary)
  }

  return null
}

function prepareExecutable(binary) {
  if (process.platform !== 'win32') {
    chmodSync(binary, 0o755)
  }

  return binary
}

function optionalBinaryPackageName() {
  return nativeBinaryPackageName(platformKey)
}
