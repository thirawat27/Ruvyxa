import { cp, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  collectLayouts,
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
  'netlify/functions',
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
    await assertRoutesSupported(adapter, outputDir)
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

/**
 * Reject routes the adapter cannot deploy, before its build hook runs.
 *
 * The capability belongs to the adapter, not to the artifact kind it happens to
 * emit. `static-site` is used both by the static adapter -- which publishes the
 * whole site and therefore cannot deploy SSR/ISR/PPR pages or API routes -- and
 * by the vercel/netlify/cloudflare adapters, which emit it for the static asset
 * layer that sits *next to* a serverless function that serves exactly those
 * routes. Enforcing the static-only constraint inside `materializeStaticSite`
 * therefore blocked every hybrid adapter from building any app with an API
 * route or an SSR page. Checking `adapter.supports` keeps the constraint with
 * the adapter that actually has it.
 *
 * An adapter that omits `supports` is treated as full-featured.
 */
async function assertRoutesSupported(adapter, buildDir) {
  if (!Array.isArray(adapter.supports)) return

  const supported = new Set(adapter.supports)
  const manifestPath = path.join(buildDir, 'manifest.json')
  if (!existsSync(manifestPath)) return
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))

  const unsupported = (manifest.routes ?? []).filter((route) =>
    route.kind === 'api' ? !supported.has('api') : !supported.has(route.render?.strategy),
  )
  if (unsupported.length === 0) return

  const detail = unsupported
    .map((route) => `${route.path} (${route.kind === 'api' ? 'api' : route.render?.strategy})`)
    .join(', ')
  throw new Error(
    `RUV2202 adapter ${adapter.name ?? 'unknown'} supports ${adapter.supports.join(', ')}; ` +
      `unsupported routes: ${detail}.`,
  )
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
    if (artifact.kind === 'function') {
      if (typeof artifact.handlerSource !== 'string') {
        throw new Error(
          `RUV2200 function artifact ${artifact.path} must include handlerSource string.`,
        )
      }
      await materializeFunction(buildDir, destination, artifact.handlerSource, output.target)
      artifacts.push(
        scope === 'project'
          ? { kind: 'function', path: artifact.path, scope }
          : { kind: 'function', path: artifact.path },
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

async function materializeFunction(buildDir, destination, handlerSource, target) {
  const manifestPath = path.join(buildDir, 'manifest.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))

  // A function artifact is a complete deployment unit. Replacing it prevents
  // removed or renamed route bundles from surviving incremental builds.
  await rm(destination, { recursive: true, force: true })
  await mkdir(destination, { recursive: true })

  // Write the platform-specific handler entry point
  await writeFile(path.join(destination, 'index.mjs'), handlerSource, 'utf8')

  // Copy the generic serverless handler runtime
  const serverlessHandlerSrc = path.join(runtimeDir, 'serverless-handler.mjs')
  if (existsSync(serverlessHandlerSrc)) {
    await cp(serverlessHandlerSrc, path.join(destination, 'serverless-handler.mjs'))
  }

  // Compile every route into executable JavaScript and expose it through a
  // static import registry. Static imports let edge bundlers discover all
  // modules; compiling here avoids shipping raw TS/TSX and removes the
  // manifest path ambiguity that previously produced server/app/app/....
  await materializeRouteModules(manifest, destination, target)

  // Copy pre-rendered pages for ISR/SSG fallback
  const prerenderDir = path.join(buildDir, 'prerender')
  if (existsSync(prerenderDir)) {
    await cp(prerenderDir, path.join(destination, 'prerender'), { recursive: true })
  }

  // Write the route manifest so the handler can do request routing
  await writeFile(
    path.join(destination, 'manifest.json'),
    JSON.stringify(manifest, null, 2),
    'utf8',
  )
}

async function materializeRouteModules(manifest, destination, target) {
  const routes = Array.isArray(manifest.routes) ? manifest.routes : []
  const imports = []
  const definitions = []
  const records = []
  const seenIds = new Set()
  const hasPages = routes.some((route) => route?.kind !== 'api')
  if (hasPages) {
    const renderer = target === 'edge' ? 'react-dom/server.browser' : 'react-dom/server'
    imports.push('import React from "react"')
    imports.push(`import * as ReactDomServer from ${JSON.stringify(renderer)}`)
  }

  for (const [index, route] of routes.entries()) {
    if (!route || typeof route !== 'object' || typeof route.id !== 'string') {
      throw new Error(`RUV2200 manifest route at index ${index} must have a string id.`)
    }
    if (seenIds.has(route.id)) {
      throw new Error(`RUV2200 manifest contains duplicate route id: ${route.id}.`)
    }
    seenIds.add(route.id)

    const routeFile = resolveProjectRouteFile(route.file, route.id)
    if (route.kind === 'api') {
      const alias = `ApiRoute${index}`
      imports.push(`import * as ${alias} from ${JSON.stringify(toImportPath(routeFile))}`)
      records.push(`  ${JSON.stringify(route.id)}: ${alias}`)
      continue
    }

    const page = pageRouteDefinition(routeFile, index)
    imports.push(...page.imports)
    definitions.push(page.definition)
    records.push(`  ${JSON.stringify(route.id)}: { render: ${page.renderName} }`)
  }

  const registrySource = `${imports.join('\n')}

${definitions.join('\n\n')}

const routeModules = Object.freeze({
${records.join(',\n')}
})

export async function loadRouteModule(routeId) {
  const routeModule = routeModules[routeId]
  if (!routeModule) throw new Error(\`Route \${routeId} is not present in the compiled registry\`)
  return routeModule
}
`
  await compileBundle({
    projectRoot,
    entrySource: registrySource,
    sourcefile: 'ruvyxa:serverless-route-registry.tsx',
    outfile: path.join(destination, 'route-modules.mjs'),
    platform: target === 'edge' ? 'browser' : serverPlatform(),
    bundlePackages: true,
    aliases: runtimeAliases(runtimeDir),
  })
}

function resolveProjectRouteFile(routeFile, routeId) {
  if (typeof routeFile !== 'string' || routeFile.trim() === '') {
    throw new Error(`RUV2200 manifest route ${routeId} must have a source file.`)
  }
  const segments = routeFile.split(/[\\/]+/).filter((segment) => segment && segment !== '.')
  const samePlatformAbsolute = path.isAbsolute(routeFile)
  const candidates = samePlatformAbsolute
    ? [path.resolve(routeFile)]
    : [path.resolve(projectRoot, ...segments), path.resolve(...segments)]
  const resolved = candidates.find(
    (candidate) => candidate.startsWith(projectRoot + path.sep) && existsSync(candidate),
  )
  if (!resolved) {
    throw new Error(`RUV2200 manifest route ${routeId} source does not exist: ${routeFile}.`)
  }
  return resolved
}

function pageRouteDefinition(pageFile, routeIndex) {
  const appDir = path.join(projectRoot, 'app')
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const pageName = `Page${routeIndex}`
  const renderName = `renderPage${routeIndex}`
  const imports = [`import ${pageName} from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []
  layouts.forEach((layoutFile, index) => {
    const layoutName = `Layout${routeIndex}_${index}`
    imports.push(`import ${layoutName} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(layoutName)
  })

  const definition = `async function ${renderName}(ctx) {
  let tree = React.createElement(${pageName}, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }

  let html
  if (typeof ReactDomServer.renderToReadableStream === "function") {
    const stream = await ReactDomServer.renderToReadableStream(tree)
    html = await new Response(stream).text()
  } else if (typeof ReactDomServer.renderToString === "function") {
    html = ReactDomServer.renderToString(tree)
  } else {
    throw new Error("React server renderer is unavailable")
  }
  return html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html
}`
  return { imports, definition, renderName }
}

// Copies the pre-rendered pages and client assets into a publish directory.
// Which routes are allowed to exist at all is decided by `adapter.supports`
// before the build hook runs (see `assertRoutesSupported`); a hybrid adapter
// legitimately emits this artifact for the static layer of an app that also has
// SSR pages and API routes served by its function artifact.
async function materializeStaticSite(buildDir, destination) {
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
