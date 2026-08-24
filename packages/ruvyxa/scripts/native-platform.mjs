import { arch, platform } from 'node:process'

export const supportedPlatforms = {
  'darwin-arm64': { os: 'darwin', cpu: 'arm64', executable: 'ruvyxa' },
  'linux-arm64': {
    os: 'linux',
    cpu: 'arm64',
    executable: 'ruvyxa',
    rustTarget: 'aarch64-unknown-linux-musl',
  },
  'linux-x64': {
    os: 'linux',
    cpu: 'x64',
    executable: 'ruvyxa',
    rustTarget: 'x86_64-unknown-linux-musl',
  },
  'win32-arm64': { os: 'win32', cpu: 'arm64', executable: 'ruvyxa.exe' },
  'win32-x64': { os: 'win32', cpu: 'x64', executable: 'ruvyxa.exe' },
}

export function currentPlatformKey() {
  return `${platform}-${arch}`
}

export function currentPlatform() {
  const key = currentPlatformKey()
  const target = supportedPlatforms[key]
  if (!target) {
    throw new Error(`Unsupported Ruvyxa CLI platform: ${key}`)
  }
  return { key, ...target }
}

export function nativeBinaryPackageName(platformKey) {
  return supportedPlatforms[platformKey] ? `@ruvyxa/cli-${platformKey}` : null
}

/**
 * Refuse a native binary whose version does not match the `ruvyxa` package
 * running it, and say why.
 *
 * The two halves are not independent. The Rust CLI resolves `runtime/*.mjs`
 * **by path** out of the installed `ruvyxa` package and spawns or imports them,
 * and the contracts between the two — the module graphs, the entry templates,
 * the conformance fixtures under `tests/fixtures/` — hold only within one
 * version. A binary from another release loads this release's JavaScript and
 * neither side can tell.
 *
 * That combination is reachable from the registry. `optionalDependencies` used
 * to carry `workspace:^`, which publishes as `^1.0.31` and matches `1.1.0`, so
 * a half-finished release that published the platform packages ahead of the
 * framework handed the previous version's users a newer bundler on their next
 * clean install — including a build refusal that release had introduced. The
 * pin is exact now; this is the check that notices when something arrives at
 * the wrong pairing anyway.
 *
 * Returns `null` when the versions agree, or when either is unreadable: a
 * guess about which one is wrong is worth less than saying nothing.
 *
 * @returns {string|null} the message to print before refusing to run
 */
export function nativeBinaryVersionError(runtimeVersion, binaryVersion, packageName) {
  if (!runtimeVersion || !binaryVersion) return null
  if (runtimeVersion === binaryVersion) return null

  return [
    `Ruvyxa CLI version mismatch: ${packageName}@${binaryVersion} cannot run ruvyxa@${runtimeVersion}.`,
    'The CLI binary loads the runtime modules that ship in this package, by path, and',
    'the two are only held to the same contracts within one release.',
    `Install ${packageName}@${runtimeVersion}, or reinstall ruvyxa so the pair is resolved together.`,
  ].join('\n')
}

/** Converts a completed native-process result into a safe CLI exit code. */
export function exitCodeForSpawnResult({ status }) {
  if (typeof status === 'number') return status
  return 1
}
