import { arch, platform } from 'node:process'

export const supportedPlatforms = {
  'darwin-arm64': { os: 'darwin', cpu: 'arm64', executable: 'ruvyxa' },
  'darwin-x64': { os: 'darwin', cpu: 'x64', executable: 'ruvyxa' },
  'linux-arm64': { os: 'linux', cpu: 'arm64', executable: 'ruvyxa' },
  'linux-x64': { os: 'linux', cpu: 'x64', executable: 'ruvyxa' },
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
