import { cp, mkdir, readdir, readFile, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'

const [projectRootArg, outputDirArg] = process.argv.slice(2)

if (!projectRootArg || !outputDirArg) {
  writeResponse(
    failure('RUV2200', 'Adapter runner requires project root and build output arguments.'),
  )
  process.exit(1)
}

const projectRoot = path.resolve(projectRootArg)
const outputDir = path.resolve(outputDirArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const config = await loadConfig(projectRoot)
  const adapter = config.adapter
  if (adapter === undefined) {
    writeResponse(success([]))
  } else if (!adapter || typeof adapter !== 'object' || typeof adapter.build !== 'function') {
    writeResponse(failure('RUV2200', 'config.adapter must provide a build(context) function.'))
    process.exitCode = 1
  } else {
    const output = await adapter.build({ root: projectRoot, outDir: outputDir })
    const artifacts = await materializeArtifacts(output, outputDir)
    writeResponse(success(artifacts))
  }
} catch (error) {
  writeResponse(
    failure('RUV2200', error instanceof Error ? error.message : String(error), error?.stack),
  )
  process.exitCode = 1
}

async function loadConfig(root) {
  const configFile = findConfig(root)
  if (!configFile) return {}

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'adapter-runner'], 'mjs'),
  )

  await compileBundle({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:adapter-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  return mod.default ?? {}
}

function findConfig(root) {
  for (const fileName of [
    'ruvyxa.config.ts',
    'ruvyxa.config.mts',
    'ruvyxa.config.js',
    'ruvyxa.config.mjs',
  ]) {
    const file = path.join(root, fileName)
    if (existsSync(file)) return file
  }
  return null
}

async function materializeArtifacts(output, buildDir) {
  if (!output || typeof output !== 'object') {
    throw new Error('RUV2200 config.adapter.build(context) must return an output object.')
  }
  if (!Array.isArray(output.artifacts)) return []

  const artifacts = []
  for (const artifact of output.artifacts) {
    if (!artifact || typeof artifact !== 'object') {
      throw new Error('RUV2200 adapter artifact must be an object.')
    }
    const destination = artifactDestination(buildDir, artifact.path)
    if (artifact.kind === 'file') {
      if (typeof artifact.contents !== 'string') {
        throw new Error(`RUV2200 file artifact ${artifact.path} must include string contents.`)
      }
      await mkdir(path.dirname(destination), { recursive: true })
      await writeFile(destination, artifact.contents, 'utf8')
      artifacts.push({ kind: 'file', path: artifact.path })
      continue
    }
    if (artifact.kind === 'static-site') {
      await materializeStaticSite(buildDir, destination)
      artifacts.push({ kind: 'static-site', path: artifact.path })
      continue
    }
    throw new Error(`RUV2200 unsupported adapter artifact kind: ${String(artifact.kind)}.`)
  }
  return artifacts
}

function artifactDestination(buildDir, artifactPath) {
  if (typeof artifactPath !== 'string' || artifactPath.trim() === '') {
    throw new Error('RUV2200 adapter artifact path must be a non-empty relative path.')
  }
  const destination = path.resolve(buildDir, artifactPath)
  if (destination === buildDir || !destination.startsWith(buildDir + path.sep)) {
    throw new Error(`RUV2200 adapter artifact path escapes the build output: ${artifactPath}.`)
  }
  const topLevel = path.relative(buildDir, destination).split(path.sep)[0]
  if (
    ['assets', 'build.json', 'cache', 'client', 'manifest.json', 'prerender', 'server'].includes(
      topLevel,
    )
  ) {
    throw new Error(
      `RUV2200 adapter artifact path overlaps protected build output: ${artifactPath}. Use a directory such as deploy/<platform> or static.`,
    )
  }
  return destination
}

async function materializeStaticSite(buildDir, destination) {
  const manifestPath = path.join(buildDir, 'manifest.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const unsupported = (manifest.routes ?? []).filter((route) => {
    if (route.kind === 'api') return true
    return !['ssg', 'csr'].includes(route.render?.strategy)
  })
  if (unsupported.length > 0) {
    const routes = unsupported.map((route) => route.path).join(', ')
    throw new Error(
      `RUV2202 static adapter output requires SSG or CSR pages and no API routes; unsupported: ${routes}.`,
    )
  }

  const prerenderDir = path.join(buildDir, 'prerender')
  if (!existsSync(prerenderDir)) {
    throw new Error('RUV2202 static adapter output requires generated prerendered pages.')
  }

  await mkdir(destination, { recursive: true })
  await copyDirectoryContents(path.join(buildDir, 'assets'), path.join(destination, 'assets'))
  await copyDirectoryContents(path.join(buildDir, 'client'), path.join(destination, 'client'))
  await copyDirectoryContents(prerenderDir, destination, new Set(['manifest.json']))
}

async function copyDirectoryContents(source, destination, excluded = new Set()) {
  if (!existsSync(source)) return
  await mkdir(destination, { recursive: true })
  for (const entry of await readdir(source, { withFileTypes: true })) {
    if (excluded.has(entry.name)) continue
    await cp(path.join(source, entry.name), path.join(destination, entry.name), { recursive: true })
  }
}

function success(result) {
  return { ok: true, result }
}

function failure(code, message, stack) {
  return { ok: false, code, message, stack }
}

function writeResponse(response) {
  process.stdout.write(JSON.stringify(response))
}
