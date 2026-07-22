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

/** Converts a completed native-process result into a safe CLI exit code. */
export function exitCodeForSpawnResult({ status }) {
  if (typeof status === 'number') return status
  return 1
}
