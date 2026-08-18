import { cp, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  collectLayouts,
  collectSpecials,
  runtimeAliases,
  serverPlatform,
  INSTRUMENTATION_FILES,
  toImportPath,
} from './compiler.mjs'
import {
  metaSourceImports,
  routeBoundaryPrelude,
  routeContextPrelude,
  routeMetaPrelude,
  routeTreeFunction,
} from './entry-templates.mjs'
import { createPluginRegistry } from './plugin-http.mjs'
import { HANDLER_RUNTIME_FILES, prerenderRelativePath } from './serverless-handler.mjs'
import { actionReferenceId } from './action-runtime.mjs'

const [projectRootArg, outputDirArg, adapterNameArg] = process.argv.slice(2)
const runnerMode = process.env.RUVYXA_ADAPTER_RUNNER_MODE ?? 'build'

if (!projectRootArg || !outputDirArg) {
  writeResponse(
    failure('RUV2200', 'Adapter runner requires project root and build output arguments.'),
  )
  process.exit(1)
}

const projectRoot = path.resolve(projectRootArg)
const outputDir = path.resolve(outputDirArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))
/** The loaded `ruvyxa.config`, shared with the function materializer. */
let projectConfig
const KNOWN_ADAPTER_NAMES = [
  'node',
  'bun',
  'deno',
  'static',
  'vercel',
  'netlify',
  'cloudflare',
  'railway',
  'render',
  'firebase',
  'aws',
]
// Hosting platforms discover deployment output at fixed project-root
// locations. Project-scope artifacts are limited to this allowlist so an
// adapter can enable zero-config deploys without gaining arbitrary write
// access to the project.
/**
 * Runtime a target implies when an adapter does not state one.
 *
 * Targets not listed here run on Node: that is the fallback a self-hosted
 * deployment gets, and it is the only runtime guaranteed to exist.
 */
const DEFAULT_RUNTIME_BY_TARGET = { edge: 'edge', static: 'static' }

const PROJECT_ARTIFACT_ALLOWLIST = [
  '.vercel/output',
  '.netlify/v1',
  'netlify.toml',
  'netlify/functions',
  'wrangler.jsonc',
  'railway.json',
  'render.yaml',
  'firebase.json',
  '.amplify-hosting',
  '_headers',
  '_redirects',
]

try {
  // The config is loaded even when `--adapter <name>` names the deploy target,
  // because it is also where `plugins` live and those have to be compiled into
  // the function bundle. Selecting an adapter on the command line overrides
  // `config.adapter`; it no longer skips the rest of the config.
  const config = await loadConfig(projectRoot)
  // Kept at module scope so the function materializer, several calls deep in
  // `materializeArtifacts`, can compile the project's plugins into the bundle
  // without threading the config through every artifact kind.
  projectConfig = config
  const adapter = adapterNameArg
    ? await loadNamedAdapter(projectRoot, adapterNameArg)
    : config?.adapter
  if (adapter === undefined) {
    writeResponse(success(runnerMode === 'inspect' ? null : []))
  } else if (!adapter || typeof adapter !== 'object' || typeof adapter.build !== 'function') {
    writeResponse(failure('RUV2200', 'config.adapter must provide a build(context) function.'))
    process.exitCode = 1
  } else {
    const buildInfo = await loadBuildInfo(outputDir)
    const output = await adapter.build({ root: projectRoot, outDir: outputDir, buildInfo })
    if (runnerMode === 'inspect') {
      writeResponse(success(inspectAdapter(adapter, output)))
    } else if (runnerMode === 'build') {
      await assertCapabilitiesSupported(adapter, outputDir, config)
      const artifacts = await materializeArtifacts(output, outputDir)
      writeResponse(success(artifacts))
    } else {
      writeResponse(failure('RUV2200', `Unsupported adapter runner mode: ${runnerMode}.`))
      process.exitCode = 1
    }
  }
} catch (error) {
  writeResponse(
    failure('RUV2200', error instanceof Error ? error.message : String(error), error?.stack),
  )
  process.exitCode = 1
}

async function loadBuildInfo(buildDir) {
  try {
    const value = JSON.parse(await readFile(path.join(buildDir, 'build.json'), 'utf8'))
    return value && typeof value === 'object' && !Array.isArray(value) ? value : undefined
  } catch {
    // Inspection can run before a build exists. Build metadata is additive,
    // so adapters retain their previous behavior when it is unavailable.
    return undefined
  }
}

