import { cp, mkdir, readdir, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync, writeSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  compileBundleWithMetadata,
  collectLayouts,
  collectSlots,
  collectSpecials,
  collectTemplates,
  runtimeAliases,
  serverPlatform,
  INSTRUMENTATION_FILES,
  toImportPath,
} from './compiler.mjs'
import {
  documentAssetsPrelude,
  documentStreamPrelude,
  metaSourceImports,
  routeBoundaryPrelude,
  routeContextPrelude,
  routeMetaPrelude,
  routeTreeFunction,
  rscActionEntrySource,
  rscServerEntrySource,
  wrapperEntryParts,
} from './entry-templates.mjs'
import {
  RSC_RENDERER_SPECIFIER,
  RSC_SSR_PACKAGE,
  clientRegistrySource,
  mergeServerReferences,
} from './client-references.mjs'
import { createPluginRegistry } from './plugin-http.mjs'
import { HANDLER_RUNTIME_FILES, prerenderRelativePath } from './serverless-handler.mjs'
import { actionReferenceId } from './action-runtime.mjs'

// Declared above the top-level `await` this file runs, not beside the function
// that reads them: a `const` below a top-level await is in its temporal dead
// zone while that await is pending, so reading one from the build path threw
// `Cannot access before initialization` — the same trap `isNullBodyStatus` in
// plugin-runtime.mjs documents.
/** The key the deployment description occupies in `manifest.json`. */
const DEPLOY_MANIFEST_KEY = 'deploy'

/**
 * The pre-rendered not-found document, inside the prerender directory.
 *
 * `404.html` is the name every static host already looks for, so a static-only
 * publish gets a real not-found page without being configured to, and a
 * function build carries the same bytes.
 */
const NOT_FOUND_DOCUMENT_FILE = '404.html'

/** The deployment-output contract version this runtime understands. */
const DEPLOY_MANIFEST_VERSION = 1

const [projectRootArg, outputDirArg, adapterNameArg] = process.argv.slice(2)
const runnerMode = process.env.RUVYXA_ADAPTER_RUNNER_MODE ?? 'build'

