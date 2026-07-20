import { cp, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'

const [projectRootArg, outputDirArg, adapterNameArg] = process.argv.slice(2)

if (!projectRootArg || !outputDirArg) {
  writeResponse(
    failure('RUV2200', 'Adapter runner requires project root and build output arguments.'),
  )
  process.exit(1)
}

const projectRoot = path.resolve(projectRootArg)
const outputDir = path.resolve(outputDirArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))
const KNOWN_ADAPTER_NAMES = ['node', 'bun', 'static', 'vercel', 'netlify', 'cloudflare']
// Hosting platforms discover deployment output at fixed project-root
// locations. Project-scope artifacts are limited to this allowlist so an
// adapter can enable zero-config deploys without gaining arbitrary write
// access to the project.
const PROJECT_ARTIFACT_ALLOWLIST = [
  '.vercel/output',
  'netlify.toml',
  'wrangler.jsonc',
  '_headers',
  '_redirects',
]

try {
  // A named adapter from `ruvyxa build --adapter <name>` overrides the config
  // so a deploy target can be selected without editing ruvyxa.config.
  const adapter = adapterNameArg
    ? await loadNamedAdapter(projectRoot, adapterNameArg)
    : (await loadConfig(projectRoot)).adapter
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

async function loadNamedAdapter(root, name) {
  if (!KNOWN_ADAPTER_NAMES.includes(name)) {
    throw new Error(
      `RUV2203 unknown adapter name: ${name}. Expected one of ${KNOWN_ADAPTER_NAMES.join(', ')}.`,
    )
  }
  const packageName = `@ruvyxa/adapter-${name}`
  const requireFromProject = createRequire(path.join(root, 'package.json'))
  let entry
  try {
    entry = requireFromProject.resolve(packageName)
  } catch {
    throw new Error(
      `RUV2203 adapter package ${packageName} is not installed. Add it with your package manager, for example: pnpm add -D ${packageName}`,
    )
  }
  const mod = await import(pathToFileURL(entry).href)
  const factory = mod.default
  if (typeof factory !== 'function') {
    throw new Error(`RUV2203 ${packageName} does not export an adapter factory.`)
  }
  return factory()
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
    const scope = artifact.scope ?? 'build'
    if (scope !== 'build' && scope !== 'project') {
      throw new Error(`RUV2200 unsupported adapter artifact scope: ${String(artifact.scope)}.`)
    }
    const destination =
      scope === 'project'
        ? projectArtifactDestination(artifact.path)
        : artifactDestination(buildDir, artifact.path)
    if (artifact.kind === 'file') {
      if (typeof artifact.contents !== 'string') {
        throw new Error(`RUV2200 file artifact ${artifact.path} must include string contents.`)
      }
      if (scope === 'project' && artifact.skipIfExists === true && existsSync(destination)) {
        artifacts.push({ kind: 'file', path: artifact.path, scope, skipped: true })
        continue
      }
      await mkdir(path.dirname(destination), { recursive: true })
      await writeFile(destination, artifact.contents, 'utf8')
      artifacts.push(
        scope === 'project'
          ? { kind: 'file', path: artifact.path, scope }
          : { kind: 'file', path: artifact.path },
      )
      continue
    }
    if (artifact.kind === 'static-site') {
      // Project-scope publish directories are replaced wholesale so hashed
      // bundles from previous builds do not accumulate at the platform root.
      if (scope === 'project') await rm(destination, { recursive: true, force: true })
      await materializeStaticSite(buildDir, destination)
      artifacts.push(
        scope === 'project'
          ? { kind: 'static-site', path: artifact.path, scope }
          : { kind: 'static-site', path: artifact.path },
      )
      continue
    }
    throw new Error(`RUV2200 unsupported adapter artifact kind: ${String(artifact.kind)}.`)
  }
  return artifacts
}

function projectArtifactDestination(artifactPath) {
  if (typeof artifactPath !== 'string' || artifactPath.trim() === '') {
    throw new Error('RUV2200 adapter artifact path must be a non-empty relative path.')
  }
  const destination = path.resolve(projectRoot, artifactPath)
  if (destination === projectRoot || !destination.startsWith(projectRoot + path.sep)) {
    throw new Error(`RUV2200 adapter artifact path escapes the project root: ${artifactPath}.`)
  }
  const relative = path.relative(projectRoot, destination).split(path.sep).join('/')
  const allowed = PROJECT_ARTIFACT_ALLOWLIST.some(
    (prefix) => relative === prefix || relative.startsWith(prefix + '/'),
  )
  if (!allowed) {
    throw new Error(
      `RUV2200 project-scope adapter artifact path is not allowlisted: ${artifactPath}. ` +
        `Allowed locations: ${PROJECT_ARTIFACT_ALLOWLIST.join(', ')}.`,
    )
  }
  return destination
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
