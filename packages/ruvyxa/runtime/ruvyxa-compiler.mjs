import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { chmodSync, existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { currentPlatformKey, nativeBinaryPackageName } from '../scripts/native-platform.mjs'

/** Compile a virtual entry through the native Ruvyxa bundler. */
export async function compileBundle(options) {
  return (await compileBundleWithMetadata(options)).outfile
}

/** Compile a virtual entry and return the native dependency fingerprint. */
export async function compileBundleWithMetadata({
  projectRoot,
  entrySource,
  sourcefile = 'ruvyxa:entry.ts',
  outfile,
  platform = 'node',
  external = [],
  aliases = {},
  minify = false,
  sourceMap = true,
}) {
  const root = path.resolve(projectRoot)
  const request = {
    project_root: root,
    entry_source: entrySource,
    sourcefile,
    outfile: path.resolve(outfile),
    target: platform === 'browser' ? 'client' : 'ssr',
    options: {
      minify,
      source_map: sourceMap,
      tree_shaking: false,
      jsx_runtime: 'classic',
      es_target: 'es2022',
      split_strategy: 'single',
      emit_chunk_manifest: false,
      collect_module_manifest: false,
    },
    aliases: Object.fromEntries(
      Object.entries(aliases).map(([specifier, file]) => [specifier, path.resolve(file)]),
    ),
    external,
  }

  const binary = findBinary()
  if (!binary) {
    throw new Error(`Ruvyxa native CLI binary was not found for ${currentPlatformKey()}.`)
  }

  try {
    const stdout = await runNativeCompiler(binary, root, JSON.stringify(request))
    return JSON.parse(stdout)
  } catch (error) {
    const detail = error?.stderr || error?.message || String(error)
    throw new Error(`Native runtime compilation failed: ${detail}`)
  }
}

function runNativeCompiler(binary, cwd, input) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, ['compile-runtime'], {
      cwd,
      windowsHide: true,
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
      if (stdout.length > 16 * 1024 * 1024) child.kill()
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.once('error', reject)
    child.once('close', (code) => {
      if (code === 0) resolve(stdout)
      else reject(new Error(stderr || `native compiler exited with code ${code}`))
    })
    child.stdin.end(input)
  })
}

export function toImportPath(file) {
  return path.resolve(file).replaceAll('\\', '/')
}

export function cacheFileName(parts, extension) {
  const hash = createHash('sha256')
  for (const part of parts) {
    hash.update(String(part))
    hash.update('\0')
  }
  return `${hash.digest('hex').slice(0, 16)}.${extension}`
}

export function runtimeAliases(runtimeDir = path.dirname(fileURLToPath(import.meta.url))) {
  const packageRoot = path.resolve(runtimeDir, '..')
  const workspaceRoot = path.resolve(packageRoot, '..')
  const coreRoot = path.join(workspaceRoot, '@ruvyxa', 'core')

  return {
    ruvyxa: preferExisting(
      path.join(packageRoot, 'src', 'index.ts'),
      path.join(packageRoot, 'dist', 'index.js'),
    ),
    'ruvyxa/server': preferExisting(
      path.join(packageRoot, 'src', 'server.ts'),
      path.join(packageRoot, 'dist', 'server.js'),
    ),
    'ruvyxa/config': preferExisting(
      path.join(packageRoot, 'src', 'config.ts'),
      path.join(packageRoot, 'dist', 'config.js'),
    ),
    '@ruvyxa/core': preferExisting(
      path.join(coreRoot, 'src', 'index.ts'),
      path.join(coreRoot, 'dist', 'index.js'),
    ),
    '@ruvyxa/core/server': preferExisting(
      path.join(coreRoot, 'src', 'server.ts'),
      path.join(coreRoot, 'dist', 'server.js'),
    ),
    '@ruvyxa/core/config': preferExisting(
      path.join(coreRoot, 'src', 'config.ts'),
      path.join(coreRoot, 'dist', 'config.js'),
    ),
  }
}

// The native compiler owns derivation caches. Keep these exports temporarily
// so existing integrations can migrate without importing the removed compiler.
export function invalidateCompilerCache() {}
export function clearCompilerCache() {}
export function compilerCacheStats() {
  return { sources: 0, rewrites: 0, content: 0 }
}

function preferExisting(...files) {
  return files.find((file) => existsSync(file)) ?? files[files.length - 1]
}

function findBinary() {
  const here = path.dirname(fileURLToPath(import.meta.url))
  const packageRoot = path.resolve(here, '..')
  const monorepoRoot = path.resolve(here, '../../..')
  const executable = process.platform === 'win32' ? 'ruvyxa.exe' : 'ruvyxa'
  const platformKey = currentPlatformKey()

  for (const profile of ['debug', 'release']) {
    const sourceBinary = path.resolve(monorepoRoot, 'target', profile, executable)
    if (existsSync(sourceBinary)) return prepareExecutable(sourceBinary)
  }

  const bundled = path.resolve(packageRoot, 'native-bin', platformKey, executable)
  if (existsSync(bundled)) return prepareExecutable(bundled)

  const optionalPackage = nativeBinaryPackageName(platformKey)
  if (optionalPackage) {
    try {
      const packageJson = import.meta.resolve(`${optionalPackage}/package.json`)
      const optionalRoot = path.dirname(fileURLToPath(packageJson))
      const optionalBinary = path.join(optionalRoot, 'bin', executable)
      if (existsSync(optionalBinary)) return prepareExecutable(optionalBinary)
    } catch {
      // The optional platform package is absent.
    }
  }

  return null
}

function prepareExecutable(binary) {
  if (process.platform !== 'win32') chmodSync(binary, 0o755)
  return binary
}