if (!projectRootArg || !outputDirArg) {
  exitWithResponse(
    failure('RUV2200', 'Adapter runner requires project root and build output arguments.'),
    1,
  )
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
/**
 * Runtime a target implies when an adapter does not state one.
 *
 * Targets not listed here run on Node: that is the fallback a self-hosted
 * deployment gets, and it is the only runtime guaranteed to exist.
 */
const DEFAULT_RUNTIME_BY_TARGET = { edge: 'edge', static: 'static' }

/**
 * Project paths a deploy adapter must never write.
 *
 * This used to run the other way — an allowlist of eleven hosting locations,
 * one per official adapter. Two things were wrong with it. A community adapter
 * could not emit the file its platform discovers (`fly.toml`, a `Dockerfile`,
 * `app.yaml`) at all, so every new target meant editing this file; and it read
 * as a security boundary while not being one, since an adapter is a JavaScript
 * function the project installed and named in its own config and therefore
 * already has `node:fs`.
 *
 * What the rule genuinely protects is the project's own source and manifests
 * from a build step whose job is to add deployment configuration beside them.
 * Written as that, it stays correct for adapters nobody here has seen.
 *
 * The configured `appDir` and `outDir` are added at check time rather than
 * listed, because a project may have moved either.
 */
const PROTECTED_PROJECT_ENTRIES = [
  '.git',
  'app',
  'src',
  'pages',
  'components',
  'lib',
  'plugins',
  'public',
  'node_modules',
  'package.json',
  'package-lock.json',
  'pnpm-lock.yaml',
  'pnpm-workspace.yaml',
  'yarn.lock',
  'bun.lock',
  'bun.lockb',
  'deno.json',
  'deno.jsonc',
  'tsconfig.json',
]

/** `ruvyxa.config.*` in every extension the config loader accepts. */
const PROJECT_CONFIG_FILE = /^ruvyxa\.config\.(?:ts|mts|js|mjs)$/

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
    ? await loadNamedAdapter(projectRoot, adapterNameArg, adapterOptionsFor(config))
    : configuredAdapter(config)
  if (adapter === undefined) {
    writeResponse(success(runnerMode === 'inspect' ? null : []))
  } else if (!adapter || typeof adapter !== 'object' || typeof adapter.build !== 'function') {
    writeResponse(failure('RUV2200', 'config.adapter must provide a build(context) function.'))
    process.exitCode = 1
  } else {
    const buildInfo = await loadBuildInfo(outputDir)
    const deployManifest = await loadDeployManifest(outputDir)
    const output = await adapter.build({
      root: projectRoot,
      outDir: outputDir,
      buildInfo,
      deployManifest,
    })
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

/**
 * The build's deployment description, or `undefined`.
 *
 * Read once here and handed to every adapter, so eleven adapters do not each
 * re-derive which routes may be published as files and what cache-control their
 * URLs carry. A version this package does not understand is treated as absent
 * rather than guessed at: `parseDeployManifest` returns null, and every helper
 * that reads the manifest falls back to deriving the same answer, which is what
 * each adapter did before the manifest existed.
 *
 * Read from the `deploy` section of `manifest.json`, not from a file of its
 * own: an older build simply has no such section, which the same fallback
 * covers.
 */
async function loadDeployManifest(buildDir) {
  try {
    const source = await readFile(path.join(buildDir, 'manifest.json'), 'utf8')
    const manifest = JSON.parse(source)[DEPLOY_MANIFEST_KEY]
    // The same three checks `parseDeployManifest` makes in @ruvyxa/core, which
    // this file cannot import: the runtime ships as plain `.mjs` beside the
    // framework package and resolves no workspace specifiers. Both are replayed
    // against tests/fixtures/deploy-output-conformance.json.
    if (!manifest || typeof manifest !== 'object') return undefined
    if (manifest.framework !== 'ruvyxa') return undefined
    if (typeof manifest.version !== 'number' || manifest.version > DEPLOY_MANIFEST_VERSION) {
      return undefined
    }
    return Array.isArray(manifest.routes) ? manifest : undefined
  } catch {
    // Inspection runs before a build exists, and an older build has no
    // manifest at all.
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
  const manifestPath = path.join(buildDir, 'manifest.json')
  if (!existsSync(manifestPath)) return
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  const adapterName = adapter.name ?? 'unknown'

  // Checked for every adapter, including ones that declare no `supports` list,
  // because this is not a capability the adapter could grant. Every adapter
  // serves pages through the route modules this file generates, and those are
  // built by `nodeSsrEntrySource` — the ordinary pipeline, not the
  // server-components one. A route that opted in and is *not* pre-rendered
  // would be served as plain SSR: the document would carry no Flight payload,
  // its browser bundle would find nothing to hydrate, and the page would go out
  // as static HTML with no error anywhere. A pre-rendered one is fine — the
  // payload is already inside the file the adapter copies.
  // A server-components route is deployable: `materializeRouteModules` compiles
  // its `react-server` graph and its SSR registry into the function bundle and
  // renders through `renderServerComponents`, the same pipeline `ruvyxa start`
  // uses. What it cannot do is render on a target with no server at all — a
  // static publish has nothing left to run the Flight pass — so the refusal is
  // now about the adapter rather than about the strategy.
  if (!Array.isArray(adapter.supports)) return
  const supported = new Set(adapter.supports)

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
  return createPluginRegistry({
    root: projectRoot,
    plugins: projectPlugins(config),
    environment: 'production',
  })
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

/**
 * Return `config.adapterOptions`, validated as an options object.
 *
 * The key was declared in `RuvyxaConfig`, validated by `config-renderer.mjs`,
 * carried into `build.json` by `ruvyxa build`, and documented in the
 * configuration guide — and then read by nothing, because this file called the
 * adapter factory with no arguments at all. Every zero-config path is selected
 * by name (`--adapter`, `RUVYXA_ADAPTER`, or platform detection), so a project
 * deploying that way had no way to configure its adapter whatsoever.
 *
 * Validated here as well as in the renderer because the two see different
 * objects: the renderer validates the config it projects for the Rust host,
 * while this file compiles and imports `ruvyxa.config` itself.
 */
function adapterOptionsFor(config) {
  const options = config?.adapterOptions
  if (options === undefined) return {}
  if (!options || typeof options !== 'object' || Array.isArray(options)) {
    throw new Error(
      `RUV2200 config.adapterOptions must be an object, got ${Array.isArray(options) ? 'array' : typeof options}.`,
    )
  }
  return options
}

/**
 * Return `config.adapter`, refusing options that would silently do nothing.
 *
 * `adapterOptions` configures an adapter this file constructs by name. When the
 * config already holds a constructed adapter, its options went to the factory
 * call that built it, and `adapterOptions` has nothing left to reach — so
 * setting both is reported rather than ignored. Ignoring it is the failure this
 * whole key had for its entire existence.
 */
function configuredAdapter(config) {
  const adapter = config?.adapter
  if (adapter !== undefined && config?.adapterOptions !== undefined) {
    throw new Error(
      'RUV2200 config.adapterOptions configures an adapter selected by name (`--adapter`, ' +
        'RUVYXA_ADAPTER, or platform detection), and config.adapter is already constructed. ' +
        'Pass the options to the factory instead — `adapter: vercel({ … })` — or drop ' +
        '`adapter` and let the deploy target select it.',
    )
  }
  return adapter
}

async function loadNamedAdapter(root, name, options) {
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
  // Every official factory takes one options object and defaults it to `{}`, so
  // passing one is safe for an adapter that ignores options entirely.
  return factory(options ?? {})
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
      // A function may name its own runtime and its own slice of the routes,
      // which is what lets one deployment answer some paths from an edge
      // runtime and the rest from Node. Both default to the output's own
      // answer, so an adapter that says nothing gets exactly what it got
      // before. Both belong to the identity: two functions differing only in
      // which routes they carry are not the same directory.
      const functionTarget = artifact.target ?? output.target
      const functionRoutes = Array.isArray(artifact.routes) ? [...artifact.routes].sort() : null
      const functionKey = [
        functionTarget ?? '',
        functionRoutes ? functionRoutes.join(',') : '*',
        artifact.handlerSource,
      ].join('\n')
      const alreadyBuilt = materializedFunctions.get(functionKey)
      if (alreadyBuilt) {
        await rm(destination, { recursive: true, force: true })
        await mkdir(path.dirname(destination), { recursive: true })
        await cp(alreadyBuilt, destination, { recursive: true })
      } else {
        await materializeFunction(
          buildDir,
          destination,
          artifact.handlerSource,
          functionTarget,
          functionRoutes,
        )
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
  const protectedBy = protectedProjectEntry(relative)
  if (protectedBy !== null) {
    throw new Error(
      `RUV2200 project-scope adapter artifact would overwrite project source: ${artifactPath} ` +
        `(protected: ${protectedBy}). A deploy adapter writes platform configuration beside the ` +
        'project, never over it.',
    )
  }
  return destination
}

/**
 * The protected entry `relative` falls under, or null when it is free to write.
 *
 * Matches whole path segments only: `apple.json` is not inside `app`, and a
 * prefix test would have said it was.
 */
function protectedProjectEntry(relative) {
  const owned = [
    ...PROTECTED_PROJECT_ENTRIES,
    projectConfig?.appDir ?? 'app',
    projectConfig?.outDir ?? '.ruvyxa',
  ]
  for (const entry of owned) {
    const normalized = String(entry).replace(/\\/g, '/').replace(/^\.\//, '').replace(/\/+$/, '')
    if (normalized === '' || normalized === '.') continue
    if (relative === normalized || relative.startsWith(normalized + '/')) return normalized
  }
  const [first] = relative.split('/')
  return PROJECT_CONFIG_FILE.test(first) ? first : null
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

async function materializeFunction(buildDir, destination, handlerSource, target, routeIds = null) {
  const manifestPath = path.join(buildDir, 'manifest.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))

  // A function that serves only part of the application carries only that part.
  // The registry it compiles and the manifest it routes with are narrowed in
  // one place, so the two can never disagree about which routes it owns — a
  // function whose manifest claims a route its registry has no entry for
  // answers that path with a lookup failure at request time and nothing at
  // build time. An unfiltered function, which is every adapter that says
  // nothing, keeps the whole manifest.
  if (Array.isArray(routeIds)) {
    const wanted = new Set(routeIds)
    const all = Array.isArray(manifest.routes) ? manifest.routes : []
    const kept = all.filter((route) => wanted.has(route?.id))
    if (kept.length !== wanted.size) {
      const missing = [...wanted].filter((id) => !kept.some((route) => route?.id === id))
      throw new Error(
        `RUV2200 function ${path.basename(destination)} asked for route ids the manifest ` +
          `does not have: ${missing.join(', ')}.`,
      )
    }
    manifest.routes = kept
  }

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
  await materializeRouteModules(manifest, destination, target, buildDir)

  // Copy pre-rendered pages for ISR/SSG fallback
  const prerenderDir = path.join(buildDir, 'prerender')
  if (existsSync(prerenderDir)) {
    await cp(prerenderDir, path.join(destination, 'prerender'), { recursive: true })
  }

  // The document an unmatched URL is answered with.
  //
  // `ruvyxa build` pre-renders the project's own `app/not-found.tsx` into
  // `prerender/404.html`; carried inline in the manifest rather than read from
  // disk at request time because an edge runtime has no filesystem, and because
  // a platform bundler that rewrites this directory into one file would not
  // find a `readFileSync` of a sibling. Without it a deployed application
  // answered every unmatched URL with the bare string `Not Found`, while the
  // same code under `ruvyxa dev` rendered the project's page.
  const notFoundDocument = path.join(prerenderDir, NOT_FOUND_DOCUMENT_FILE)
  if (existsSync(notFoundDocument)) {
    manifest.notFoundDocument = await readFile(notFoundDocument, 'utf8')
  }

  await writeRuntimeManifest(destination, manifest)
}

/**
 * Write the route manifest a deployed function routes with.
 *
 * The only place this repository writes a manifest into a runtime directory, so
 * that what a runtime copy must *not* contain is decided once rather than
 * remembered at each writer. Today that is the `deploy` section: how to serve a
 * build is a build-time question the adapter has already answered, the running
 * function has no use for it, and on an edge runtime the whole registry is
 * inlined into a single worker. A second writer that forgot would ship it, and
 * nothing about the result would look wrong.
 *
 * Two files, one content. The `.mjs` copy is what handlers import: a platform
 * bundler (Netlify's zip-it-and-ship-it, Vercel's NFT tracer, wrangler) rewrites
 * the function into a single file and only carries along what it can resolve
 * statically, so a sibling `manifest.json` read through
 * `readFileSync(import.meta.dirname)` is invisible to it and disappears from
 * the deployed bundle — which crashed Netlify with
 * `ENOENT /var/task/manifest.json`. A static import is part of the module graph
 * on every platform. `manifest.json` is still written for inspection and for
 * hosts that ship the directory verbatim.
 */
async function writeRuntimeManifest(destination, manifest) {
  const { [DEPLOY_MANIFEST_KEY]: _buildTimeOnly, ...runtime } = manifest
  await writeFile(path.join(destination, 'manifest.json'), JSON.stringify(runtime, null, 2), 'utf8')
  await writeFile(
    path.join(destination, 'manifest.mjs'),
    `export default ${JSON.stringify(runtime)}\n`,
    'utf8',
  )
}

/**
 * Per-route browser assets, read from the client manifest the build wrote.
 *
 * The same file `load_prerender_client_assets` reads on the Rust side, and the
 * same four fields. Baked into the generated registry as literals rather than
 * shipped and read at request time: they are fixed once the build has run, and
 * a deployed function that had to load a manifest would need it copied into
 * every function directory.
 *
 * An unreadable or absent manifest yields an empty map, which leaves every
 * route without a client script — the behaviour every deployed build had until
 * this existed, so a build that somehow produced no manifest degrades to it
 * rather than failing.
 */
/**
 * The stylesheet link a deployed document carries, or an empty string.
 *
 * `ruvyxa build` writes the project's compiled CSS as a content-addressed asset
 * and records it in the client route manifest; the adapter copies the whole
 * client directory, so the file is already beside the function. Before this
 * existed a request-time render on a deployed build had no stylesheet at all —
 * the pre-rendered pages carried theirs inline and every other route was
 * unstyled.
 */
async function styleHeadTag(buildDir) {
  try {
    const manifest = JSON.parse(
      await readFile(path.join(buildDir, 'client', 'route-manifest.json'), 'utf8'),
    )
    const href = Array.isArray(manifest?.styles) ? manifest.styles[0] : null
    if (typeof href !== 'string' || href === '') return ''
    return `<link rel="stylesheet" href="${href.replaceAll('"', '&quot;')}">`
  } catch {
    return ''
  }
}

/**
 * The head fragments a request-time render on a deployed build has to add.
 *
 * The build resolved both — the icon link from what it published, the plugin
 * entries from `ruvyxa.config.ts` — and recorded them in the deploy manifest,
 * because a deployed function has neither a `public/` directory to stat nor a
 * config to load. Read from the same `manifest.json` every adapter already
 * reads rather than recomputed here: `public_asset_links` and
 * `render_plugin_head` stay the only implementations of either rule.
 *
 * An older build has no such section, and the empty pair is exactly what this
 * path did before it existed.
 */
async function documentHeadDefaults(buildDir) {
  try {
    const manifest = JSON.parse(await readFile(path.join(buildDir, 'manifest.json'), 'utf8'))
    const head = manifest?.[DEPLOY_MANIFEST_KEY]?.documentHead
    return {
      assetLinks: typeof head?.assetLinks === 'string' ? head.assetLinks : '',
      pluginHead: typeof head?.pluginHead === 'string' ? head.pluginHead : '',
    }
  } catch {
    return { assetLinks: '', pluginHead: '' }
  }
}

async function loadClientAssets(buildDir) {
  let manifest
  try {
    manifest = JSON.parse(await readFile(path.join(buildDir, 'client', 'manifest.json'), 'utf8'))
  } catch {
    return new Map()
  }
  const assets = new Map()
  for (const route of Array.isArray(manifest?.routes) ? manifest.routes : []) {
    if (typeof route?.path !== 'string' || typeof route?.src !== 'string') continue
    assets.set(route.path, {
      src: route.src,
      preloads: (Array.isArray(route.sharedChunks) ? route.sharedChunks : [])
        .map((chunk) => chunk?.src)
        .filter((src) => typeof src === 'string'),
      hydration: typeof route.hydration === 'string' ? route.hydration : 'load',
      hydrationLoader: typeof route.hydrationLoader === 'string' ? route.hydrationLoader : null,
    })
  }
  return assets
}

/**
 * Build the two `react-server` artifacts one server-components route needs.
 *
 * A server-components render is two passes over two module graphs that never
 * share a React instance, and both graphs are compiled — the server one with
 * the `react-server` export condition so React's server build is linked into
 * it, the registry with the ordinary React left external so the SSR pass
 * renders client components with the instance `react-dom/server` is using.
 * `worker-pool.mjs` builds the same two for `ruvyxa dev` and `ruvyxa start`;
 * this builds them once, at build time, for a deployed function.
 *
 * Not shared with that host's builders because what differs is the *caching*:
 * the worker keys them in an LRU with build locks because it rebuilds on every
 * edit, and a build runs once. What must not differ is the compile options and
 * the reference base, so both read `rscReferenceBase` and the entry sources
 * from the modules that own them.
 *
 * Returned as absolute paths. The registry compile inlines both, because a
 * deployed function directory resolves no sibling specifiers.
 */
async function buildServerComponentBundles(route, pageFile, index) {
  const appDir = projectAppDir()
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'rsc-deploy')
  await mkdir(cacheDir, { recursive: true })

  // The one position the project's own tree and a staged copy share, so a
  // reference id names the same module in the browser bundle the Rust client
  // build emitted and in the payload this bundle will produce.
  const referenceBase = path.dirname(path.resolve(appDir))
  const routeDir = path.dirname(pageFile)
  const layouts = collectLayouts(appDir, routeDir)
  const templates = collectTemplates(appDir, routeDir)
  const slots = collectSlots(appDir, routeDir)
  const specials = collectSpecials(appDir, routeDir)
  const routePath = route.path ?? '/'

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const {
    imports: wrapperImports,
    layoutNames,
    levels,
  } = wrapperEntryParts(layouts, templates, slots)
  imports.push(...wrapperImports)
  if (specials.loading) {
    imports.push(`import RouteLoading from ${JSON.stringify(toImportPath(specials.loading))}`)
  }
  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const serverFile = path.join(cacheDir, `server.${index}.mjs`)
  const server = await compileBundleWithMetadata({
    projectRoot,
    entrySource: rscServerEntrySource({
      imports,
      pageName: 'Page',
      layoutNames,
      levels,
      routePath,
      loadingName: specials.loading ? 'RouteLoading' : null,
      metaNames,
    }),
    sourcefile: 'ruvyxa:rsc-server.tsx',
    outfile: serverFile,
    platform: serverPlatform(),
    bundleTarget: 'react-server',
    clientReferenceBase: referenceBase,
    // The one graph in this framework that carries its dependencies. Left
    // external, `react` would resolve through the host's resolver, which has no
    // way to know this module wants the `react-server` build.
    bundlePackages: true,
    nodeEnv: 'production',
    aliases: runtimeAliases(runtimeDir),
  })

  const { imports: registryImports, statements } = clientRegistrySource(server.clientReferences)
  const registryFile = path.join(cacheDir, `registry.${index}.mjs`)
  const registry = await compileBundleWithMetadata({
    projectRoot,
    entrySource: `${registryImports.join('\n')}\n${statements.join('\n')}\n`,
    sourcefile: 'ruvyxa:rsc-registry.tsx',
    outfile: registryFile,
    platform: serverPlatform(),
    // Client modules are lifted out of their packages and inlined here, so a
    // bare specifier left behind would resolve from this directory instead of
    // from the package that wrote it.
    bundlePackages: true,
    // React and the DOM renderer stay external so these components share the
    // instance `react-dom/server` renders them with. Bundling either would put
    // two copies in one render and every hook would throw.
    external: ['react', 'react/jsx-runtime', 'react-dom', 'react-dom/client', 'react-dom/server'],
    // This bundle is the *client* side of the boundary that happens to run on a
    // server, so a `'use server'` module it reaches becomes a reference here
    // exactly as it does in the browser — which is what makes a
    // `<form action={fn}>` render the same hidden fields in both.
    serverReferenceClient: RSC_SSR_PACKAGE,
    clientReferenceBase: referenceBase,
    aliases: runtimeAliases(runtimeDir),
    // This bundle is linked *into* the route registry, because that is the only
    // way it can share the React instance `react-dom/server` renders with —
    // left as a sibling file it would resolve its own copy and every hook threw
    // `Cannot read properties of null (reading 'useRef')`. Two bundles that
    // both number their modules `__m0` upward cannot share a scope, so this one
    // is numbered differently.
    identifierPrefix: `__rsc${index}_`,
    // Pinned here too, even though this bundle is linked into one that already
    // pins it. The outer pin is a *runtime* assignment to `globalThis.process`,
    // and on an edge target there is no `process` to assign to — the literal
    // compiled into each module wrapper is the only `NODE_ENV` its code will
    // ever read. Reasoning that the outer bundle covered this left every
    // Cloudflare worker's SSR pass running the client modules it inlines under
    // `NODE_ENV: "development"`, which is where a component library's dev-only
    // branches live. The duplicated statement in the combined output is
    // idempotent.
    nodeEnv: 'production',
  })

  // Every `'use server'` module either graph can reach. Both lists are needed:
  // an actions file the page imports is in the `react-server` graph and nowhere
  // else, and one imported only by a `'use client'` component is in the
  // registry's graph and nowhere else, because a reference's own imports are
  // never walked by the server graph.
  const serverReferences = mergeServerReferences(server.serverReferences, registry.serverReferences)
  let actionFile = null
  if (serverReferences.length > 0) {
    actionFile = path.join(cacheDir, `action.${index}.mjs`)
    await compileBundleWithMetadata({
      projectRoot,
      entrySource: rscActionEntrySource({ references: serverReferences }),
      sourcefile: 'ruvyxa:rsc-action.tsx',
      outfile: actionFile,
      platform: serverPlatform(),
      // The functions run in the realm the page rendered in, so this bundle
      // carries the `react-server` build for the same reason the render entry
      // does: a function that returns an element tree must produce one the
      // page's own React can serialise.
      bundleTarget: 'react-server',
      clientReferenceBase: referenceBase,
      bundlePackages: true,
      nodeEnv: 'production',
      aliases: runtimeAliases(runtimeDir),
    })
  }

  return {
    serverFile,
    registryFile,
    actionFile,
    // The server bundle stays a sibling: it carries its own React — the
    // `react-server` build, which must *not* be the renderer's instance — so it
    // has nothing to share and no reason to be re-linked.
    serverSpecifier: `./rsc/server.${index}.mjs`,
    // Same reasoning, and it is loaded only when a call arrives, so it is
    // imported lazily rather than at module scope.
    actionSpecifier: actionFile ? `./rsc/action.${index}.mjs` : null,
    references: server.clientReferences,
  }
}

/** The project's app directory, honouring a configured `appDir`. */
function projectAppDir() {
  const configured = projectConfig?.appDir
  return path.resolve(
    projectRoot,
    typeof configured === 'string' && configured ? configured : 'app',
  )
}

async function materializeRouteModules(manifest, destination, target, buildDir) {
  const routes = Array.isArray(manifest.routes) ? manifest.routes : []
  const clientAssets = await loadClientAssets(buildDir)
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
    // The document writers a deployed render needs. Nothing else in the
    // deployment has them: `client_hydration_script` and
    // `inject_prerender_client_assets` are both Rust, and this registry is the
    // renderer once the build is over.
    definitions.push(
      documentAssetsPrelude(await styleHeadTag(buildDir), await documentHeadDefaults(buildDir)),
    )
    // The shared streaming-render policy: a suspended child that rejects
    // must not take the whole document down, which is what every other
    // Ruvyxa host already did.
    definitions.push(documentStreamPrelude())
    // Imported only when a route needs it, so an app with no server-components
    // route ships neither the renderer nor the second React it links.
    if (routes.some((route) => route?.render?.serverComponents === true)) {
      imports.push(
        `import { callRouteServerFunction as __ruvyxaCallServerFunction, renderServerComponents as __ruvyxaRenderServerComponents, renderServerComponentsStream as __ruvyxaRenderServerComponentsStream, runRouteFormAction as __ruvyxaRunFormAction } from ${JSON.stringify(RSC_RENDERER_SPECIFIER)}`,
      )
    }
  }

  // Server actions live in `action.ts` beside the page they belong to and are
  // absent from the route manifest, so they are discovered here the same way
  // the native server resolves them at request time. Without this the compiled
  // registry had no way to reach them and `POST /__ruvyxa/action` could only
  // ever 404 in a deployed build.
  const actionRecords = []

  /** Pre-linked server-components artifacts to copy beside the registry. */
  const serverComponentBundles = []

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

    const serverComponents =
      route.render?.serverComponents === true
        ? await buildServerComponentBundles(route, routeFile, index)
        : null
    if (serverComponents) serverComponentBundles.push(serverComponents)

    const page = pageRouteDefinition(routeFile, index, route.path ?? '/', route.cache === true, {
      // Null for a route that ships no bundle (`export const hydrate = false`)
      // and for one the client build skipped: the render then adds no script,
      // which is what those routes want.
      clientAssets: clientAssets.get(route.path ?? '/') ?? null,
      serverComponents,
    })
    imports.push(...page.imports)
    definitions.push(page.definition)
    records.push(
      `  ${JSON.stringify(route.id)}: { render: ${page.renderName}, flight: ${page.flightName}` +
        // Present only on a server-components route; the handler reads its
        // absence as "this route has no payload to give".
        `${page.payloadName ? `, rscPayload: ${page.payloadName}` : ''}` +
        // Present only when that route also declares server functions, which
        // is what tells the handler a `POST` to `/__ruvyxa/rsc` can be answered
        // rather than refused.
        `${page.actionName ? `, rscAction: ${page.actionName}` : ''} }`,
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
  // Every pre-linked server-components artifact, copied beside the registry and
  // named external so the compile emits the import rather than inlining it.
  const rscExternals = []
  for (const bundle of serverComponentBundles) {
    for (const [specifier, source] of [
      [bundle.serverSpecifier, bundle.serverFile],
      [bundle.actionSpecifier, bundle.actionFile],
    ]) {
      if (!specifier) continue
      const destinationFile = path.join(destination, ...specifier.slice('./'.length).split('/'))
      await mkdir(path.dirname(destinationFile), { recursive: true })
      await cp(source, destinationFile)
      rscExternals.push(specifier)
    }
  }
  await compileRegistry(buildSource(plugins), outfile, target, rscExternals)

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
      await compileRegistry(buildSource(withoutPlugins), outfile, target, rscExternals)
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
      await compileRegistry(buildSource(plugins), outfile, target, rscExternals)
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

async function compileRegistry(entrySource, outfile, target, external = []) {
  await compileBundle({
    projectRoot,
    entrySource,
    sourcefile: 'ruvyxa:serverless-route-registry.tsx',
    outfile,
    platform: target === 'edge' ? 'browser' : serverPlatform(),
    // `platform` says which host runs the artifact; `bundleTarget` says which
    // `exports` conditions apply, and an edge artifact needs both answers. It
    // is compiled as `browser` because a Worker has no Node resolver at
    // runtime, but it must read `worker`/`edge-light` rather than `browser` —
    // stated here because the default derives `client` from `platform`.
    bundleTarget: target === 'edge' ? 'edge' : 'ssr',
    bundlePackages: true,
    // Every bundle in a deployment states the build it is, rather than reading
    // one off whatever started the process. See `nodeEnvPrelude` in the
    // compiler: an edge artifact has no `process` at all and took the
    // stand-in's literal, and a Node artifact took whatever the host exported —
    // which for the documented `node server/index.mjs` is nothing.
    nodeEnv: 'production',
    // Sibling artifacts this registry imports rather than contains. See the
    // server-components branch of `pageRouteDefinition` for why a finished
    // bundle must not be re-linked into this one.
    external,
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
    // A deployed function only ever serves production traffic.
    environment: 'production',
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

function pageRouteDefinition(
  pageFile,
  routeIndex,
  routePath = '/',
  cacheFlight = false,
  { clientAssets = null, serverComponents = null } = {},
) {
  const appDir = projectAppDir()
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const pageName = `Page${routeIndex}`
  const moduleName = `PageModule${routeIndex}`
  const renderName = `renderPage${routeIndex}`
  const payloadName = `rscPayload${routeIndex}`
  const actionName = `rscAction${routeIndex}`
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

  // The route's browser assets, frozen into the module. `null` means the route
  // ships no bundle, and the render then adds no script rather than a broken
  // one — the same branch `client_hydration_script` takes for `hydrate = false`.
  const assetsLiteral = clientAssets === null ? 'null' : JSON.stringify(clientAssets)

  // A server-components route renders through the other pipeline entirely: the
  // page and its layouts never reach `react-dom/server` here, so none of the
  // tree, boundary, or meta machinery below applies to it. Its meta is resolved
  // inside the `react-server` graph by `rscServerEntrySource`.
  if (serverComponents) {
    const serverAlias = `RscServer${routeIndex}`
    return {
      // Sibling specifiers, left external by `compileRegistry` and copied into
      // the function directory beside this module. Not inlined: both files are
      // *already linked* bundles, and the linker names its modules `__m<N>`
      // from zero, so inlining one into another puts a second `const __m1` in
      // the same scope as the first. The inner declaration shadows the outer,
      // and the outer module's own reference to it hits the temporal dead zone
      // — the whole deployment failed to import with
      // `Cannot access '__m1' before initialization`. A finished bundle is an
      // artifact, not a source file; it gets copied, not re-linked.
      imports: [
        `import * as ${serverAlias} from ${JSON.stringify(serverComponents.serverSpecifier)}`,
        // Inlined, not a sibling: evaluating the registry is what registers
        // each client module under the id the payload names, and it has to do
        // that with the React instance this module renders with.
        `import ${JSON.stringify(toImportPath(serverComponents.registryFile))}`,
      ],
      definition: `async function ${renderName}(ctx) {${
        serverComponents.actionSpecifier
          ? `
  // A \`<form action={fn}>\` submitted without JavaScript posts here, with the
  // reference in hidden fields. The action runs before the render so the page
  // can show what it returned, which is what \`useActionState\` replays.
  const posted = ctx?.formData
    ? await __ruvyxaRunFormAction({ actionModule: await ${actionName}Bundle(), formData: ctx.formData })
    : null`
          : ''
      }
  // Only when the caller asked, and it asks only where nothing is stored — see
  // the same guard on the non-server-components render. The payload is a
  // *promise* here: it is complete when the Flight render is, which is long
  // after the first bytes have gone out, and the browser needs it before
  // hydration rather than before the first paint.
  if (ctx?.stream === true) {
    const streamed = await __ruvyxaRenderServerComponentsStream({
      serverModule: ${serverAlias},
      references: ${JSON.stringify(serverComponents.references)},
      ctx,
      routePath: ${JSON.stringify(routePath)},${
        serverComponents.actionSpecifier ? `\n      formState: posted?.formState ?? null,` : ''
      }
    })
    const streamHead = __ruvyxaDocumentAssets(${assetsLiteral}, ctx, null).head
    const streamTail = streamed.payload.then((payload) => {
      for (const failure of streamed.failures) {
        // Reported, never thrown: the response left before this was known, and
        // the only thing left to do with it is put it in the log.
        console.error("[ruvyxa] server component failed after the shell was sent", failure)
      }
      return __ruvyxaDocumentAssets(${assetsLiteral}, ctx, payload).tail
    })
    return __ruvyxaDocumentStream(streamed.stream, streamHead, streamTail, null)
  }

  const rendered = await __ruvyxaRenderServerComponents({
    serverModule: ${serverAlias},
    references: ${JSON.stringify(serverComponents.references)},
    ctx,
    routePath: ${JSON.stringify(routePath)},${
      serverComponents.actionSpecifier ? `\n    formState: posted?.formState ?? null,` : ''
    }
    // Serving a request, not building a page: a suspended child that rejects
    // after the shell has rendered is answered by its own error boundary, and
    // the rest of the document is correct. Failing the whole render here is
    // what made a route that streams fine locally answer 500 in production.
    tolerateStreamErrors: true,
  })
  const assets = __ruvyxaDocumentAssets(${assetsLiteral}, ctx, rendered.payload)
  return __ruvyxaFinishDocument(rendered.html, assets.head, assets.tail, null)
}

/**
 * The payload alone, for a soft navigation into this route.
 *
 * The browser already has a document, so rendering markup it would throw away
 * would run every server component for nothing — \`html: false\` skips the SSR
 * pass and the registry lookups it needs.
 */
async function ${payloadName}(ctx) {
  const rendered = await __ruvyxaRenderServerComponents({
    serverModule: ${serverAlias},
    references: ${JSON.stringify(serverComponents.references)},
    ctx,
    routePath: ${JSON.stringify(routePath)},
    html: false,
    // Same reason as the document render above: the payload carries the error
    // row, and the browser renders the boundary from it.
    tolerateStreamErrors: true,
  })
  return rendered.payload
}${
        serverComponents.actionSpecifier
          ? `

/**
 * This route's server functions, loaded on the first call that needs them.
 *
 * Imported lazily rather than at module scope: the bundle carries its own React
 * — the \`react-server\` build again — and most requests to a deployment
 * neither call a server function nor submit a form.
 */
let ${actionName}Module = null
async function ${actionName}Bundle() {
  ${actionName}Module ??= await import(${JSON.stringify(serverComponents.actionSpecifier)})
  return ${actionName}Module
}

/** Run one of this route's server functions, for a \`POST /__ruvyxa/rsc\`. */
async function ${actionName}({ reference, body }) {
  return await __ruvyxaCallServerFunction({
    actionModule: await ${actionName}Bundle(),
    references: ${JSON.stringify(serverComponents.references)},
    reference,
    body,
  })
}`
          : ''
      }`,
      renderName,
      payloadName,
      actionName: serverComponents.actionSpecifier ? actionName : null,
      moduleName,
      flightName: 'null',
    }
  }

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
  const assets = __ruvyxaDocumentAssets(${assetsLiteral}, ctx, null)
  const lang = __ruvyxaResolveMeta([${metaNames.join(', ')}], ctx).lang

  // Only when the caller asked, and it asks only where nothing is stored: a
  // pre-render, an ISR entry, and the \`requestScoped\` check that guards them
  // all need the finished string, and a document still being written has no
  // finished anything.
  if (ctx?.stream === true) {
    const stream = await __ruvyxaRenderDocumentStreamOnly(tree)
    if (stream !== null) return __ruvyxaDocumentStream(stream, assets.head, assets.tail, lang)
  }

  const html = await __ruvyxaRenderDocumentHtml(tree)
  return __ruvyxaFinishDocument(html, assets.head, assets.tail, lang)
}${cachedFlight}`
  return { imports, definition, renderName, payloadName: null, moduleName, flightName }
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

/**
 * Write a final response and leave, without racing the write against the exit.
 *
 * Stdout here is a pipe read by the Rust host, and a write to a pipe is
 * asynchronous: `process.exit()` tears the process down without draining one
 * that has not flushed, so `writeResponse()` followed by `process.exit(1)`
 * could drop the very diagnostic that explains why the run failed, leaving the
 * host to report unparsable output instead. Writing straight to fd 1 removes
 * the race rather than narrowing it. Every other exit path sets
 * `process.exitCode` and returns, which lets Node drain stdout on its own.
 */
function exitWithResponse(response, code) {
  writeSync(1, JSON.stringify(response))
  process.exit(code)
}