/** Return deployment capabilities without writing any adapter artifacts. */
function inspectAdapter(adapter, output) {
  if (!output || typeof output !== 'object') {
    throw new Error('RUV2200 config.adapter.build(context) must return an output object.')
  }
  const target = output.target ?? adapter.target ?? 'unknown'
  const runtime = output.runtime ?? DEFAULT_RUNTIME_BY_TARGET[target] ?? 'node'
  return {
    name: output.name ?? adapter.name ?? 'unknown',
    target,
    runtime,
    platform: output.platform ?? null,
    supports: Array.isArray(adapter.supports)
      ? adapter.supports
      : ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
  }
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
async function assertCapabilitiesSupported(adapter, buildDir, config) {
  if (!Array.isArray(adapter.supports)) return

  const supported = new Set(adapter.supports)
  const manifestPath = path.join(buildDir, 'manifest.json')
  if (!existsSync(manifestPath)) return
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const adapterName = adapter.name ?? 'unknown'

  const unsupported = (manifest.routes ?? []).filter((route) =>
    route.kind === 'api' ? !supported.has('api') : !supported.has(route.render?.strategy),
  )
  if (unsupported.length > 0) {
    const detail = unsupported
      .map((route) => `${route.path} (${route.kind === 'api' ? 'api' : route.render?.strategy})`)
      .join(', ')
    throw new Error(
      `RUV2202 adapter ${adapterName} supports ${adapter.supports.join(', ')}; ` +
        `unsupported routes: ${detail}.`,
    )
  }

  // Everything below answers "can this target run the code this project
  // actually wrote?", which nothing used to ask. A project could declare
  // server actions, plugin HTTP routes, or a realtime transport, build cleanly
  // against any adapter, and then answer 404 on every one of them in
  // production. Deciding it here turns a silent runtime hole into a build
  // failure that names the feature and the target.
  const dynamic = supported.has('ssr') || supported.has('api')

  const flightRoutes = (manifest.routes ?? []).filter((route) => route.flight === true)
  if (flightRoutes.length > 0 && !dynamic) {
    throw new Error(
      `RUV2204 adapter ${adapterName} publishes a static site and cannot serve Flight, ` +
        `but ${flightRoutes.map((route) => route.path).join(', ')} opt in. ` +
        'Remove the flight export, or build with an adapter that runs a server.',
    )
  }

  const actionRoutes = (manifest.routes ?? []).filter(
    (route) => route.kind !== 'api' && actionFileFor(route),
  )
  if (actionRoutes.length > 0 && !dynamic) {
    throw new Error(
      `RUV2204 adapter ${adapterName} publishes a static site and cannot serve server actions, ` +
        `but ${actionRoutes.map((route) => route.path).join(', ')} declare one. ` +
        'Remove the action file, or build with an adapter that runs a server.',
    )
  }

  const registry = await loadProjectPluginRegistry(config)
  if (!registry) return
  const pluginRoutes = registry.httpRequest.filter((entry) => entry.kind === 'route')
  const hooks = registry.httpRequest.length + registry.httpResponse.length
  if (hooks > 0 && !dynamic) {
    const detail =
      pluginRoutes.length > 0
        ? `routes ${pluginRoutes.map((entry) => entry.path).join(', ')}`
        : 'request/response hooks'
    throw new Error(
      `RUV2204 adapter ${adapterName} publishes a static site and cannot run plugin HTTP ` +
        `behavior, but ${registry.plugins.join(', ')} registered ${detail}. ` +
        'Remove the plugin, or build with an adapter that runs a server.',
    )
  }

  // A native transport is a persistent connection. `ruvyxa start` upgrades the
  // socket itself; nothing a build emits can — not a serverless function, and
  // not the generated standalone server, which serves plain HTTP with no
  // upgrade path.
  //
  // Reported rather than thrown, unlike the cases above. Those describe an
  // adapter that cannot serve the app at all; this one describes an endpoint
  // that will be missing from an otherwise correct deployment, which is a
  // legitimate thing to ship when realtime is only used in development. The
  // client retries a missing endpoint indefinitely and says nothing, so the
  // absence still has to be stated somewhere — here, at build time, once.
  for (const transport of registry.capabilities.values()) {
    console.error(
      `[ruvyxa] RUV2205 plugin ${transport.plugin} claims ${transport.id}, which needs a ` +
        `persistent connection at ${transport.path}. Adapter ${adapterName} emits a build ` +
        `artifact, which cannot hold one, so ${transport.path} will not exist in this ` +
        'deployment. Serve the project with `ruvyxa start` if clients depend on it.',
    )
  }
}

/**
 * Build the project's plugin registry at build time.
 *
 * Doubles as validation: a plugin with a malformed hook, a duplicate name, or a
 * route that collides with a framework endpoint now fails `ruvyxa build`
 * rather than the first production request.
 *
 * The content engine is deliberately absent, unlike in `plugin-runtime.mjs`.
 * Its HTTP hook answers `/content.json` and `/search-index.json`, and the build
 * already writes both as public files, so a deployed site serves them from the
 * CDN. Registering it here would pull the whole content pipeline into every
 * function bundle to answer requests that never reach the function.
 */
function projectPlugins(config) {
  return Array.isArray(config?.plugins) ? config.plugins : []
}

function loadProjectPluginRegistry(config) {
  return createPluginRegistry({ root: projectRoot, plugins: projectPlugins(config) })
}

/**
 * The `action.ts` beside a page route, or `null`.
 *
 * Mirrors `action_file_for` in `crates/ruvyxa_dev_server/src/render_pipeline.rs`,
 * including the `.ts` before `.js` order, so the same file is chosen in a build
 * as under `ruvyxa dev`.
 */
function actionFileFor(route) {
  if (!route || typeof route.file !== 'string' || route.file.trim() === '') return null
  let routeFile
  try {
    routeFile = resolveProjectRouteFile(route.file, route.id)
  } catch {
    return null
  }
  const directory = path.dirname(routeFile)
  for (const name of ['action.ts', 'action.js']) {
    const candidate = path.join(directory, name)
    if (existsSync(candidate)) return candidate
  }
  return null
}

async function loadNamedAdapter(root, name) {
  // A bare short name maps onto the official and community naming
  // conventions; a scoped or slashed name is used verbatim so any published
  // adapter package works with `ruvyxa build --adapter <package>`.
  const candidates =
    name.startsWith('@') || name.includes('/')
      ? [name]
      : [`@ruvyxa/adapter-${name}`, `ruvyxa-adapter-${name}`, name]

  const requireFromProject = createRequire(path.join(root, 'package.json'))
  // Official adapters ship as dependencies of the ruvyxa package itself, so
  // built-in names work with zero install even when the project has not added
  // the adapter package. A project-installed copy still wins.
  const requireFromRuntime = createRequire(import.meta.url)

  let entry
  let resolvedPackage
  for (const candidate of candidates) {
    for (const resolve of [requireFromProject, requireFromRuntime]) {
      try {
        entry = resolve.resolve(candidate)
        resolvedPackage = candidate
        break
      } catch {
        // try the next resolution base
      }
    }
    if (entry) break
  }
  if (!entry) {
    const hint = KNOWN_ADAPTER_NAMES.includes(name)
      ? `Reinstall the ruvyxa package, or add it directly: pnpm add -D @ruvyxa/adapter-${name}`
      : `Expected one of ${KNOWN_ADAPTER_NAMES.join(', ')}, or an installed adapter package (tried: ${candidates.join(', ')}).`
    throw new Error(`RUV2203 adapter ${name} could not be resolved. ${hint}`)
  }
  const mod = await import(pathToFileURL(entry).href)
  const factory = mod.default
  if (typeof factory !== 'function') {
    throw new Error(`RUV2203 ${resolvedPackage} does not export an adapter factory.`)
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
    bundleAliasDependencies: true,
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
  // An adapter may emit the same function bundle at several destinations
  // (deploy directory + platform discovery directory). Compiling the route
  // registry is the expensive step, so identical handler sources are built
  // once and copied afterwards.
  const materializedFunctions = new Map()
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
      await materializeStaticSite(buildDir, destination, {
        requirePrerender: artifact.optional !== true,
        excludeStrategies: Array.isArray(artifact.excludeStrategies)
          ? artifact.excludeStrategies
          : [],
      })
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
      const functionKey = `${output.target ?? ''}\n${artifact.handlerSource}`
      const alreadyBuilt = materializedFunctions.get(functionKey)
      if (alreadyBuilt) {
        await rm(destination, { recursive: true, force: true })
        await mkdir(path.dirname(destination), { recursive: true })
        await cp(alreadyBuilt, destination, { recursive: true })
      } else {
        await materializeFunction(buildDir, destination, artifact.handlerSource, output.target)
        materializedFunctions.set(functionKey, destination)
      }
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

  // Copy the generic serverless handler runtime.
  //
  // `route-match.mjs` and `request-context.mjs` travel with it: the handler
  // imports both as siblings, and a function directory resolves no bare
  // specifiers. A missing file here would surface as a broken deployment on the
  // first request rather than a failed build, so the set is required rather
  // than copied opportunistically.
  for (const runtimeFile of HANDLER_RUNTIME_FILES) {
    const source = path.join(runtimeDir, runtimeFile)
    if (!existsSync(source)) {
      throw new Error(
        `RUV2201 The Ruvyxa runtime file ${runtimeFile} is missing from ${runtimeDir}. ` +
          'Reinstall the ruvyxa package, or run `pnpm --filter ruvyxa build` in a checkout ' +
          'to regenerate it.',
      )
    }
    await cp(source, path.join(destination, runtimeFile))
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

  // Write the route manifest so the handler can do request routing.
  //
  // The `.mjs` copy is what handlers import. A platform bundler (Netlify's
  // zip-it-and-ship-it, Vercel's NFT tracer, wrangler) rewrites the function
  // into a single file and only carries along what it can resolve statically:
  // a sibling `manifest.json` read through `readFileSync(import.meta.dirname)`
  // is invisible to it and disappears from the deployed bundle, which crashed
  // Netlify with `ENOENT /var/task/manifest.json`. A static import is part of
  // the module graph on every platform. `manifest.json` is still written for
  // inspection and for hosts that ship the directory verbatim.
  await writeFile(
    path.join(destination, 'manifest.json'),
    JSON.stringify(manifest, null, 2),
    'utf8',
  )
  await writeFile(
    path.join(destination, 'manifest.mjs'),
    `export default ${JSON.stringify(manifest)}\n`,
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
    if (routes.some((route) => route?.flight === true && route?.cache === true)) {
      imports.push('import { cache as __ruvyxaCache } from "@ruvyxa/core/server"')
      definitions.push(flightCachePrelude())
    }
    // One binding for the whole registry: every route definition below shares
    // it, and a second `const`/`class` in the same module would not parse. The
    // boundary class is emitted unconditionally on the server registry (its
    // cost is a few unused lines when no route declares error/not-found, and
    // this keeps a single definition site instead of a per-route guard).
    definitions.push(routeContextPrelude())
    definitions.push(routeBoundaryPrelude())
    definitions.push(routeMetaPrelude())
  }

  // Server actions live in `action.ts` beside the page they belong to and are
  // absent from the route manifest, so they are discovered here the same way
  // the native server resolves them at request time. Without this the compiled
  // registry had no way to reach them and `POST /__ruvyxa/action` could only
  // ever 404 in a deployed build.
  const actionRecords = []

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

    const page = pageRouteDefinition(routeFile, index, route.path ?? '/', route.cache === true)
    imports.push(...page.imports)
    definitions.push(page.definition)
    records.push(
      `  ${JSON.stringify(route.id)}: { render: ${page.renderName}, flight: ${page.flightName} }`,
    )

    const actionFile = actionFileFor(route)
    if (actionFile) {
      route.actionReferenceId = actionReferenceId(route.id, await readFile(actionFile, 'utf8'))
      const alias = `ActionModule${index}`
      imports.push(`import * as ${alias} from ${JSON.stringify(toImportPath(actionFile))}`)
      actionRecords.push(`  ${JSON.stringify(route.id)}: ${alias}`)
    }
  }

  const plugins = pluginRegistrySource()
  const buildSource = (pluginPart) => `${[...imports, ...pluginPart.imports].join('\n')}

${instrumentationPrelude()}
${definitions.join('\n\n')}

const routeModules = Object.freeze({
${records.join(',\n')}
})

const actionModules = Object.freeze({
${actionRecords.join(',\n')}
})

export async function loadRouteModule(routeId) {
  await __ruvyxaInstrumentationReady
  const routeModule = routeModules[routeId]
  if (!routeModule) throw new Error(\`Route \${routeId} is not present in the compiled registry\`)
  return routeModule
}

/**
 * The action module beside a page route, or null when it declares none.
 *
 * Null rather than a throw: the handler turns it into "no action file for this
 * route", which is the same answer the native server gives.
 */
export async function loadActionModule(routeId) {
  await __ruvyxaInstrumentationReady
  return actionModules[routeId] ?? null
}

${pluginPart.definition}
`
  const outfile = path.join(destination, 'route-modules.mjs')
  await compileRegistry(buildSource(plugins), outfile, target)

  // An edge runtime has no Node built-ins. Compiling the plugin registry into
  // the bundle brings `ruvyxa.config` and everything it imports with it, and
  // `ruvyxa/plugins` reaches `node:fs`, `node:path`, and `node:crypto` — so a
  // Worker built this way would throw on module load and answer nothing, which
  // is worse than the 404s this change set out to remove. Prove the plugins are
  // the cause by rebuilding without them before blaming them, then refuse the
  // build rather than emitting the artifact.
  if (target === 'edge' && plugins.imports.length > 0) {
    const builtins = nodeBuiltinImports(await readFile(outfile, 'utf8'))
    if (builtins.length > 0) {
      const withoutPlugins = { imports: [], definition: 'export const applyPluginHttp = undefined' }
      await compileRegistry(buildSource(withoutPlugins), outfile, target)
      if (nodeBuiltinImports(await readFile(outfile, 'utf8')).length === 0) {
        throw new Error(
          `RUV2206 the project's plugins reach ${builtins.join(', ')}, which an edge runtime ` +
            'does not provide, so plugin HTTP hooks cannot be compiled into this function. ' +
            'Build for a Node or Bun target, or remove the plugin from ruvyxa.config.',
        )
      }
      // The routes themselves reach a Node built-in. That is the pre-existing
      // shape of this project against an edge target and is not this step's to
      // decide, so the registry is restored with its plugins intact.
      await compileRegistry(buildSource(plugins), outfile, target)
    }
  }
}

/** Top-level `node:` specifiers a compiled registry still imports. */
function nodeBuiltinImports(source) {
  const specifiers = new Set()
  for (const match of source.matchAll(/(?:from|require\()\s*["'](node:[a-z_/]+)["']/g)) {
    specifiers.add(match[1])
  }
  return [...specifiers].sort()
}

async function compileRegistry(entrySource, outfile, target) {
  await compileBundle({
    projectRoot,
    entrySource,
    sourcefile: 'ruvyxa:serverless-route-registry.tsx',
    outfile,
    platform: target === 'edge' ? 'browser' : serverPlatform(),
    bundlePackages: true,
    reactCompiler: projectConfig?.reactCompiler === true,
    aliases: runtimeAliases(runtimeDir),
  })
}

/**
 * Source that runs the project's plugin HTTP hooks inside a function bundle.
 *
 * The plugins themselves are imported from `ruvyxa.config`, so they are
 * compiled into the bundle by the same pass that compiles the routes — the
 * only way to reach them, since a deployed function cannot spawn
 * `plugin-runtime.mjs` and could not resolve its bare specifiers if it tried.
 *
 * The registry is built lazily and memoized rather than at module scope: a
 * `register()` hook may be async, and a cold start that throws while building
 * the registry must surface on the request that triggered it rather than
 * breaking the module import for every route.
 *
 * Emitted as an inert stub when the project declares no plugins, so the common
 * case ships no extra code and the handler skips the pipeline entirely.
 */
function pluginRegistrySource() {
  const configFile = findConfig(projectRoot)
  if (!configFile || projectPlugins(projectConfig).length === 0) {
    return { imports: [], definition: 'export const applyPluginHttp = undefined' }
  }

  const pluginHttpModule = path.join(runtimeDir, 'plugin-http.mjs')
  return {
    imports: [
      `import __ruvyxaConfig from ${JSON.stringify(toImportPath(configFile))}`,
      'import {' +
        ' createPluginRegistry as __ruvyxaCreatePluginRegistry,' +
        ' dispatchPluginRequest as __ruvyxaDispatchPluginRequest,' +
        ' dispatchPluginResponse as __ruvyxaDispatchPluginResponse,' +
        ' hasPluginHttp as __ruvyxaHasPluginHttp' +
        `} from ${JSON.stringify(toImportPath(pluginHttpModule))}`,
    ],
    definition: `let __ruvyxaPluginRegistry

function __ruvyxaPluginRegistryReady() {
  __ruvyxaPluginRegistry ??= __ruvyxaCreatePluginRegistry({
    root: ${JSON.stringify(projectRoot)},
    plugins: Array.isArray(__ruvyxaConfig?.plugins) ? __ruvyxaConfig.plugins : [],
  })
  return __ruvyxaPluginRegistry
}

export async function applyPluginHttp(request, next) {
  const registry = await __ruvyxaPluginRegistryReady()
  if (!__ruvyxaHasPluginHttp(registry)) return next(request)
  const outcome = await __ruvyxaDispatchPluginRequest(registry, request)
  // A short-circuiting hook returns its response directly, without the
  // response hooks running over it. That is what the native server does, where
  // \`apply_request_plugins\` returning a response returns from the handler.
  if (outcome.kind === 'response') return outcome.response
  const response = await next(outcome.request)
  return __ruvyxaDispatchPluginResponse(registry, outcome.request, response)
}`,
  }
}

/**
 * Source that runs the project's `instrumentation.ts` inside a function bundle.
 *
 * Emitted into the route registry rather than into each platform's handler
 * entry point, because the registry is the one module every adapter wrapper
 * imports and reaching it does not mean editing ten handler templates.
 * A shared promise runs it exactly once per runtime instance. The route loader
 * awaits it before returning the first module. Avoid top-level await here: the
 * compiler wraps the registry in an IIFE, where `await` is invalid syntax.
 *
 * A failure is logged and swallowed. A misconfigured telemetry SDK must not
 * make every route in the deployment fail to import.
 */
function instrumentationPrelude() {
  const entry = INSTRUMENTATION_FILES.map((name) => path.join(projectRoot, name)).find(
    (candidate) => existsSync(candidate),
  )
  if (!entry) return 'const __ruvyxaInstrumentationReady = Promise.resolve()'

  return [
    `import * as __ruvyxaInstrumentation from ${JSON.stringify(toImportPath(entry))}`,
    'const __ruvyxaInstrumentationReady = Promise.resolve()',
    "  .then(() => typeof __ruvyxaInstrumentation.register === 'function'",
    '    ? __ruvyxaInstrumentation.register()',
    '    : undefined)',
    '  .catch((error) => {',
    "    console.error('[ruvyxa] instrumentation failed:', error)",
    '  })',
  ].join('\n')
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

function pageRouteDefinition(pageFile, routeIndex, routePath = '/', cacheFlight = false) {
  const appDir = path.join(projectRoot, 'app')
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const pageName = `Page${routeIndex}`
  const moduleName = `PageModule${routeIndex}`
  const renderName = `renderPage${routeIndex}`
  const treeName = `buildTree${routeIndex}`
  const flightName = cacheFlight
    ? `flightPage${routeIndex}`
    : `typeof ${moduleName}.flight === 'function' ? ${moduleName}.flight : null`
  const imports = [
    `import ${pageName}, * as ${moduleName} from ${JSON.stringify(toImportPath(pageFile))}`,
  ]
  const wrappers = []
  layouts.forEach((layoutFile, index) => {
    const layoutName = `Layout${routeIndex}_${index}`
    imports.push(`import ${layoutName} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(layoutName)
  })

  // Unique identifiers per route: every page shares one registry module, so
  // reusing `RouteError` across routes would collide.
  const specialNames = { errorName: null, loadingName: null, notFoundName: null }
  for (const [kind, ident, nameKey] of [
    ['error', `RouteError${routeIndex}`, 'errorName'],
    ['loading', `RouteLoading${routeIndex}`, 'loadingName'],
    ['notFound', `RouteNotFound${routeIndex}`, 'notFoundName'],
  ]) {
    if (specials[kind]) {
      imports.push(`import ${ident} from ${JSON.stringify(toImportPath(specials[kind]))}`)
      specialNames[nameKey] = ident
    }
  }

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
    `__ruvyxaMeta${routeIndex}_`,
  )
  imports.push(...metaImports)

  const cachedFlight = cacheFlight
    ? `\n\nasync function ${flightName}(ctx) {\n  return __ruvyxaCache(__ruvyxaFlightKey(${JSON.stringify(routePath)}, ctx)).get(() => ${moduleName}.flight(ctx))\n}`
    : ''
  const definition = `${routeTreeFunction({
    name: treeName,
    pageName,
    layoutNames: wrappers,
    routePath,
    metaNames,
    ...specialNames,
  })}

async function ${renderName}(ctx) {
  const tree = ${treeName}(ctx)

  let html
  if (typeof ReactDomServer.renderToReadableStream === "function") {
    const stream = await ReactDomServer.renderToReadableStream(tree)
    html = await new Response(stream).text()
  } else if (typeof ReactDomServer.renderToString === "function") {
    html = ReactDomServer.renderToString(tree)
  } else {
    throw new Error("React server renderer is unavailable")
  }
  const document = html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html
  return __ruvyxaApplyLang(document, __ruvyxaResolveMeta([${metaNames.join(', ')}], ctx).lang)
}${cachedFlight}`
  return { imports, definition, renderName, moduleName, flightName }
}

/**
 * Deterministic, collision-free key material for a cached Flight producer.
 *
 * The key ordering is written out here rather than imported: this string is
 * emitted into a function artifact that resolves no bare or sibling
 * specifiers. It has to agree with `flightCacheKey` in `worker-pool.mjs`,
 * which computes the same key on the build host — so both compare code units.
 * `localeCompare` made the ordering depend on each host's ICU locale, and the
 * two run in different environments by construction.
 */
function flightCachePrelude() {
  return `function __ruvyxaFlightKey(route, ctx) {
  const params = Object.fromEntries(
    Object.entries(ctx.params ?? {}).sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0)),
  )
  return "flight:" + JSON.stringify([route, ctx.path, params])
}`
}

// Copies the pre-rendered pages and client assets into a publish directory.
// Which routes are allowed to exist at all is decided by `adapter.supports`
// before the build hook runs (see `assertCapabilitiesSupported`); a hybrid adapter
// legitimately emits this artifact for the static layer of an app that also has
// SSR pages and API routes served by its function artifact.
async function materializeStaticSite(
  buildDir,
  destination,
  { requirePrerender = true, excludeStrategies = [] } = {},
) {
  const prerenderDir = path.join(buildDir, 'prerender')
  if (requirePrerender && !existsSync(prerenderDir)) {
    throw new Error('RUV2202 static adapter output requires generated prerendered pages.')
  }

  await mkdir(destination, { recursive: true })
  // Match the production server's public URLs: `assets/foo.png` is served as
  // `/foo.png`, while hashed client bundles live under `/__ruvyxa/client/`.
  // Prerendered routes are copied last so a page wins an exact URL collision
  // in the same way routing wins before public-file fallback at runtime.
  // `.ruvyxa-images.json` is build telemetry (source paths, byte counts) that
  // the optimizer drops beside the images. It answers no request and must not
  // become a public URL on the deployed site.
  await copyDirectoryContents(
    path.join(buildDir, 'assets'),
    destination,
    new Set(['.ruvyxa-images.json']),
  )
  await copyDirectoryContents(
    path.join(buildDir, 'client'),
    path.join(destination, '__ruvyxa', 'client'),
  )
  await copyDirectoryContents(
    prerenderDir,
    destination,
    new Set(['manifest.json']),
    await dynamicPrerenderFiles(prerenderDir, excludeStrategies),
  )
}

/**
 * Relative HTML paths whose route is served by the function, not the CDN.
 *
 * `prerender/manifest.json` records the concrete path and strategy of every
 * page written at build time, including the expansions of dynamic routes, so
 * an ISR page can be held back from the publish directory without the adapter
 * having to know which parameter values existed.
 */
async function dynamicPrerenderFiles(prerenderDir, strategies) {
  if (strategies.length === 0) return new Set()
  const manifestPath = path.join(prerenderDir, 'manifest.json')
  if (!existsSync(manifestPath)) return new Set()

  const excluded = new Set()
  try {
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
    for (const route of manifest.routes ?? []) {
      if (!strategies.includes(route?.strategy)) continue
      const relative = prerenderRelativePath(route?.path)
      if (relative !== null) excluded.add(relative)
    }
  } catch {
    // A malformed prerender manifest must not drop the publish directory;
    // publishing every page is the previous, safe behavior.
    return new Set()
  }
  return excluded
}

async function copyDirectoryContents(
  source,
  destination,
  excluded = new Set(),
  excludedPaths = new Set(),
) {
  if (!existsSync(source)) return
  await mkdir(destination, { recursive: true })
  const filter =
    excludedPaths.size === 0
      ? undefined
      : (from) => !excludedPaths.has(path.relative(source, from).split(path.sep).join('/'))
  for (const entry of await readdir(source, { withFileTypes: true })) {
    if (excluded.has(entry.name)) continue
    await cp(path.join(source, entry.name), path.join(destination, entry.name), {
      recursive: true,
      ...(filter === undefined ? {} : { filter }),
    })
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
