import { createHash } from 'node:crypto'
import { existsSync, readFileSync, realpathSync, statSync } from 'node:fs'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createRequire, isBuiltin } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import {
  RSC_CLIENT_RUNTIME_SPECIFIER,
  clientModuleId,
  clientProxyModuleSource,
  serverModuleId,
  serverProxyModuleSource,
  serverRegistrationSource,
} from './client-references.mjs'
import { compareCodeUnits } from './order.mjs'
import {
  isSafePackageRelativePath,
  legacyEntryCandidates,
  packageNameAndExportKey,
  PACKAGE_EXPORT_TARGETS,
  resolveExportsEntry,
} from './package-exports.mjs'
import { createCodeIndex, directivePrologueEnd, findInCode, maskNonCode } from './scanner.mjs'
const JS_EXTENSIONS = ['', '.ts', '.tsx', '.js', '.jsx', '.mts', '.mjs', '.md', '.mdx']
export const MDX_COMPONENT_EXTENSIONS = ['.tsx', '.ts', '.jsx', '.js', '.mts', '.mjs']
const ASSET_EXTENSIONS = new Set(['.css', '.scss', '.sass', '.less'])
/// Every file extension this compiler knows how to turn into a module. A
/// resolved file outside this set has no compilation path, so it is rejected by
/// name instead of being handed to the JavaScript transform, which would report
/// the mismatch as an unrelated syntax error in someone else's package.
/// An empty extension stays allowed: package entry points without one are
/// JavaScript by Node's own rules.
const MODULE_KIND_EXTENSIONS = new Set([
  '.ts',
  '.tsx',
  '.js',
  '.jsx',
  '.mts',
  '.cts',
  '.mjs',
  '.cjs',
  '.md',
  '.mdx',
  '.json',
  '.css',
  '.scss',
  '.sass',
])
const COMPILER_CACHE_MAX_ENTRIES = 512
const compilerCache = (globalThis.__RUVYXA_COMPILER_CACHE__ ??= {
  sources: new Map(),
  transforms: new Map(),
  rewrites: new Map(),
  content: new Map(),
  markdownConfigurations: new Map(),
})
compilerCache.transforms ??= new Map()
compilerCache.markdownConfigurations ??= new Map()

/**
 * Drop compiler entries associated with changed files, or every entry when the
 * caller cannot identify the change. Worker invalidation and memory-pressure
 * handling call this before compiling another bundle.
 */
export function invalidateCompilerCache(paths) {
  if (!paths || paths.length === 0) {
    clearCompilerCache()
    return
  }

  const normalizedPaths = new Set(paths.map((file) => path.resolve(file)))
  for (const key of compilerCache.sources.keys()) {
    if (normalizedPaths.has(key)) compilerCache.sources.delete(key)
  }
  // Rewrite keys embed module keys and dependency aliases, so selectively
  // removing them is less reliable than rebuilding these bounded derivations.
  compilerCache.rewrites.clear()
}

export function clearCompilerCache() {
  compilerCache.sources.clear()
  compilerCache.transforms.clear()
  compilerCache.rewrites.clear()
  compilerCache.content.clear()
  compilerCache.markdownConfigurations.clear()
}

export function compilerCacheStats() {
  return {
    sources: compilerCache.sources.size,
    transforms: compilerCache.transforms.size,
    rewrites: compilerCache.rewrites.size,
    content: compilerCache.content.size,
    markdownConfigurations: compilerCache.markdownConfigurations.size,
    maxEntries: COMPILER_CACHE_MAX_ENTRIES,
  }
}

export function collectLayouts(appDir, routeDir) {
  const layouts = []
  let current = appDir

  pushIfExists(layouts, path.join(current, 'layout.tsx'))

  const relative = path.relative(appDir, routeDir)
  if (relative && !relative.startsWith('..')) {
    for (const segment of relative.split(path.sep)) {
      if (!segment) continue
      current = path.join(current, segment)
      pushIfExists(layouts, path.join(current, 'layout.tsx'))
    }
  }

  return layouts
}

/** File names of the special files a segment may declare, by kind. */
const SPECIAL_FILES = { error: 'error.tsx', loading: 'loading.tsx', notFound: 'not-found.tsx' }

/**
 * Resolve the special files that apply to the route at `routeDir`.
 *
 * Each kind resolves nearest-wins: walking from the app root down to the route
 * directory, the deepest segment that declares the file owns it — the same
 * rule Next.js uses for `error.tsx` / `loading.tsx` / `not-found.tsx`. Returns
 * `{ error, loading, notFound }` with an absolute path or `null` per kind.
 *
 * The Rust build path mirrors this in `resolve_route_specials`
 * (`crates/ruvyxa_cli/src/main.rs`); keep the file names and the deepest-wins
 * rule in step.
 */
export function collectSpecials(appDir, routeDir) {
  const specials = { error: null, loading: null, notFound: null }

  const dirs = [appDir]
  const relative = path.relative(appDir, routeDir)
  if (relative && !relative.startsWith('..')) {
    let current = appDir
    for (const segment of relative.split(path.sep)) {
      if (!segment) continue
      current = path.join(current, segment)
      dirs.push(current)
    }
  }

  for (const dir of dirs) {
    for (const [kind, fileName] of Object.entries(SPECIAL_FILES)) {
      const candidate = path.join(dir, fileName)
      if (existsSync(candidate)) specials[kind] = candidate
    }
  }

  return specials
}

export async function compileBundle(options) {
  return (await compileBundleWithMetadata(options)).outfile
}

/** Return the active server runtime without changing browser bundle semantics. */
export function serverPlatform() {
  const runtime = process.env.RUVYXA_RUNTIME
  return runtime === 'bun' || runtime === 'deno' ? runtime : 'node'
}

/** Compile a bundle and return a stable fingerprint of its project-local inputs. */
export async function compileBundleWithMetadata({
  projectRoot,
  entrySource,
  sourcefile = 'ruvyxa:entry.ts',
  outfile,
  platform = 'node',
  bundleTarget,
  clientReferenceBase,
  serverReferenceClient = null,
  externalUrls,
  bundlePackages = false,
  bundleAliasDependencies = false,
  external = [],
  aliases = {},
  minify = false,
  sourceMap = true,
  jsxRuntime = process.env.RUVYXA_JSX_RUNTIME ?? 'automatic',
  reactCompiler = false,
  markdownConfig,
}) {
  const { loadTsconfigPaths } = await loadCompilerSupport()
  const normalizedJsxRuntime = normalizeJsxRuntime(jsxRuntime)
  const resolvedBundleTarget = normalizeBundleTarget(bundleTarget, platform)
  const root = path.resolve(projectRoot)
  // The directory client-reference ids are measured from. It is *not* the
  // project root, because the same module is compiled from two trees: the
  // project's own sources during `ruvyxa build`, and the copy the build stages
  // under `<out>/server/` which `ruvyxa start` serves from. Measured from the
  // root those two give different paths — `app/x.tsx` and
  // `.ruvyxa/server/app/x.tsx` — and therefore different ids, so the payload a
  // running server rendered named a reference the browser bundle never
  // registered. The caller passes the directory holding the app directory,
  // which is the position both trees share.
  const referenceBase = clientReferenceBase ? path.resolve(clientReferenceBase) : root
  const modules = []
  const byKey = new Map()
  // Every `'use client'` module the server-components graph turned into a
  // reference. Empty for every other bundle target, so nothing else changes.
  const clientReferences = new Map()
  // Every `'use server'` module this graph reached, whichever side of the
  // boundary it was compiled for. A client graph only fills this when
  // `serverReferenceClient` names the package its references are made with;
  // the `react-server` graph always does, because a server function it can see
  // is one this build must be able to call.
  const serverReferences = new Map()
  const externals = new Map()
  // A rewritten specifier is external by definition: the bundle imports it by
  // URL instead of inlining it, so the caller does not have to list it twice.
  const externalSet = new Set([...external, ...Object.keys(externalUrls ?? {})])
  const tsconfigPaths = loadTsconfigPaths(root)
  const entryKey = sourcefile

  await visitModule({
    key: entryKey,
    filePath: null,
    source: entrySource,
    sourcefile,
    baseDir: root,
    root,
    modules,
    byKey,
    externals,
    externalSet,
    externalUrls,
    aliases,
    platform,
    bundleTarget: resolvedBundleTarget,
    clientReferences,
    serverReferences,
    serverReferenceClient,
    referenceBase,
    bundlePackages,
    bundleAliasDependencies,
    bundleDependencies: false,
    jsxRuntime: normalizedJsxRuntime,
    reactCompiler,
    markdownConfig,
    tsconfigPaths,
  })

  const linked = linkModules(modules, externals, { minify, outfile, sourceMap, externalUrls })
  await mkdir(path.dirname(outfile), { recursive: true })
  await writeIfChanged(outfile, linked.code)
  if (linked.map) {
    await writeIfChanged(`${outfile}.map`, JSON.stringify(linked.map))
  }
  return {
    outfile,
    // Every `'use client'` module this graph turned into a reference, in a
    // stable order. Empty for every bundle target but `react-server`, so no
    // existing caller sees a change. The client build reads it to know which
    // modules need their own browser chunk, and the render reads it to build
    // the manifest React serialises references against.
    clientReferences: [...clientReferences.values()].sort((left, right) =>
      compareCodeUnits(left.id, right.id),
    ),
    // Every `'use server'` module, in the same stable order and for the same
    // reasons: the host reads it to know which files a server-function call may
    // resolve to, and a call naming a module no graph reported is refused
    // rather than loaded.
    serverReferences: [...serverReferences.values()].sort((left, right) =>
      compareCodeUnits(left.id, right.id),
    ),
    // Hash of the emitted bundle, used as the ESM import version token. Two
    // builds that emit identical code must resolve to the same import URL:
    // Node never releases a module URL once it is loaded, so a token that
    // changes on every rebuild retains one module graph per rebuild for the
    // life of the process.
    contentHash: createHash('sha256').update(linked.code).digest('hex').slice(0, 16),
    dependencyHash: await fingerprintProjectInputs(root, modules, tsconfigPaths.files),
    inputs: projectInputPaths(root, modules, tsconfigPaths.files),
    // Every file whose contents feed `dependencyHash`, so a caller can decide
    // whether a cached result is still valid without recompiling to find out.
    // Built from the same two sources the hash is, so the list can never claim
    // fewer inputs than the hash actually covers.
    fingerprintInputs: [
      ...projectModulePaths(root, modules),
      ...existingManifestFiles(root),
      ...tsconfigPaths.files.map((file) => path.relative(root, file).replaceAll('\\', '/')),
    ].sort(),
    // Every file this compile actually read, project-local or not, plus the
    // manifests and tsconfigs that took part in resolving them.
    //
    // Deliberately wider than `fingerprintInputs`, which is project-relative
    // because it answers "did the application change". Reusing a compiled
    // bundle asks the different question "could these bytes still be
    // produced", and for a config bundle the answer lives mostly outside the
    // project: 89 of a demo config's 94 inputs are the framework's own modules.
    readFiles: [
      ...new Set([
        ...modules
          .filter((module) => module.filePath)
          .map((module) => path.resolve(module.filePath)),
        ...existingManifestFiles(root).map((name) => path.join(root, name)),
        ...tsconfigPaths.files.map((file) => path.resolve(file)),
      ]),
    ].sort(compareCodeUnits),
    // Files that were looked for and not found. A lockfile or a tsconfig
    // appearing changes what a bare specifier resolves to without changing any
    // file that was read, so their absence is part of what was observed.
    absentFiles: [
      ...PROJECT_MANIFEST_FILES,
      ...(tsconfigPaths.files.length === 0 ? ['tsconfig.json', 'jsconfig.json'] : []),
    ]
      .map((name) => path.join(root, name))
      .filter((file) => !existsSync(file))
      .sort(compareCodeUnits),
  }
}

/**
 * Compile a bundle, or reuse the previous compile when nothing it read changed.
 *
 * [`compileBundleWithMetadata`] cannot say the output is still current without
 * walking and compiling the whole graph — which is the work a caller wanted to
 * skip. `writeIfChanged` then finds the bytes identical and writes nothing, so
 * the whole cost bought an answer that was already sitting on disk. Booting the
 * plugin host for the demo spent 341ms of a 964ms warm build exactly this way,
 * on every build, and the dev server pays it again at startup.
 *
 * The manifest beside the output records what the last compile read and what
 * those files hash to together, which answers the question by reading them
 * instead of recompiling them. Recorded metadata is replayed verbatim, so a
 * reused bundle reports the same `dependencyHash` a fresh one would — callers
 * key their own caches on it.
 *
 * A manifest that is missing, unparseable, or no longer matching is a miss and
 * nothing more: the compile runs and rewrites it. The one change this cannot
 * see is a file appearing where none was before and capturing a specifier that
 * already resolved elsewhere, which is the same bound the other
 * content-addressed caches here carry.
 */
export async function compileBundleIfChanged(options) {
  const manifestFile = `${options.outfile}.inputs.json`
  const reused = await reusableBundle(manifestFile, options.outfile)
  if (reused) return reused

  const bundle = await compileBundleWithMetadata(options)
  await writeIfChanged(
    manifestFile,
    JSON.stringify({
      compiler: BUNDLE_INPUT_MANIFEST_IDENTITY,
      hash: await fingerprintFiles(bundle.readFiles),
      metadata: bundle,
    }),
  )
  return bundle
}

/**
 * Identity of the compiler that wrote a bundle-input manifest.
 *
 * Derived from this module's own bytes rather than a literal. The constant it
 * replaced was documented as "bump it when the meaning of a recorded field
 * changes", which holds only while somebody remembers — and forgetting is
 * silent: the build replays metadata recorded under rules that no longer
 * apply. The file that defines the format is exactly the thing that changes
 * when the format does, so hashing it answers the question with nothing left
 * to maintain. Computed once per process, off a module that is by definition
 * readable because it is running.
 */
const BUNDLE_INPUT_MANIFEST_IDENTITY = createHash('sha256')
  .update(readFileSync(fileURLToPath(import.meta.url)))
  .digest('hex')
  .slice(0, 16)

/** The recorded metadata, if the output and every file behind it still hold. */
async function reusableBundle(manifestFile, outfile) {
  if (!existsSync(outfile)) return null
  let manifest
  try {
    manifest = JSON.parse(await readFile(manifestFile, 'utf8'))
  } catch {
    return null
  }
  if (manifest?.compiler !== BUNDLE_INPUT_MANIFEST_IDENTITY) return null
  const { hash, metadata } = manifest
  if (typeof hash !== 'string' || !metadata) return null
  if (metadata.absentFiles?.some((file) => existsSync(file))) return null
  const current = await fingerprintFiles(metadata.readFiles ?? []).catch(() => null)
  return current === hash ? metadata : null
}

/**
 * Hash a set of files by path and content.
 *
 * Both sides of the cache go through here so they cannot compute the
 * fingerprint differently. Contents come from disk rather than from the
 * compiler's in-memory module sources, which have already been transformed and
 * so would never match a later read of the file itself.
 */
async function fingerprintFiles(files) {
  const hash = createHash('sha256')
  for (const file of files) {
    hash.update(file)
    hash.update('\0')
    hash.update(await readFile(file))
    hash.update('\0')
  }
  return hash.digest('hex')
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

/**
 * Whether a `process.env.NAME` read must not reach a browser bundle.
 *
 * The Rust bundler decides the same thing in `boundary::env_read_is_private`,
 * and `ruvyxa_graph` calls that one rather than keeping a third copy. This is
 * the Node half. Both are replayed against
 * `tests/fixtures/env-policy-conformance.json`, because the rule decides which
 * secrets may be serialized into a browser bundle and it has drifted before —
 * it was named in `AGENTS.md` as fixture-held while no such fixture existed.
 *
 * @param {string} name Environment variable name, exactly as written in source.
 * @returns {boolean} True when the read is private.
 * @public
 */
export function envReadIsPrivate(name) {
  return name !== 'NODE_ENV' && !name.startsWith('RUVYXA_PUBLIC_')
}

/** Match one leading module directive after a BOM, whitespace, and comments. */
export function hasModuleDirective(source, expected) {
  let index = source.charCodeAt(0) === 0xfeff ? 1 : 0
  while (index < source.length) {
    while (/\s/.test(source[index] ?? '')) index += 1
    if (source.startsWith('//', index)) {
      const end = source.indexOf('\n', index + 2)
      index = end === -1 ? source.length : end + 1
      continue
    }
    if (source.startsWith('/*', index)) {
      const end = source.indexOf('*/', index + 2)
      if (end === -1) return false
      index = end + 2
      continue
    }
    break
  }
  const quote = source[index]
  if (quote !== '"' && quote !== "'") return false
  const end = source.indexOf(quote, index + 1)
  return end !== -1 && source.slice(index + 1, end) === expected
}

/**
 * The lane a module belongs to: `client`, `server`, `action`, or `shared`.
 *
 * `declared_lane` in `crates/ruvyxa_bundler/src/references.rs` decides the same
 * thing, and both replay `tests/fixtures/module-lane-conformance.json`. They had
 * disagreed: the Rust bundler read the leading directive and then the file stem,
 * while this graph matched the single literal filename `server.ts`. So
 * `server.js` — the other half of the documented convention — every action
 * module, and every `'use server'` module were compiled into the browser bundle
 * under `ruvyxa dev` and refused by `ruvyxa build` with RUV1820. Server-only
 * source reached a browser during development and the error arrived only at the
 * end.
 *
 * The directive outranks the stem: a `server.ts` opening with `'use client'` is
 * a client module that happens to be named server.
 *
 * The stem rule is a **project** convention and applies to project files only.
 * `react-dom/server.js` is a package entry point that a browser bundle may
 * legitimately contain, and reading the convention into `node_modules` refused
 * it. A directive still counts wherever it appears: a dependency that declares
 * `'use server'` means it.
 *
 * @param {string} filePath Absolute path of the module.
 * @param {string} source Module source, before transformation.
 * @returns {'client' | 'server' | 'action' | 'shared'} The module's lane.
 * @public
 */
export function moduleLane(filePath, source) {
  if (hasModuleDirective(source, 'use client')) return 'client'
  if (hasModuleDirective(source, 'use server')) return 'action'
  if (isInstalledDependency(filePath)) return 'shared'
  // The stem, not the extension: the convention is a filename, and a project
  // writes it in whichever language it uses. `server.d.ts` has the stem
  // `server.d` and is a declaration file, not a server module.
  const base = path.basename(filePath)
  const stem = base.slice(0, base.length - path.extname(base).length)
  if (stem === 'client') return 'client'
  if (stem === 'server') return 'server'
  if (stem === 'action' || stem === 'actions') return 'action'
  return 'shared'
}

/** Whether a path lives inside an installed package rather than project source. */
function isInstalledDependency(filePath) {
  return filePath.split(/[\\/]/).includes('node_modules')
}

function projectInputPaths(root, modules, configurationFiles = []) {
  return [
    ...new Set(
      [
        ...modules.flatMap((module) => [module.filePath, ...(module.assetInputs || [])]),
        ...configurationFiles,
      ]
        .filter((file) => file && isWithinProject(root, file))
        .map((file) => path.relative(root, file).replaceAll('\\', '/')),
    ),
  ].sort()
}

async function fingerprintProjectInputs(root, modules, configurationFiles = []) {
  const hash = createHash('sha256')
  const projectModules = modules
    .filter((module) => module.filePath && isWithinProject(root, module.filePath))
    .map((module) => ({
      path: path.relative(root, module.filePath).replaceAll('\\', '/'),
      source: module.source,
    }))
    .sort((left, right) => compareCodeUnits(left.path, right.path))

  for (const module of projectModules) {
    hash.update(module.path)
    hash.update('\0')
    hash.update(module.source)
    hash.update('\0')
  }

  for (const fileName of PROJECT_MANIFEST_FILES) {
    const file = path.join(root, fileName)
    if (!existsSync(file)) continue
    hash.update(fileName)
    hash.update('\0')
    hash.update(await readFile(file))
    hash.update('\0')
  }

  for (const file of configurationFiles) {
    if (!existsSync(file)) continue
    hash.update(path.relative(root, file).replaceAll('\\', '/'))
    hash.update('\0')
    hash.update(await readFile(file))
    hash.update('\0')
  }

  return hash.digest('hex')
}

/**
 * Project manifests and lockfiles that participate in the dependency
 * fingerprint. A change to any of them can change what a bare specifier
 * resolves to, so they invalidate a compiled bundle even when no source file
 * was touched.
 */
const PROJECT_MANIFEST_FILES = [
  'package.json',
  'pnpm-lock.yaml',
  'package-lock.json',
  'yarn.lock',
  'bun.lock',
  'bun.lockb',
]

/** Project-relative paths of the modules that feed the dependency fingerprint. */
function projectModulePaths(root, modules) {
  return modules
    .filter((module) => module.filePath && isWithinProject(root, module.filePath))
    .map((module) => path.relative(root, module.filePath).replaceAll('\\', '/'))
}

function existingManifestFiles(root) {
  return PROJECT_MANIFEST_FILES.filter((fileName) => existsSync(path.join(root, fileName)))
}

/**
 * Names Ruvyxa accepts for the project instrumentation hook, in priority order.
 *
 * Shared by the Node worker and the adapter runner. They run the hook in
 * different processes — one lazily per worker, one as a top-level await in the
 * function bundle — but a project that works in `ruvyxa dev` and not after
 * deployment because only one side recognised `.mjs` would be a bad surprise,
 * so the list has one home.
 */
export const INSTRUMENTATION_FILES = Object.freeze([
  'instrumentation.ts',
  'instrumentation.js',
  'instrumentation.mjs',
])

/**
 * Where a development browser bundle imports its shared React from.
 *
 * A build gives every route one shared chunk, so a page holds one React no
 * matter how many route bundles it loads. Development compiles a bundle per
 * route on demand and has no such analysis, so each one inlined its own React —
 * and rendering a component from one copy into a root owned by another makes
 * every hook in it throw. Soft navigation therefore failed on the first route
 * change and the router fell back to a document load, which is why the pages
 * still worked and client-side routing quietly did nothing.
 *
 * Each module below is served separately and built with the *others* rewritten
 * the same way, so the browser ends up with exactly one instance of each.
 */
export const CLIENT_VENDOR_PATH = '/__ruvyxa/client/vendor'

/**
 * Global the shared browser modules are published on, keyed by specifier.
 *
 * Deliberately not `__RUVYXA_SHARED_MODULES__`, which a build's shared chunk
 * uses with module-id keys: the two never appear on one page, and one global
 * holding two key rules is a collision waiting for the day they do.
 */
export const VENDOR_REGISTRY_GLOBAL = '__RUVYXA_VENDOR_MODULES__'

/** The module source that publishes one shared browser module. */
export function clientVendorEntrySource(specifier) {
  return [
    `import * as __ruvyxaVendor from ${JSON.stringify(specifier)}`,
    `;(globalThis.${VENDOR_REGISTRY_GLOBAL} ??= {})[${JSON.stringify(specifier)}] = __ruvyxaVendor`,
    '',
  ].join('\n')
}

/**
 * The shared modules, keyed by the `name` their URL carries.
 *
 * A query parameter rather than a path segment, matching `/__ruvyxa/client`:
 * a parameterised route could not be named exactly in the reserved-path lists
 * that keep plugins from colliding with framework endpoints.
 */
export const CLIENT_VENDOR_MODULES = Object.freeze({
  react: 'react',
  'react-jsx-runtime': 'react/jsx-runtime',
  'react-dom': 'react-dom',
  'react-dom-client': 'react-dom/client',
})

// `scheduler` is deliberately absent. It is a transitive dependency of
// `react-dom/client` and of nothing else here, so the one module that needs it
// carries it — and it could not be a vendor entry anyway: under pnpm it is a
// sibling of react-dom inside the store and is not reachable from a project's
// own `node_modules` at all.

/**
 * Specifier-to-URL map for a bundle that should share these modules.
 *
 * `exclude` is the specifier the caller is *building*: a vendor module must
 * inline itself and externalise only its siblings, or it would import itself.
 */
export function clientVendorUrls(exclude = null) {
  const urls = {}
  for (const [name, specifier] of Object.entries(CLIENT_VENDOR_MODULES)) {
    if (specifier === exclude) continue
    urls[specifier] = `${CLIENT_VENDOR_PATH}?name=${name}`
  }
  return urls
}

/** The specifier one vendor `name` stands for, or `null` for an unknown name. */
export function clientVendorSpecifier(name) {
  return Object.hasOwn(CLIENT_VENDOR_MODULES, name) ? CLIENT_VENDOR_MODULES[name] : null
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
    'ruvyxa/plugin': preferExisting(
      path.join(packageRoot, 'src', 'plugin.ts'),
      path.join(packageRoot, 'dist', 'plugin.js'),
    ),
    'ruvyxa/plugins': preferExisting(
      path.join(packageRoot, 'src', 'plugins.ts'),
      path.join(packageRoot, 'dist', 'plugins.js'),
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
    '@ruvyxa/core/plugin': preferExisting(
      path.join(coreRoot, 'src', 'plugin.ts'),
      path.join(coreRoot, 'dist', 'plugin.js'),
    ),
    // Not a specifier any app writes: it is how a generated server-components
    // entry reaches the module that installs the two globals React resolves a
    // client reference through. An alias rather than a path because the file
    // lives outside the project, and a server target leaves such a path
    // external — emitting an absolute import no ESM loader accepts.
    [RSC_CLIENT_RUNTIME_SPECIFIER]: path.join(runtimeDir, 'rsc-client-runtime.mjs'),
  }
}

async function visitModule(context) {
  const { expandImportMetaGlob, resolveTsconfigPath } = await loadCompilerSupport()
  const {
    key,
    filePath,
    source,
    sourcefile,
    baseDir,
    root,
    modules,
    byKey,
    externals,
    externalSet,
    externalUrls,
    aliases,
    platform,
    bundleTarget,
    clientReferences,
    serverReferences,
    serverReferenceClient,
    referenceBase,
    bundlePackages,
    bundleAliasDependencies,
    bundleDependencies,
    jsxRuntime,
    reactCompiler,
    markdownConfig,
    tsconfigPaths,
  } = context

  if (byKey.has(key)) return byKey.get(key)

  const { clientReference, serverReference, moduleSource } = moduleBoundary({
    source,
    filePath,
    root,
    referenceBase,
    bundleTarget,
    clientReferences,
    serverReferences,
    serverReferenceClient,
  })

  const { jsonModule, styleModule, contentModule, compiledSource, globExpansion } =
    await classifyModuleSource(
      { source: moduleSource, filePath, root, baseDir, markdownConfig, tsconfigPaths },
      expandImportMetaGlob,
    )
  const id = `__m${modules.length}`
  const module = {
    id,
    key,
    filePath,
    sourceName:
      filePath ?? (sourcefile.startsWith('ruvyxa:') ? sourcefile : path.resolve(root, sourcefile)),
    source: globExpansion.source,
    baseDir,
    deps: new Map(),
    assetInputs: jsonModule
      ? []
      : [...(styleModule?.inputs ?? contentModule.inputs), ...globExpansion.inputs],
    jsxRuntime,
    reactCompiler,
  }
  byKey.set(key, module)
  modules.push(module)

  // A JSON module is data. It has no dependencies to walk, no client boundary to
  // check, and must never reach the JavaScript transform, so its compiled source
  // is also its transformed source.
  if (jsonModule) {
    module.transformedSource = compiledSource
    return module
  }

  // Not for a module this graph replaced with references. The check reads the
  // module's lane, and a `'use server'` file is in the action lane by both the
  // directive rule and — for the `actions.ts` name React's own convention
  // uses — the filename rule. Neither says anything about the proxy that took
  // its place: the server code is gone, and what is left is the reference
  // machinery the browser is supposed to have.
  if (platform === 'browser' && !serverReference) {
    checkClientBoundary(root, filePath, module.source)
  }

  // Inspect the transformed module so automatic JSX helper imports are linked
  // like ordinary dependencies. Oxc adds `react/jsx-runtime` during transform;
  // scanning only the source would otherwise drop those bindings in wrapped
  // Node bundles and leave `_jsx` undefined at render time.
  // React's other way of declaring a server function: the directive inside one
  // function's body rather than at the top of a whole module. Scanned after the
  // transform, on TypeScript-free source, so a return-type annotation between
  // the parameter list and the body cannot be misread as part of either.
  const transformedSource = withInlineServerFunctions(transformModuleSource(module), {
    filePath,
    root,
    referenceBase,
    bundleTarget,
    serverReferenceClient,
    serverReferences,
    alreadyRegistered: serverReference !== null || clientReference !== null,
  })
  module.transformedSource = transformedSource
  for (const specifier of extractSpecifiers(transformedSource)) {
    if (isAssetSpecifier(specifier) && !isCssModuleSpecifier(specifier)) continue

    // A rewritten specifier is answered before resolution, not after. A browser
    // bundle inlines everything it can reach — that is what `platform:
    // 'browser'` means — so asking `shouldBundleResolved` about React would
    // always bundle it, and the URL the caller supplied would never be emitted.
    if (externalUrls?.[specifier]) {
      registerExternalDependency(module, specifier, null, externals, externalUrls)
      continue
    }

    // A specifier the caller named external is answered here for the same
    // reason. `external` used to hold only by accident: a server bundle leaves
    // packages alone anyway, so nobody noticed the list was never consulted —
    // until one asked for `bundlePackages` as well, resolved its own React
    // despite listing it, and rendered client components against a second copy
    // whose dispatcher was null.
    if (externalSet.has(specifier)) {
      registerExternalDependency(module, specifier, null, externals, externalUrls)
      continue
    }

    const resolvedAlias = aliases[specifier]
    const resolved = resolveSpecifierPath(specifier, resolvedAlias, {
      baseDir,
      root,
      platform,
      bundleTarget,
      bundlePackages,
      bundleDependencies,
      tsconfigPaths,
      resolveTsconfigPath,
    })

    if (
      shouldBundleResolved(resolved, resolvedAlias, {
        root,
        platform,
        bundlePackages,
        bundleDependencies,
      })
    ) {
      assertSupportedModuleKind(resolved, specifier, filePath || sourcefile)
      const depSource = await readSourceFile(resolved)
      const dep = await visitModule({
        key: resolved,
        filePath: resolved,
        source: depSource,
        sourcefile,
        baseDir: path.dirname(resolved),
        root,
        modules,
        byKey,
        externals,
        externalSet,
        externalUrls,
        aliases,
        platform,
        bundleTarget,
        clientReferences,
        serverReferences,
        serverReferenceClient,
        referenceBase,
        bundlePackages,
        bundleAliasDependencies,
        bundleDependencies:
          bundleDependencies || (bundleAliasDependencies && Boolean(resolvedAlias)),
        jsxRuntime,
        reactCompiler,
        markdownConfig,
        tsconfigPaths,
      })
      module.deps.set(specifier, dep)
      continue
    }

    registerExternalDependency(module, specifier, resolvedAlias, externals, externalUrls)

    if (!externalSet.has(specifier) && specifier.startsWith('.')) {
      throw new Error(`RUV1801 cannot resolve '${specifier}' from ${filePath || sourcefile}`)
    }
  }

  return module
}

/**
 * Which side of the React boundary a module is on, and what to compile for it.
 *
 * The two directives are mirror images. A `'use client'` module belongs to the
 * browser, so the server graph gets a proxy and never walks its imports — or it
 * would pull in browser-only code and a `useState` the react-server build does
 * not have. A `'use server'` module belongs to the server, so it is compiled
 * for real there and every client graph gets references instead.
 *
 * Decided before classification rather than after, because for both of them the
 * substituted source is what the rest of the pipeline must see: its dependency
 * walk, its lane check, and its transform.
 */
function moduleBoundary({
  source,
  filePath,
  root,
  referenceBase,
  bundleTarget,
  clientReferences,
  serverReferences,
  serverReferenceClient,
}) {
  const clientReference =
    bundleTarget === 'react-server' && filePath && hasModuleDirective(source, 'use client')
      ? clientModuleId(path.relative(referenceBase ?? root, filePath))
      : null
  if (clientReference) {
    clientReferences.set(clientReference, {
      id: clientReference,
      file: filePath,
      // Reported from the same base the id was computed from, so a diagnostic
      // that prints one and a lookup that uses the other cannot disagree.
      relativePath: path.relative(referenceBase ?? root, filePath).replaceAll('\\', '/'),
    })
  }
  // Both graphs record a server module, because either may be the only one that
  // reaches it: an actions file imported solely by a `'use client'` component
  // never appears in the server graph at all.
  const serverReference =
    !clientReference && filePath && hasModuleDirective(source, 'use server')
      ? serverReferenceFor({
          filePath,
          root,
          referenceBase,
          bundleTarget,
          serverReferenceClient,
          serverReferences,
        })
      : null
  return {
    clientReference,
    serverReference,
    moduleSource: serverModuleSource(
      clientReference ? clientProxyModuleSource(clientReference) : source,
      serverReference,
    ),
  }
}

/**
 * Record one `'use server'` module and say what to do with its source.
 *
 * Returns `null` for a graph that has no server-function machinery — an
 * ordinary SSR bundle, an ordinary client bundle — which leaves the module
 * exactly as it was and lets the existing lane rules judge it. That is what
 * keeps `RUV1007` firing for a server action imported into a browser bundle on
 * a route that never opted into server components: nothing there could call it.
 */
function serverReferenceFor({
  filePath,
  root,
  referenceBase,
  bundleTarget,
  serverReferenceClient,
  serverReferences,
}) {
  const owned = bundleTarget === 'react-server'
  if (!owned && !serverReferenceClient) return null
  const relativePath = path.relative(referenceBase ?? root, filePath)
  const id = serverModuleId(relativePath)
  serverReferences.set(id, {
    id,
    file: filePath,
    // From the same base the id was computed from, so a diagnostic that prints
    // one and a lookup that uses the other cannot disagree.
    relativePath: relativePath.replaceAll('\\', '/'),
  })
  return owned ? { id, own: true } : { id, own: false, client: serverReferenceClient }
}

/**
 * Find the functions in a module that declare `'use server'` in their body.
 *
 * React's other way of writing a server function: instead of a whole module
 * behind the directive, one function inside a server component declares it and
 * becomes callable from the browser.
 *
 * Only functions at **module scope** are supported. A function nested inside
 * another one closes over that one's variables, and making it callable later —
 * from a different request, in a different process — means hoisting it and
 * binding what it captured, which needs a scope-resolving parser this graph does
 * not have. Rather than compile such a function into one that reads stale or
 * missing values at call time, it is refused: `unsupported` carries the lines,
 * and the caller turns them into `RUV1867`.
 *
 * The scan runs on the *transformed* source, after TypeScript is stripped, so a
 * return-type annotation between the parameter list and the body cannot be
 * mistaken for part of either.
 *
 * @param {string} source Transformed module source.
 * @returns {{ names: string[], unsupported: number[] }} Exported-in-place
 *   function names, and the 1-based lines of directives that cannot be honoured.
 */
export function inlineServerFunctions(source) {
  // Everything below reads the masked copy: comments and string bodies become
  // spaces, so a doc comment between two statements cannot hide the boundary a
  // header match anchors on, and a brace inside a string cannot move the depth.
  const masked = maskToCode(source)
  const names = []
  const unsupported = []
  let depth = 0
  for (let at = 0; at < masked.length; at += 1) {
    const char = masked[at]
    if (char === '}') {
      depth -= 1
      continue
    }
    if (char !== '{') continue
    const bodyDepth = depth
    depth += 1
    // The cheap test first, against the original: `hasModuleDirective` slices,
    // and calling it for every brace in a large module would be quadratic.
    if (!opensWithQuote(source, at + 1)) continue
    if (!hasModuleDirective(source.slice(at + 1), 'use server')) continue
    const name = bodyDepth === 0 ? functionNameBefore(masked, at) : null
    if (name) names.push(name)
    else unsupported.push(lineNumberAt(source, at))
  }
  return { names: [...new Set(names)], unsupported }
}

/**
 * `source` with everything that is not code replaced by spaces.
 *
 * Offsets are preserved, so a position found here names the same character in
 * the original — which is what lets the directive test run against the real
 * text while the structure is read from this one.
 */
function maskToCode(source) {
  const index = createCodeIndex(source)
  let masked = ''
  for (let at = 0; at < source.length; at += 1) {
    const char = source[at]
    masked += index.isCode(at) || char === '\n' ? char : ' '
  }
  return masked
}

/** Whether the first non-whitespace character at `start` could open a directive. */
function opensWithQuote(source, start) {
  let at = start
  while (at < source.length && /\s/.test(source[at])) at += 1
  return source[at] === "'" || source[at] === '"'
}

/**
 * The name of the function whose body opens at `braceOffset`, or `null`.
 *
 * `null` for every shape this cannot name with certainty — an anonymous default
 * export, a method in an object literal, a function passed as an argument.
 * Refusing is the point: a name that is wrong, or a binding that is not in
 * scope where the registration is emitted, would fail at call time instead of
 * at build time.
 */
function functionNameBefore(source, braceOffset) {
  let at = skipBackWhitespace(source, braceOffset - 1)
  if (source[at] === '>' && source[at - 1] === '=') {
    at = skipBackWhitespace(source, at - 2)
  }
  if (source[at] === ')') {
    const open = matchingOpenParen(source, at)
    if (open < 0) return null
    at = skipBackWhitespace(source, open - 1)
  } else if (/[\w$]/.test(source[at] ?? '')) {
    // A single unparenthesised arrow parameter: `async x => { … }`.
    while (at >= 0 && /[\w$]/.test(source[at])) at -= 1
    at = skipBackWhitespace(source, at)
  } else {
    return null
  }
  const header = source.slice(statementStart(source, at), at + 1).trim()
  for (const pattern of HEADER_PATTERNS) {
    const match = pattern.exec(header)
    if (match) return match[1]
  }
  return null
}

/**
 * Header shapes a server function may be written in, and only these.
 *
 * Each ends at the token before the parameter list, so the name is the last
 * thing they capture. `export default function name` is here because the local
 * binding is what the registration reads; whether it is also the default export
 * makes no difference to that.
 */
const HEADER_PATTERNS = [
  /(?:^|[;{}])\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)$/,
  /(?:^|[;{}])\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?$/,
  /(?:^|[;{}])\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?function\s*\*?\s*[A-Za-z_$]*$/,
]

function skipBackWhitespace(source, from) {
  let at = from
  while (at >= 0 && /\s/.test(source[at])) at -= 1
  return at
}

/** Offset of the `(` matching the `)` at `closeOffset`, or `-1`. */
function matchingOpenParen(source, closeOffset) {
  let depth = 0
  for (let at = closeOffset; at >= 0; at -= 1) {
    if (source[at] === ')') depth += 1
    else if (source[at] === '(') {
      depth -= 1
      if (depth === 0) return at
    }
  }
  return -1
}

/** Offset just past the previous statement boundary before `from`. */
function statementStart(source, from) {
  for (let at = from; at >= 0; at -= 1) {
    if (source[at] === ';' || source[at] === '{' || source[at] === '}') return at
  }
  return 0
}

function lineNumberAt(source, offset) {
  let line = 1
  for (let at = 0; at < offset; at += 1) {
    if (source[at] === '\n') line += 1
  }
  return line
}

/**
 * Register the module-scope functions that declare `'use server'` themselves.
 *
 * Only the `react-server` graph registers them, because that is the graph their
 * code belongs to. A client graph runs the same scan for one reason: to refuse.
 * A `'use client'` module that declares a server function inside itself gets a
 * plain function that runs in the browser — no error, no reference, and a
 * mutation that never reaches a server — so it is rejected rather than compiled.
 *
 * `alreadyRegistered` covers a module the graph has already decided about: one
 * behind a module-level directive has every export registered already, and a
 * client reference has been replaced by a proxy whose body is not the user's.
 */
function withInlineServerFunctions(
  source,
  {
    filePath,
    root,
    referenceBase,
    bundleTarget,
    serverReferenceClient,
    serverReferences,
    alreadyRegistered,
  },
) {
  const owned = bundleTarget === 'react-server'
  if (!filePath || alreadyRegistered || (!owned && !serverReferenceClient)) return source
  const found = inlineServerFunctions(source)
  if (found.unsupported.length > 0) {
    throw new Error(
      `RUV1867 ${filePath}:${found.unsupported[0]} declares 'use server' inside a function this graph cannot make callable. ` +
        'A server function must be declared at the top level of its module, or moved into a module that opens with the directive.',
    )
  }
  if (found.names.length === 0) return source
  if (!owned) {
    throw new Error(
      `RUV1867 ${filePath} declares 'use server' inside a client module. ` +
        'Move the function into a module that opens with the directive and import it.',
    )
  }
  const relativePath = path.relative(referenceBase ?? root, filePath)
  const id = serverModuleId(relativePath)
  serverReferences.set(id, {
    id,
    file: filePath,
    relativePath: relativePath.replaceAll('\\', '/'),
  })
  return source + serverRegistrationSource(id, found.names)
}

/** Apply whichever half of the server-function transform this graph needs. */
function serverModuleSource(source, serverReference) {
  if (!serverReference) return source
  return serverReference.own
    ? source + serverRegistrationSource(serverReference.id)
    : serverProxyModuleSource(serverReference.id, serverReference.client)
}

/**
 * Decide what language a module is before anything reads it as JavaScript.
 *
 * Resolution answers "which file", not "which language". Without this split a
 * JSON file reached through `require('./package.json')` was handed straight to
 * the JavaScript transform. Markdown and CSS modules skip glob expansion for
 * the same reason: `import.meta.glob` is a JavaScript construct, and scanning a
 * stylesheet for it would only find false positives.
 */
async function classifyModuleSource(
  { source, filePath, root, baseDir, markdownConfig, tsconfigPaths },
  expandImportMetaGlob,
) {
  const jsonModule = isJsonModuleFile(filePath) ? compileJsonModuleSource(source, filePath) : null
  const styleModule =
    !jsonModule && isCssModuleFile(filePath)
      ? await compileStyleModuleSource(source, filePath, root)
      : null
  const contentModule =
    jsonModule || styleModule
      ? null
      : await compileContentSource(source, filePath, root, markdownConfig)
  const compiledSource = jsonModule?.source ?? styleModule?.source ?? contentModule.source
  const contentFile = ['.md', '.mdx'].includes(path.extname(filePath ?? '').toLowerCase())
  const globExpansion =
    jsonModule || styleModule || contentFile
      ? { source: compiledSource, inputs: [] }
      : await expandImportMetaGlob(compiledSource, baseDir, root, tsconfigPaths)
  return { jsonModule, styleModule, contentModule, compiledSource, globExpansion }
}

/**
 * Turn one import specifier into a file path, or `null` when nothing local
 * answers it.
 *
 * Ordered most specific first: a configured alias wins outright, then a
 * `tsconfig` path mapping, then a relative file, then `node_modules` — and that
 * last step only when the output is actually meant to carry its dependencies.
 */
function resolveSpecifierPath(
  specifier,
  resolvedAlias,
  {
    baseDir,
    root,
    platform,
    bundleTarget,
    bundlePackages,
    bundleDependencies,
    tsconfigPaths,
    resolveTsconfigPath,
  },
) {
  if (resolvedAlias) return resolveFile(path.resolve(resolvedAlias))
  const resolvedTsconfig = resolveTsconfigPath(tsconfigPaths, specifier, resolveFile)
  if (resolvedTsconfig) return resolvedTsconfig
  const local = resolveLocalSpecifier(baseDir, specifier)
  if (local) return local
  if (platform === 'browser' || bundlePackages || bundleDependencies) {
    return resolvePackage(baseDir, specifier, bundleTarget, root)
  }
  return null
}

/**
 * Whether a resolved file is walked into the bundle or left as an external.
 *
 * A server build leaves `node_modules` alone by default: Node resolves them at
 * runtime, and inlining them would defeat the package manager's deduplication.
 * A browser build has no such resolver, so everything it reaches comes along.
 */
function shouldBundleResolved(
  resolved,
  resolvedAlias,
  { root, platform, bundlePackages, bundleDependencies },
) {
  if (!resolved) return false
  return Boolean(
    resolvedAlias ||
    isProjectLocal(root, resolved) ||
    platform === 'browser' ||
    bundlePackages ||
    bundleDependencies,
  )
}

/**
 * Record a specifier the bundle will import rather than inline.
 *
 * `externalUrls` rewrites the specifier the emitted import names. A browser
 * cannot resolve a bare `react`, so leaving one in a browser bundle produces a
 * module that never loads — but inlining React into every bundle instead gives
 * each of them its own copy, and rendering a component from one into a root
 * owned by another makes every hook in it throw. Pointing them all at one URL
 * is what keeps a page on a single React while still emitting one bundle per
 * route.
 */
function registerExternalDependency(module, specifier, resolvedAlias, externals, externalUrls) {
  // A shared module keeps its own name as the key. The URL is where the browser
  // loads it from; the key is what the registry stores it under, and both ends
  // of that lookup are generated from this one string.
  const shared = Boolean(externalUrls?.[specifier])
  let externalSpecifier = specifier
  if (!shared && resolvedAlias) externalSpecifier = toImportPath(resolvedAlias)
  if (!externals.has(externalSpecifier)) {
    externals.set(externalSpecifier, `__ext${externals.size}`)
  }
  module.deps.set(specifier, {
    external: true,
    specifier: externalSpecifier,
    alias: externals.get(externalSpecifier),
  })
}

/**
 * The `NODE_ENV` a browser bundle from this compiler sees.
 *
 * Browsers have no `process`, so every wrapped module gets a stand-in — and it
 * used to be hardcoded to `production`. React reads it to choose between the
 * two builds it ships, so a page rendered by this worker (development React,
 * because `ruvyxa dev` deliberately does not set `NODE_ENV`) hydrated against
 * production React. Ordinary hydration tolerated it; a Flight payload does not,
 * and `/server-components` failed with React refusing to read a development
 * payload with a production client.
 *
 * Mirroring the worker's own value is what keeps the two halves of one render
 * on the same build: `ruvyxa dev` leaves it unset and both halves are
 * development, `ruvyxa build` and `ruvyxa start` set it and both are
 * production.
 */
function browserNodeEnv() {
  return process.env.NODE_ENV || 'development'
}

function linkModules(modules, externals, { minify, outfile, sourceMap, externalUrls }) {
  const out = []
  const lineMappings = []
  const mapSources = new Map()
  const push = (line, mapping = null) => {
    out.push(line)
    lineMappings.push(mapping)
  }

  for (const [specifier, alias] of externals) {
    const sharedUrl = externalUrls?.[specifier]
    if (!sharedUrl) {
      push(`import * as ${alias} from ${JSON.stringify(specifier)};`)
      continue
    }
    // Read out of a runtime registry rather than imported by name. These are
    // CommonJS packages, and a generated `export *` cannot enumerate a CommonJS
    // module's names — a re-exporting shim gave consumers a namespace with only
    // `default` on it, and every `__ext0.useState` came back undefined. The
    // registry holds the module's own exports object, so a named read and a
    // `default ?? namespace` read both find what they are looking for.
    push(`import ${JSON.stringify(sharedUrl)};`)
    push(
      `const ${alias} = (globalThis.${VENDOR_REGISTRY_GLOBAL} ?? {})[${JSON.stringify(specifier)}];`,
    )
    push(
      `if (!${alias}) throw new Error(${JSON.stringify(`RUV1306 shared browser module ${specifier} was not loaded`)});`,
    )
  }
  // The rewritten specifier when one is in play: with `externalUrls` the map is
  // keyed by the URL the import names, not by `react`.
  const reactAlias = externals.get('react') ?? externals.get(externalUrls?.react)
  if (reactAlias) push(`const React = ${reactAlias}.default ?? ${reactAlias};`)
  if (externals.size > 0) push('')

  const rewrittenModules = new Map(modules.map((module) => [module.id, rewriteModule(module)]))

  for (const module of orderModulesByDependencies(modules)) {
    const rewritten = rewrittenModules.get(module.id)
    const sourceIndex = sourceMap
      ? sourceMapIndex(mapSources, module.sourceName, module.source)
      : null

    push(`const ${module.id} = (() => {`)
    push(`  const __exports = {};`)
    push(`  const module = { exports: __exports };`)
    push(`  const exports = module.exports;`)
    push(
      `  const process = globalThis.process ?? { env: { NODE_ENV: ${JSON.stringify(browserNodeEnv())} } };`,
    )
    const codeLines = rewritten.code.split('\n')
    for (let index = 0; index < codeLines.length; index++) {
      const line = codeLines[index]
      const originalLine = rewritten.lineMap[index]
      push(
        line ? `  ${line}` : '',
        sourceIndex !== null && originalLine !== null ? { sourceIndex, originalLine } : null,
      )
    }
    push(`  return module.exports;`)
    push(`})();`)
    push('')
  }

  const entry = modules[0]
  const entryRewritten = rewrittenModules.get(entry.id)
  if (entryRewritten && entryRewritten.exportedNames.includes('default')) {
    push(`export default ${entry.id}.default;`)
  }
  push(`Object.assign(globalThis.__RUVYXA_LAST_EXPORTS__ ??= {}, ${entry.id});`)
  for (const name of collectLinkedExportNames(entry.id, rewrittenModules)) {
    if (name !== 'default') push(`export const ${name} = ${entry.id}.${name};`)
  }
  if (sourceMap && !minify) push(`//# sourceMappingURL=${path.basename(outfile)}.map`)

  const code = out.join('\n')
  return {
    // Whitespace replacement is not JavaScript minification: it corrupts strings,
    // regexes, template literals, and line comments. Native production builds use
    // the Oxc minifier; the runtime compiler keeps generated code semantically exact.
    code,
    map: sourceMap ? buildSourceMap(outfile, lineMappings, mapSources) : null,
  }
}

/**
 * Return modules in stable dependency-first order.
 *
 * Discovery order is depth-first from the synthetic entry, but reversing that
 * order is not a valid topological sort when separate branches share a module.
 * Eager IIFE wrappers must initialize each local dependency before any importer
 * reads its namespace object.
 */
function orderModulesByDependencies(modules) {
  const ordered = []
  const visiting = new Map()
  const visited = new Set()
  const stack = []

  const visit = (module) => {
    if (visited.has(module.id)) return
    if (visiting.has(module.id)) {
      const cycleStart = visiting.get(module.id)
      const cycle = [...stack.slice(cycleStart), module].map(moduleDisplayName).join(' -> ')
      throw new Error(`RUV1803 circular dependency detected: ${cycle}`)
    }

    visiting.set(module.id, stack.length)
    stack.push(module)
    for (const dependency of module.deps.values()) {
      if (!dependency.external) visit(dependency)
    }
    stack.pop()
    visiting.delete(module.id)
    visited.add(module.id)
    ordered.push(module)
  }

  for (const module of modules) visit(module)
  return ordered
}

function moduleDisplayName(module) {
  return module.filePath ? path.basename(module.filePath) : module.key
}

function collectLinkedExportNames(moduleId, rewrittenModules, seen = new Set()) {
  if (seen.has(moduleId)) return []
  seen.add(moduleId)

  const rewritten = rewrittenModules.get(moduleId)
  if (!rewritten) return []

  const names = new Set(rewritten.exportedNames)
  for (const reExportedModuleId of rewritten.reExportAll) {
    for (const name of collectLinkedExportNames(reExportedModuleId, rewrittenModules, seen)) {
      names.add(name)
    }
  }
  return [...names]
}

function sourceMapIndex(mapSources, filePath, source) {
  const normalized = String(filePath).startsWith('ruvyxa:')
    ? String(filePath)
    : toImportPath(filePath)
  if (!mapSources.has(normalized)) {
    mapSources.set(normalized, { index: mapSources.size, source })
  }
  return mapSources.get(normalized).index
}

function buildSourceMap(outfile, lineMappings, mapSources) {
  const sources = [...mapSources.keys()]
  const sourcesContent = [...mapSources.values()].map((source) => source.source)
  return {
    version: 3,
    file: path.basename(outfile),
    sources,
    sourcesContent,
    names: [],
    mappings: encodeMappings(lineMappings),
  }
}

function encodeMappings(lineMappings) {
  let previousSource = 0
  let previousOriginalLine = 0
  let previousOriginalColumn = 0

  return lineMappings
    .map((mapping) => {
      if (!mapping) return ''
      const segment = [
        0,
        mapping.sourceIndex - previousSource,
        mapping.originalLine - previousOriginalLine,
        0 - previousOriginalColumn,
      ]
      previousSource = mapping.sourceIndex
      previousOriginalLine = mapping.originalLine
      previousOriginalColumn = 0
      return segment.map(encodeVlq).join('')
    })
    .join(';')
}

function encodeVlq(value) {
  const base64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  let vlq = value < 0 ? (-value << 1) + 1 : value << 1
  let encoded = ''
  do {
    let digit = vlq & 31
    vlq >>>= 5
    if (vlq > 0) digit |= 32
    encoded += base64[digit]
  } while (vlq > 0)
  return encoded
}

function rewriteModule(module) {
  const rewriteKey = [
    module.key,
    createHash('sha256').update(module.source).digest('hex'),
    module.jsxRuntime,
    module.reactCompiler ? 'react-compiler' : 'baseline',
    [...module.deps.entries()]
      .map(([specifier, dep]) => `${specifier}:${dep.external ? dep.alias : dep.id}`)
      .join('|'),
  ].join('\0')
  const cached = compilerCache.rewrites.get(rewriteKey)
  if (cached) return cached

  const source = module.transformedSource ?? transformModuleSource(module)
  const codeOnly = maskNonCode(source)

  const lines = []
  const lineMap = []
  const exported = []
  const reExportAll = []

  const sourceLines = source.split('\n')
  const codeLines = codeOnly.split('\n')
  for (let sourceLine = 0; sourceLine < sourceLines.length; sourceLine++) {
    const rawLine = sourceLines[sourceLine]
    const line = (codeLines[sourceLine] ?? '').trim()
    if (!line) {
      lines.push(rawLine)
      lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
      continue
    }

    if (/^import\b/.test(line)) {
      const rewritten = rewriteImport(rawLine.trim(), module)
      if (rewritten) {
        lines.push(rewritten)
        lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
      }
      continue
    }

    if (/^export\s+default\b/.test(line) && !line.startsWith('export default function ')) {
      const collectedRaw = [rawLine.trim()]
      const collectedCode = [line]
      let endLine = sourceLine
      while (!isBalancedDefaultExpression(collectedCode) && endLine + 1 < sourceLines.length) {
        endLine += 1
        collectedRaw.push(sourceLines[endLine].trim())
        collectedCode.push((codeLines[endLine] ?? '').trim())
      }

      const expression = collectedRaw
        .join('\n')
        .replace(/^export\s+default\s+/, '')
        .replace(/;$/, '')
      lines.push(`__exports.default = ${rewriteDynamicImports(expression, module)};`)
      lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
      sourceLine = endLine
      continue
    }

    if (/^export\b/.test(line)) {
      const result = rewriteExport(rawLine.trim(), module, exported, reExportAll)
      if (result) {
        lines.push(result)
        lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
      }
      continue
    }

    lines.push(rewriteCommonJsRequires(rewriteDynamicImports(rawLine, module), module))
    lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
  }

  for (const item of exported) {
    lines.push(item)
    lineMap.push(null)
  }

  const result = {
    code: lines.join('\n'),
    lineMap,
    exportedNames: exported
      .map((item) => item.match(/__exports\.([A-Za-z_$][\w$]*)\s=/)?.[1])
      .filter(Boolean),
    reExportAll,
  }
  setBoundedCacheEntry(compilerCache.rewrites, rewriteKey, result)
  return result
}

function isBalancedDefaultExpression(lines) {
  const expression = lines.join('\n').replace(/^export\s+default\s+/, '')
  let depth = 0
  for (const char of expression) {
    if (char === '(' || char === '{' || char === '[') depth += 1
    else if (char === ')' || char === '}' || char === ']') depth -= 1
  }
  return depth <= 0
}

function rewriteImport(line, module) {
  if (/^import\s+type\b/.test(line)) return ''
  if (/^import\s+["']/.test(line)) return ''

  const match = line.match(/^import\s+(.+?)\s+from\s+["'](.+?)["'];?$/)
  if (!match) return line

  const [, clause, specifier] = match
  const source = module.deps.get(specifier)
  if (!source) return ''

  const sourceRef = source.external ? source.alias : source.id
  return rewriteImportClause(clause, sourceRef)
}

function rewriteExport(line, module, exported, reExportAll) {
  line = rewriteDynamicImports(line, module)

  if (line.startsWith('export default function ')) {
    const name = line.match(/^export\s+default\s+function\s+([A-Za-z_$][\w$]*)/)?.[1]
    const declaration = line.replace(/^export\s+default\s+/, '')
    if (name) exported.push(`__exports.default = ${name};`)
    return name
      ? declaration
      : `__exports.default = ${declaration.replace(/^function\s*/, 'function ')}`
  }

  if (line.startsWith('export default ')) {
    return `__exports.default = ${line.replace(/^export\s+default\s+/, '').replace(/;$/, '')};`
  }

  if (/^export\s+(const|let|var)\s+/.test(line)) {
    const name = line.match(/^export\s+(?:const|let|var)\s+([A-Za-z_$][\w$]*)/)?.[1]
    if (name) exported.push(`__exports.${name} = ${name};`)
    return line.replace(/^export\s+/, '')
  }

  // `\s*\*?\s*` rather than `\s+`: a generator's `*` binds to the keyword, so
  // `export function* stream()` and `export async function* stream()` matched
  // nothing here and fell through with their `export` intact. Node then parsed
  // the wrapped module and reported `RUV1700 Unexpected token 'export'` from
  // inside generated code. `declared_lane`'s neighbour in
  // `crates/ruvyxa_bundler/src/linker.rs` had the same blind spot, written as a
  // list of prefixes with trailing spaces — one bug, once per module graph.
  if (/^export\s+(?:async\s+)?function\s*\*?\s*[A-Za-z_$]/.test(line)) {
    const name = line.match(/^export\s+(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)/)?.[1]
    if (name) exported.push(`__exports.${name} = ${name};`)
    return line.replace(/^export\s+/, '')
  }

  if (/^export\s+class\s+/.test(line)) {
    const name = line.match(/^export\s+class\s+([A-Za-z_$][\w$]*)/)?.[1]
    if (name) exported.push(`__exports.${name} = ${name};`)
    return line.replace(/^export\s+/, '')
  }

  if (line.includes(' from ')) {
    const match = line.match(/^export\s+(.+?)\s+from\s+["'](.+?)["'];?$/)
    if (!match) return ''
    const [, clause, specifier] = match
    const source = module.deps.get(specifier)
    if (!source) return ''
    const sourceRef = source.external ? source.alias : source.id
    if (clause.trim() === '*') {
      if (!source.external) reExportAll.push(source.id)
      return `Object.assign(__exports, ${sourceRef});`
    }
    const assignments = parseNamedBindings(clause).map(([original, alias]) => {
      const assignment = `__exports.${alias} = ${sourceRef}.${original};`
      exported.push(assignment)
      return assignment
    })
    return assignments.join(' ')
  }

  if (line.startsWith('export {')) {
    const assignments = parseNamedBindings(line.replace(/^export\s+/, '').replace(/;$/, '')).map(
      ([original, alias]) => {
        const assignment = `__exports.${alias} = ${original};`
        exported.push(assignment)
        return assignment
      },
    )
    return assignments.join(' ')
  }

  return line
}

function rewriteImportClause(clause, sourceRef) {
  const cleaned = clause.trim()
  if (cleaned.startsWith('* as ')) {
    return `const ${cleaned.slice(5).trim()} = ${sourceRef};`
  }
  if (cleaned.startsWith('{')) {
    return parseNamedBindings(cleaned)
      .map(([original, alias]) => `const ${alias} = ${sourceRef}.${original};`)
      .join(' ')
  }
  if (cleaned.includes(',')) {
    const [defaultName, rest] = cleaned.split(/,(.+)/)
    return [
      `const ${defaultName.trim()} = ${sourceRef}.default ?? ${sourceRef};`,
      rewriteImportClause(rest.trim(), sourceRef),
    ].join(' ')
  }
  return `const ${cleaned} = ${sourceRef}.default ?? ${sourceRef};`
}

function rewriteDynamicImports(line, module) {
  const codeOnly = maskNonCode(line, { preserveImportCallSpecifiers: true })
  return line.replace(/\bimport\s*\(\s*["']([^"']+)["']\s*\)/g, (match, specifier, offset) => {
    if (codeOnly.slice(offset, offset + match.length).trim() !== match) return match
    const source = module.deps.get(specifier)
    if (!source || source.external) return match
    return `Promise.resolve(${source.id})`
  })
}

function rewriteCommonJsRequires(line, module) {
  const codeOnly = maskNonCode(line, { preserveRequireCallSpecifiers: true })
  return line.replace(/\brequire\s*\(\s*["']([^"']+)["']\s*\)/g, (match, specifier, offset) => {
    if (codeOnly.slice(offset, offset + match.length).trim() !== match) return match
    const source = module.deps.get(specifier)
    if (!source) return match
    return source.external ? source.alias : source.id
  })
}

function parseNamedBindings(clause) {
  return clause
    .trim()
    .replace(/^\{/, '')
    .replace(/\}$/, '')
    .split(',')
    .map((part) => part.trim())
    .filter(Boolean)
    .filter((part) => !part.startsWith('type '))
    .map((part) => {
      const cleaned = part.replace(/^type\s+/, '')
      const [original, alias] = cleaned.split(/\s+as\s+/)
      return [original.trim(), (alias || original).trim()]
    })
}

/**
 * Every specifier a module imports, in the order the source imports them.
 *
 * Order is the contract, not a detail: the linker evaluates a module's
 * dependencies in this order, and ESM evaluates them in source order. Five
 * patterns are needed because one regex cannot cover the static, re-export,
 * dynamic, `require`, and side-effect forms — but running them in sequence and
 * appending each pattern's matches sorted the *forms*, not the imports. The
 * side-effect form is the last pattern, so `import './polyfill.js'` written
 * first was evaluated last, after everything it existed to prepare.
 *
 * Sorting by match position restores source order. The `Set` then keeps the
 * first occurrence of a repeated specifier, which is the one ESM evaluates.
 */
function extractSpecifiers(source) {
  const codeOnly = maskNonCode(source, {
    preserveImportExportSpecifiers: true,
    preserveImportCallSpecifiers: true,
    preserveRequireCallSpecifiers: true,
  })
  const found = []
  // `d` so each match reports where its *specifier* is. Ordering by where the
  // match began would be wrong: the first pattern's `[\s\S]*?` can start at one
  // `import` keyword and run past a side-effect import to reach the next
  // `from`, which would give that later specifier the earlier position.
  const patterns = [
    /\bimport\s+(?:type\s+)?[\s\S]*?\s+from\s+["']([^"']+)["']/dg,
    /\bexport\s+[\s\S]*?\s+from\s+["']([^"']+)["']/dg,
    /\bimport\s*\(\s*["']([^"']+)["']\s*\)/dg,
    /\brequire\s*\(\s*["']([^"']+)["']\s*\)/dg,
    /^\s*import\s+["']([^"']+)["']/dgm,
  ]
  for (const pattern of patterns) {
    for (const match of codeOnly.matchAll(pattern)) {
      const at = match.index ?? 0
      if (isTypeOnlySpecifier(codeOnly, at)) continue
      found.push({ at: match.indices?.[1]?.[0] ?? at, specifier: match[1] })
    }
  }
  found.sort((left, right) => left.at - right.at)
  return [...new Set(found.map((entry) => entry.specifier))]
}

function resolveLocalSpecifier(baseDir, specifier) {
  if (!specifier.startsWith('.') && !path.isAbsolute(specifier)) return null
  const base = path.isAbsolute(specifier) ? specifier : path.resolve(baseDir, specifier)
  return resolveFile(base)
}

/**
 * Which bundle target a caller's `platform` means, when it did not say.
 *
 * `platform` describes the JavaScript host; the bundle target decides which
 * `exports` conditions apply, and the two are not the same question. An edge
 * artifact is compiled with `platform: 'browser'` because it has no Node
 * resolver at runtime, but it must still read `worker`/`edge-light` rather than
 * `browser` — so `adapter-runner.mjs` states its target explicitly and
 * everything else takes the default.
 */
function normalizeBundleTarget(bundleTarget, platform) {
  if (bundleTarget === undefined || bundleTarget === null) {
    return platform === 'browser' ? 'client' : 'ssr'
  }
  if (!PACKAGE_EXPORT_TARGETS.includes(bundleTarget)) {
    throw new Error(
      `RUV1810 unknown bundleTarget \`${bundleTarget}\`; expected one of ${PACKAGE_EXPORT_TARGETS.join(', ')}`,
    )
  }
  return bundleTarget
}

/**
 * File extensions probed when a package path names no file directly.
 *
 * Mirrors `resolve_file_candidate` in `crates/ruvyxa_bundler/src/resolver.rs`,
 * including its order: a `.ts` source beside a `.js` build is the one this
 * project meant to compile. `.cts`/`.cjs` are here and not in `JS_EXTENSIONS`
 * because only a published package ships them.
 */
const PACKAGE_FILE_EXTENSIONS = [
  '',
  '.ts',
  '.tsx',
  '.js',
  '.jsx',
  '.mts',
  '.cts',
  '.mjs',
  '.cjs',
  '.md',
  '.mdx',
]

/**
 * The importer directory a `node_modules` walk starts from.
 *
 * Node resolves from a module's *real* path, and under pnpm that is the whole
 * difference between finding a package and not: `node_modules/react-dom` is a
 * symlink into a store directory whose siblings are react-dom's own
 * dependencies. Walking the link path instead reaches the project's
 * `node_modules`, where a transitive dependency like `scheduler` was never
 * installed — so `react-dom/client` came out of the bundler with a bare
 * `import "scheduler"` in it, and no browser could load the result.
 *
 * The Rust resolver does not need this because every path reaching it has
 * already been through `normalized_canonical_path`. This is the same rule,
 * applied where this graph's paths arrive unresolved.
 *
 * Failure is not an error: a caller may pass a directory that does not exist —
 * unit tests do — and the literal path is the right answer for those.
 */
function realImporterDir(importerDir) {
  try {
    return realpathSync(importerDir)
  } catch {
    return importerDir
  }
}

/**
 * `node_modules` directories to probe, nearest importer first.
 *
 * Mirrors `node_modules_candidates`: every ancestor of the importer that is not
 * itself named `node_modules`, then the project root's own chain. The second
 * chain is what makes a pnpm store on another path resolve for an importer that
 * lives outside the project root.
 */
function nodeModulesCandidates(importerDir, projectRoot) {
  const candidates = []
  const seen = new Set()
  for (const start of [realImporterDir(importerDir), projectRoot]) {
    if (typeof start !== 'string' || start === '') continue
    let current = path.resolve(start)
    for (;;) {
      if (path.basename(current) !== 'node_modules') {
        const candidate = path.join(current, 'node_modules')
        if (!seen.has(candidate)) {
          seen.add(candidate)
          candidates.push(candidate)
        }
      }
      const parent = path.dirname(current)
      if (parent === current) break
      current = parent
    }
  }
  return candidates
}

/** Probe a package-relative path, refusing anything that escapes the package. */
function resolvePackageRelative(pkgDir, relative) {
  if (!isSafePackageRelativePath(relative)) return null
  const joined = path.join(pkgDir, relative)
  for (const extension of PACKAGE_FILE_EXTENSIONS) {
    const candidate = extension
      ? `${joined.slice(0, joined.length - path.extname(joined).length)}${extension}`
      : joined
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  for (const extension of PACKAGE_FILE_EXTENSIONS.slice(1)) {
    const candidate = path.join(joined, `index${extension}`)
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  return null
}

/** An `exports` target names an exact file: no extension probing applies. */
function resolveExportTarget(pkgDir, target) {
  if (!target.startsWith('./')) return null
  const relative = target.slice('./'.length)
  if (!isSafePackageRelativePath(relative)) return null
  const candidate = path.join(pkgDir, relative)
  return existsSync(candidate) && !isDirectory(candidate) ? path.resolve(candidate) : null
}

/**
 * Resolve a bare package specifier with the rule the Rust bundler uses.
 *
 * This used to be `createRequire(...).resolve(specifier)` — Node's *CommonJS*
 * resolver, which matches the conditions `["node", "require"]` and nothing
 * else. For any dual package the two module graphs then picked different files
 * for the same import: the Rust client bundler took `browser`/`import` while
 * this graph inlined the CommonJS build into the very same browser bundle, and
 * an edge function artifact never saw `worker` or `edge-light`. Neither
 * disagreement raised anything; each produced a bundle that ran different code
 * than the build reported. The decision lives in `./package-exports.mjs` now,
 * and `tests/fixtures/module-resolution-conformance.json` holds the two hosts
 * to it.
 */
function resolvePackage(baseDir, specifier, bundleTarget, projectRoot) {
  if (isBuiltin(specifier)) return null
  const split = packageNameAndExportKey(specifier)
  if (!split) return null

  for (const modulesDir of nodeModulesCandidates(baseDir, projectRoot)) {
    const pkgDir = path.join(modulesDir, split.name)
    if (existsSync(path.join(pkgDir, 'package.json'))) {
      // The nearest package with this name wins, exactly as in Node: once a
      // manifest is found the walk stops, successfully or not.
      return resolveInsidePackage(pkgDir, split.key, bundleTarget)
    }
    // No manifest here. A bare directory can still satisfy a deep subpath
    // import (`pkg/dist/thing.js`); anything else means this is not the
    // package and the walk continues.
    if (split.key === '.') continue
    const deep = resolvePackageRelative(pkgDir, split.key.slice('./'.length))
    if (deep) return deep
  }

  return null
}

/** Read a package manifest, treating unreadable JSON as no manifest at all. */
function readPackageManifest(pkgDir) {
  try {
    return JSON.parse(readFileSync(path.join(pkgDir, 'package.json'), 'utf8'))
  } catch {
    return null
  }
}

/** Apply the shared rule inside one package directory. */
function resolveInsidePackage(pkgDir, key, bundleTarget) {
  const manifest = readPackageManifest(pkgDir)
  if (manifest && Object.hasOwn(manifest, 'exports')) {
    const resolved = resolveExportsEntry(manifest.exports, key, bundleTarget)
    if (resolved.kind === 'blocked') return null
    if (resolved.kind === 'targets') {
      return firstMatch(resolved.targets, (target) => resolveExportTarget(pkgDir, target))
    }
    // `unmatched` falls through to the legacy fields rather than failing.
  }
  return firstMatch(legacyEntryCandidates(manifest ?? {}, key, bundleTarget), (candidate) =>
    resolvePackageRelative(pkgDir, candidate),
  )
}

/** First candidate that probes to a real file, or null. */
function firstMatch(candidates, probe) {
  for (const candidate of candidates) {
    const file = probe(candidate)
    if (file) return file
  }
  return null
}

function resolveFile(base) {
  const extensionFallbacks = {
    '.js': ['.ts', '.tsx', '.jsx'],
    '.mjs': ['.mts', '.ts'],
    '.cjs': ['.cts', '.ts'],
    '.jsx': ['.tsx'],
  }
  const ext = path.extname(base)
  if (extensionFallbacks[ext]) {
    const withoutExt = base.slice(0, -ext.length)
    for (const fallback of extensionFallbacks[ext]) {
      const candidate = `${withoutExt}${fallback}`
      if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
    }
  }

  for (const extension of JS_EXTENSIONS) {
    const candidate = extension ? `${base}${extension}` : base
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  for (const extension of JS_EXTENSIONS.slice(1)) {
    const candidate = path.join(base, `index${extension}`)
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  return null
}

function isTypeOnlySpecifier(source, index) {
  const lineStart = source.lastIndexOf('\n', index) + 1
  const lineEnd = source.indexOf('\n', index)
  const line = source.slice(lineStart, lineEnd === -1 ? source.length : lineEnd)
  return /^\s*(import|export)\s+type\b/.test(line)
}

function isDirectory(file) {
  try {
    return statSync(file).isDirectory()
  } catch {
    return false
  }
}

function isProjectLocal(root, file) {
  const relative = path.relative(root, file)
  return relative && !relative.startsWith('..') && !path.isAbsolute(relative)
}

function isWithinProject(root, file) {
  const relative = path.relative(root, file)
  return !relative.startsWith('..') && !path.isAbsolute(relative)
}

function isAssetSpecifier(specifier) {
  return ASSET_EXTENSIONS.has(path.extname(specifier).toLowerCase())
}

function isCssModuleSpecifier(specifier) {
  return /\.module\.(css|scss|sass)(?:[?#].*)?$/i.test(specifier)
}

function isCssModuleFile(file) {
  return typeof file === 'string' && isCssModuleSpecifier(file)
}

function isJsonModuleFile(file) {
  return typeof file === 'string' && /\.json(?:[?#].*)?$/i.test(file)
}

/// Reject a resolved file whose extension has no compilation path.
///
/// The alternative is what this replaces: the file reaches the JavaScript
/// transform and Oxc reports a syntax error inside a dependency the application
/// never wrote, with no indication of which import pulled it in.
function assertSupportedModuleKind(resolved, specifier, importer) {
  const extension = path.extname(resolved).toLowerCase()
  if (!extension || MODULE_KIND_EXTENSIONS.has(extension)) return
  throw new Error(
    `RUV1806 cannot compile '${resolved}' (${extension}) imported as '${specifier}' from ${importer}: ` +
      `Ruvyxa compiles ${[...MODULE_KIND_EXTENSIONS].join(', ')}. ` +
      `Add the package to \`build.external\` if it must load this file at runtime.`,
  )
}

/// Compile a JSON file into a module the linker's CommonJS wrapper can host.
///
/// The document is emitted as one string literal parsed at runtime rather than
/// as an inline object literal: no JSON text can then be misread as code, and a
/// large manifest parses faster than the equivalent literal.
function compileJsonModuleSource(source, file) {
  let value
  try {
    value = JSON.parse(source.charCodeAt(0) === 0xfeff ? source.slice(1) : source)
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1805 Invalid JSON module ${file}: ${detail}`)
  }

  const lines = [`module.exports = JSON.parse(${JSON.stringify(JSON.stringify(value))});`]
  // Node hands a default import the whole document. The linker reads
  // `<module>.default ?? <module>`, so attach the self-reference — but never
  // when the document has its own `default` key, because overwriting it would
  // change data the application can read through `require()`.
  if (value !== null && typeof value === 'object' && !Object.hasOwn(value, 'default')) {
    lines.push(
      `Object.defineProperty(module.exports, 'default', { value: module.exports, configurable: true });`,
    )
  }
  return { source: `${lines.join('\n')}\n` }
}

async function compileStyleModuleSource(source, file, root) {
  const extension = path.extname(file).toLowerCase()
  let css = source
  let inputs = [path.resolve(file)]

  if (extension === '.scss' || extension === '.sass') {
    try {
      const sass = await import('sass')
      const result = sass.compileString(source, {
        url: pathToFileURL(file),
        syntax: extension === '.sass' ? 'indented' : 'scss',
        loadPaths: [root, path.join(root, 'node_modules')],
        style: 'expanded',
      })
      css = result.css
      inputs = [
        ...new Set([
          path.resolve(file),
          ...result.loadedUrls
            .filter((url) => url.protocol === 'file:')
            .map((url) => fileURLToPath(url)),
        ]),
      ]
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error)
      throw new Error(`RUV1402 Sass compilation failed for ${file}: ${detail}`)
    }
  }

  const classes = scopeCssModule(css, file, root)
  return {
    source: `export default ${JSON.stringify(classes)};`,
    inputs,
  }
}

function scopeCssModule(css, file, root) {
  let output = ''
  const classes = new Map()
  const scopedNames = new Map()
  const chars = [...css]
  const blockAllowsRules = [true]
  const ruleLocalClasses = [[]]
  let prelude = ''
  let preludeLocals = []
  let index = 0

  /**
   * The scoped name for one local class, minting it on first sight.
   *
   * `scopedNames` is the identity map and `classes` is what the module exports.
   * They are written together because a class can be seen first as a selector
   * and later as a `composes` target, and the two paths must agree on the name.
   */
  function scopeLocal(local) {
    const scoped = scopedNames.get(local) ?? scopedClassName(file, root, local)
    scopedNames.set(local, scoped)
    if (!classes.has(local)) classes.set(local, scoped)
    return scoped
  }

  /**
   * Fold a `composes:` declaration into the classes that own the rule.
   *
   * The exported value gains the composed names rather than replacing them, so
   * `class={styles.button}` yields every class the rule composed, in the order
   * they were declared. Duplicates are dropped: a name appearing twice in the
   * attribute changes nothing but the size of the HTML.
   */
  function applyComposition(names, owners) {
    const composed = names.map(scopeLocal)
    for (const owner of owners) {
      const ownerScoped = scopeLocal(owner)
      const exported = (classes.get(owner) ?? ownerScoped).split(/\s+/)
      for (const scoped of composed) if (!exported.includes(scoped)) exported.push(scoped)
      classes.set(owner, exported.join(' '))
    }
  }

  while (index < chars.length) {
    const char = chars[index]
    const next = chars[index + 1]

    if (char === '/' && next === '*') {
      const end = commentEnd(chars, index)
      output += chars.slice(index, end).join('')
      index = end
      continue
    }
    if (char === '"' || char === "'") {
      const end = stringEnd(chars, index)
      const literal = chars.slice(index, end).join('')
      output += literal
      prelude += literal
      index = end
      continue
    }

    const selectorContext =
      (blockAllowsRules.at(-1) ?? true) || statementOpensNestedRule(chars, index)
    if (selectorContext && chars.slice(index, index + 8).join('') === ':global(') {
      const global = globalSelectorContents(chars, index + 8)
      if (global) {
        output += global.content
        prelude += global.content
        index = global.end
        continue
      }
    }
    if (selectorContext && char === '.' && next && /[A-Za-z_-]/.test(next)) {
      let end = index + 1
      while (end < chars.length && /[A-Za-z0-9_-]/.test(chars[end])) end += 1
      const local = chars.slice(index + 1, end).join('')
      const scoped = scopeLocal(local)
      output += `.${scoped}`
      prelude += `.${scoped}`
      if (!preludeLocals.includes(local)) preludeLocals.push(local)
      index = end
      continue
    }

    if (!selectorContext && prelude.trim() === '') {
      const composition = localComposition(chars, index)
      const owners = [...ruleLocalClasses].reverse().find((items) => items.length > 0)
      if (composition && owners) {
        applyComposition(composition.names, owners)
        index = composition.end
        prelude = ''
        continue
      }
    }

    output += char
    if (char === '{') {
      const container = isContainerAtRule(prelude)
      blockAllowsRules.push(container)
      ruleLocalClasses.push(container ? [] : preludeLocals)
      preludeLocals = []
      prelude = ''
    } else if (char === '}') {
      if (blockAllowsRules.length > 1) blockAllowsRules.pop()
      if (ruleLocalClasses.length > 1) ruleLocalClasses.pop()
      prelude = ''
      preludeLocals = []
    } else if (char === ';') {
      prelude = ''
      preludeLocals = []
    } else {
      prelude += char
    }
    index += 1
  }

  return Object.fromEntries(classes)
}

/**
 * Index one past the delimiter that closes the comment starting at `start`.
 *
 * An unterminated comment runs to the end of the file, which is what a browser
 * does with one too: the remaining bytes are not selectors, so the scanner must
 * not resume reading them as such.
 */
function commentEnd(chars, start) {
  for (let index = start + 2; index < chars.length; index += 1) {
    if (chars[index] === '*' && chars[index + 1] === '/') return index + 2
  }
  return chars.length
}

/**
 * Index one past the quote that closes the string starting at `start`.
 *
 * A backslash escapes the next character, so `content: "\""` stays one string
 * rather than ending early and leaving the rest of the rule scanned as a
 * selector.
 */
function stringEnd(chars, start) {
  const quote = chars[start]
  let escaped = false
  for (let index = start + 1; index < chars.length; index += 1) {
    const character = chars[index]
    if (escaped) escaped = false
    else if (character === '\\') escaped = true
    else if (character === quote) return index + 1
  }
  return chars.length
}

function statementOpensNestedRule(chars, start) {
  let quote = null
  let escaped = false
  for (let index = start; index < chars.length; index += 1) {
    const character = chars[index]
    if (quote) {
      if (escaped) escaped = false
      else if (character === '\\') escaped = true
      else if (character === quote) quote = null
    } else if (character === '"' || character === "'") quote = character
    else if (character === '{') return true
    else if (character === ';' || character === '}') return false
  }
  return false
}

function globalSelectorContents(chars, contentStart) {
  let depth = 1
  let content = ''
  for (let index = contentStart; index < chars.length; index += 1) {
    if (chars[index] === '(') {
      depth += 1
      content += '('
    } else if (chars[index] === ')') {
      depth -= 1
      if (depth === 0) return { content, end: index + 1 }
      content += ')'
    } else content += chars[index]
  }
  return null
}

function localComposition(chars, start) {
  const keyword = 'composes'
  if (chars.slice(start, start + keyword.length).join('') !== keyword) return null
  let index = start + keyword.length
  if (chars[index] && /[A-Za-z0-9_-]/.test(chars[index])) return null
  while (chars[index] && /\s/u.test(chars[index])) index += 1
  if (chars[index] !== ':') return null
  index += 1
  const valueStart = index
  while (index < chars.length && chars[index] !== ';') index += 1
  if (chars[index] !== ';') return null
  const names = chars.slice(valueStart, index).join('').trim().split(/\s+/)
  if (
    names.length === 0 ||
    names.includes('from') ||
    names.some((name) => !/^[A-Za-z0-9_-]+$/.test(name))
  ) {
    return null
  }
  return { end: index + 1, names }
}

function scopedClassName(file, root, local) {
  const relative = path.relative(root, file).replaceAll('\\', '/').toLowerCase()
  const stem = path
    .basename(file, path.extname(file))
    .replace(/\.module$/i, '')
    .replace(/[^A-Za-z0-9]/g, '_')
  return `${stem}_${local}__${fnv1a64(`${relative}:${local}`)}`
}

function fnv1a64(value) {
  let hash = 0xcbf29ce484222325n
  for (const byte of Buffer.from(value)) {
    hash ^= BigInt(byte)
    hash = BigInt.asUintN(64, hash * 0x100000001b3n)
  }
  return hash.toString(16).padStart(16, '0')
}

function isContainerAtRule(prelude) {
  const normalized = prelude.trimStart().toLowerCase()
  return [
    '@media',
    '@supports',
    '@layer',
    '@container',
    '@document',
    '@scope',
    '@keyframes',
    '@-webkit-keyframes',
  ].some((prefix) => normalized.startsWith(prefix))
}

async function readSourceFile(file) {
  const stats = statSync(file)
  const cacheKey = path.resolve(file)
  const cached = compilerCache.sources.get(cacheKey)
  if (cached && cached.mtimeMs === stats.mtimeMs && cached.size === stats.size) {
    return cached.source
  }
  const source = await readFile(file, 'utf8')
  setBoundedCacheEntry(compilerCache.sources, cacheKey, {
    mtimeMs: stats.mtimeMs,
    size: stats.size,
    source,
  })
  return source
}

/**
 * Compile a Markdown or MDX module through the shared `@mdx-js/mdx` pipeline.
 *
 * `markdownConfig` is normally discovered from the stable config pointer the
 * CLI writes. Passing an object keeps executable plugin functions live inside
 * the persistent plugin host; passing `false` disables project config loading
 * while the config module itself is being compiled.
 */
export async function compileContentSource(source, filePath, projectRoot, markdownConfig) {
  const extension = filePath ? path.extname(filePath).toLowerCase() : ''
  if (extension !== '.md' && extension !== '.mdx') return { source, inputs: [] }

  const configured = await resolveMarkdownConfiguration(projectRoot, markdownConfig)

  const providerFile = extension === '.mdx' ? findMdxComponentsFile(filePath, projectRoot) : null
  const providerImportSource = providerFile
    ? relativeImportSpecifier(path.dirname(filePath), providerFile)
    : undefined
  const cacheKey = createHash('sha256')
    .update(extension)
    .update('\0')
    .update(source)
    .update('\0')
    .update(providerImportSource ?? '')
    .update('\0')
    .update(configured.fingerprint)
    .digest('hex')
  const cached = compilerCache.content.get(cacheKey)
  if (cached) return cached

  const { frontmatterSource, body } = splitContentFrontmatter(source)
  const [frontmatter, { compile }, { default: remarkGfm }] = await Promise.all([
    parseContentFrontmatter(frontmatterSource, filePath),
    import('@mdx-js/mdx'),
    import('remark-gfm'),
  ])
  const headings = []
  let compiledFile
  try {
    compiledFile = await compile(
      {
        value: body,
        path: filePath,
        data: { ruvyxa: { frontmatter } },
      },
      {
        format: extension === '.md' ? 'md' : 'mdx',
        jsx: false,
        outputFormat: 'program',
        development: false,
        providerImportSource,
        remarkPlugins: [
          ...(configured.options?.gfm === false ? [] : [remarkGfm]),
          ...(configured.options?.remarkPlugins ?? []),
          ...(extension === '.md' ? [escapeRawMarkdownHtmlPlugin] : []),
        ],
        rehypePlugins: [
          ...(configured.options?.rehypePlugins ?? []),
          createContentMetadataPlugin(headings),
          [wrapContentRootPlugin, { format: extension.slice(1) }],
        ],
        recmaPlugins: configured.options?.recmaPlugins ?? [],
        remarkRehypeOptions: configured.options?.remarkRehypeOptions,
      },
    )
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1311 ${filePath}: ${detail}`)
  }

  const compiled = String(compiledFile)
  const pluginFrontmatter = compiledFile.data?.ruvyxa?.frontmatter ?? frontmatter
  let serializedFrontmatter
  try {
    assertJsonCompatibleFrontmatter(pluginFrontmatter, new WeakSet())
    serializedFrontmatter = JSON.stringify(pluginFrontmatter)
    if (serializedFrontmatter === undefined) throw new TypeError('value is not JSON-compatible')
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1312 ${filePath}: plugin frontmatter must be JSON-compatible: ${detail}`)
  }

  const prefix = [
    contentExport(compiled, 'frontmatter', serializedFrontmatter),
    contentExport(compiled, 'meta', 'frontmatter'),
    contentExport(compiled, 'headings', JSON.stringify(headings)),
    contentExport(compiled, 'contentFormat', JSON.stringify(extension.slice(1))),
  ]
    .filter(Boolean)
    .join('\n')
  const output = { source: `${compiled}\n${prefix}\n`, inputs: configured.inputs }
  setBoundedCacheEntry(compilerCache.content, cacheKey, output)
  return output
}

async function resolveMarkdownConfiguration(projectRoot, configured) {
  if (configured === false || configured === null) {
    return { options: undefined, fingerprint: 'defaults', inputs: [] }
  }
  if (configured !== undefined) {
    return {
      options: normalizeMarkdownConfiguration(configured),
      fingerprint: markdownConfigurationIdentity(configured),
      inputs: [],
    }
  }

  const root = path.resolve(projectRoot)
  const pointer = path.join(root, '.ruvyxa', 'cache', 'config', 'runtime-config.mjs')
  let pointerSource
  try {
    pointerSource = await readFile(pointer, 'utf8')
  } catch {
    return { options: undefined, fingerprint: 'defaults', inputs: [] }
  }

  const fingerprint = createHash('sha256').update(pointerSource).digest('hex')
  const cached = compilerCache.markdownConfigurations.get(root)
  if (cached?.fingerprint === fingerprint) return cached

  let projectMarkdown
  try {
    const url = `${pathToFileURL(pointer).href}?v=${fingerprint}`
    projectMarkdown = (await import(url)).default
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1313 failed to load Markdown configuration: ${detail}`)
  }

  const result = {
    options: normalizeMarkdownConfiguration(projectMarkdown),
    fingerprint,
    inputs: projectMarkdown === undefined ? [] : [pointer],
  }
  setBoundedCacheEntry(compilerCache.markdownConfigurations, root, result)
  return result
}

const markdownConfigurationIds = new WeakMap()
let nextMarkdownConfigurationId = 1

function markdownConfigurationIdentity(configured) {
  if (!configured || typeof configured !== 'object') return String(configured)
  let identity = markdownConfigurationIds.get(configured)
  if (!identity) {
    identity = `explicit-${nextMarkdownConfigurationId}`
    nextMarkdownConfigurationId += 1
    markdownConfigurationIds.set(configured, identity)
  }
  return identity
}

function normalizeMarkdownConfiguration(value) {
  if (value === undefined) return undefined
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('RUV1602 config.markdown must be an object.')
  }
  for (const field of ['remarkPlugins', 'rehypePlugins', 'recmaPlugins']) {
    if (value[field] !== undefined && !Array.isArray(value[field])) {
      throw new TypeError(`RUV1602 config.markdown.${field} must be an array.`)
    }
  }
  if (value.gfm !== undefined && typeof value.gfm !== 'boolean') {
    throw new TypeError('RUV1602 config.markdown.gfm must be boolean.')
  }
  if (
    value.remarkRehypeOptions !== undefined &&
    (!value.remarkRehypeOptions ||
      typeof value.remarkRehypeOptions !== 'object' ||
      Array.isArray(value.remarkRehypeOptions))
  ) {
    throw new TypeError('RUV1602 config.markdown.remarkRehypeOptions must be an object.')
  }
  return value
}

function findMdxComponentsFile(filePath, projectRoot) {
  const root = path.resolve(projectRoot)
  let directory = path.dirname(path.resolve(filePath))

  while (isWithinProject(root, directory)) {
    for (const extension of MDX_COMPONENT_EXTENSIONS) {
      const candidate = path.join(directory, `mdx-components${extension}`)
      if (existsSync(candidate)) return candidate
    }
    if (directory === root) break
    const parent = path.dirname(directory)
    if (parent === directory) break
    directory = parent
  }
  return null
}

function relativeImportSpecifier(fromDirectory, file) {
  const relative = path.relative(fromDirectory, file).replaceAll('\\', '/')
  return relative.startsWith('.') ? relative : `./${relative}`
}

function setBoundedCacheEntry(cache, key, value) {
  cache.delete(key)
  cache.set(key, value)
  while (cache.size > COMPILER_CACHE_MAX_ENTRIES) {
    cache.delete(cache.keys().next().value)
  }
}

function contentExport(compiled, name, value) {
  return hasNamedExport(compiled, name) ? '' : `export const ${name} = ${value};`
}

function hasNamedExport(source, name) {
  const tokens = javascriptTokens(source)
  for (let index = 0; index < tokens.length; index += 1) {
    if (tokens[index] !== 'export') continue
    let cursor = index + 1
    if (tokens[cursor] === 'async') cursor += 1
    if (['const', 'let', 'var'].includes(tokens[cursor]) && tokens[cursor + 1] === name) return true
    if (['function', 'class'].includes(tokens[cursor])) {
      cursor += 1
      if (tokens[cursor] === '*') cursor += 1
      if (tokens[cursor] === name) return true
    }
    if (tokens[cursor] === '{' && exportListBinds(tokens, cursor + 1, name)) return true
  }
  return false
}

/**
 * Whether an `export { ... }` list publishes `name`.
 *
 * `a as b` publishes `b`, so the name after `as` is the one that counts. A bare
 * `type` token is dropped rather than treated as an identifier: `export { type
 * Foo, bar }` publishes only `bar`, and counting `type` would shift every
 * specifier in the list by one.
 */
function exportListBinds(tokens, start, name) {
  let specifier = []
  for (let cursor = start; cursor < tokens.length; cursor += 1) {
    const token = tokens[cursor]
    if (token !== ',' && token !== '}') {
      if (token !== 'type') specifier.push(token)
      continue
    }
    const asIndex = specifier.indexOf('as')
    const exported = asIndex >= 0 ? specifier[asIndex + 1] : specifier[0]
    if (exported === name) return true
    specifier = []
    if (token === '}') return false
  }
  return false
}

function javascriptTokens(source) {
  const tokens = []
  let index = 0
  while (index < source.length) {
    const character = source[index]
    if (/\s/u.test(character)) {
      index += 1
      continue
    }
    if (character === '/' && source[index + 1] === '/') {
      index += 2
      while (index < source.length && source[index] !== '\n') index += 1
      continue
    }
    if (character === '/' && source[index + 1] === '*') {
      index += 2
      while (index + 1 < source.length && !(source[index] === '*' && source[index + 1] === '/'))
        index += 1
      index = Math.min(index + 2, source.length)
      continue
    }
    if (character === "'" || character === '"' || character === '`') {
      const quote = character
      index += 1
      while (index < source.length) {
        if (source[index] === '\\') index += 2
        else if (source[index] === quote) {
          index += 1
          break
        } else index += 1
      }
      continue
    }
    if (/[\p{Letter}\p{Number}_$]/u.test(character)) {
      const start = index
      index += 1
      while (index < source.length && /[\p{Letter}\p{Number}_$]/u.test(source[index])) index += 1
      tokens.push(source.slice(start, index))
      continue
    }
    tokens.push(character)
    index += 1
  }
  return tokens
}

function splitContentFrontmatter(source) {
  const normalized = source.replace(/^\uFEFF/, '')
  if (!normalized.startsWith('---\n') && !normalized.startsWith('---\r\n')) {
    return { frontmatterSource: null, body: normalized }
  }

  const lines = normalized.split(/\r?\n/)
  const end = lines.findIndex((line, index) => index > 0 && /^(---|\.\.\.)\s*$/.test(line))
  if (end === -1) {
    throw new Error("RUV1312 frontmatter starts with '---' but has no closing delimiter")
  }
  return {
    // YAML block-scalar chomping depends on the final line ending before the delimiter.
    frontmatterSource: `${lines.slice(1, end).join('\n')}\n`,
    body: lines.slice(end + 1).join('\n'),
  }
}

async function parseContentFrontmatter(source, filePath) {
  if (source === null || source.trim() === '') return {}

  const { isMap, isScalar, isSeq, parseDocument } = await import('yaml')
  let document
  try {
    document = parseDocument(source, { schema: 'core' })
    if (document.errors.length > 0) throw document.errors[0]
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1312 ${filePath}: invalid YAML frontmatter: ${detail}`)
  }

  let value
  try {
    assertJsonCompatibleYamlKeys(document.contents, { isMap, isScalar, isSeq })
    value = document.toJS({ maxAliasCount: 100 })
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(
      `RUV1312 ${filePath}: frontmatter must contain JSON-compatible values: ${detail}`,
    )
  }

  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`RUV1312 ${filePath}: frontmatter must be a YAML mapping`)
  }

  try {
    assertJsonCompatibleFrontmatter(value, new WeakSet())
    return JSON.parse(JSON.stringify(value))
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(
      `RUV1312 ${filePath}: frontmatter must contain JSON-compatible values: ${detail}`,
    )
  }
}

function assertJsonCompatibleYamlKeys(node, yaml) {
  if (yaml.isMap(node)) {
    for (const pair of node.items) {
      if (!yaml.isScalar(pair.key) || typeof pair.key.value !== 'string') {
        throw new TypeError('YAML mapping keys must be strings')
      }
      assertJsonCompatibleYamlKeys(pair.value, yaml)
    }
    return
  }
  if (yaml.isSeq(node)) {
    for (const child of node.items) assertJsonCompatibleYamlKeys(child, yaml)
  }
}

function assertJsonCompatibleFrontmatter(value, ancestors) {
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw new TypeError('non-finite numbers are not supported')
  }
  if (value === null || typeof value !== 'object') return
  if (ancestors.has(value)) throw new TypeError('cyclic YAML aliases are not supported')

  ancestors.add(value)
  for (const child of Array.isArray(value) ? value : Object.values(value)) {
    assertJsonCompatibleFrontmatter(child, ancestors)
  }
  ancestors.delete(value)
}

function escapeRawMarkdownHtmlPlugin() {
  return (tree) => replaceRawMarkdownHtml(tree)
}

function replaceRawMarkdownHtml(node) {
  if (!Array.isArray(node?.children)) return
  node.children = node.children.map((child) => {
    if (child?.type === 'html') return { type: 'text', value: String(child.value ?? '') }
    replaceRawMarkdownHtml(child)
    return child
  })
}

function createContentMetadataPlugin(headings) {
  return function contentMetadataPlugin() {
    return (tree) => {
      const slugCounts = new Map()
      collectContentHeadingElements(tree, headings, slugCounts)
    }
  }
}

function collectContentHeadingElements(node, headings, slugCounts) {
  const heading = node?.type === 'element' && /^h[1-6]$/.test(node.tagName ?? '')
  if (heading) {
    const text = contentPlainText(node.children ?? [])
    const properties = (node.properties ??= {})
    // `remark-gfm` synthesizes this accessibility label for footnotes. It is
    // document chrome rather than an authored content heading and the native
    // compiler has never exposed it through `headings`.
    if (properties.id === 'footnote-label') return
    let slug = typeof properties.id === 'string' && properties.id ? properties.id : undefined
    if (!slug) {
      const baseSlug =
        text
          // Locale-independent on purpose: this slug becomes a heading `id` in
          // build output, and the native compiler's `slugify` lowercases with
          // Rust's `char::to_lowercase`. `toLocaleLowerCase()` would answer by
          // the host's ICU locale instead — on a Turkish host `I` becomes `ı`
          // here and `i` there, so the same source would build to different
          // bytes on different machines. Same reason `localeCompare` is banned.
          .toLowerCase()
          .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
          .replace(/(?:^-)|(?:-$)/g, '') || 'section'
      const occurrence = slugCounts.get(baseSlug) ?? 0
      slug = occurrence === 0 ? baseSlug : `${baseSlug}-${occurrence}`
      slugCounts.set(baseSlug, occurrence + 1)
      properties.id = slug
    }
    headings.push({ depth: Number(node.tagName.slice(1)), slug, text })
  }

  for (const child of node?.children ?? []) {
    collectContentHeadingElements(child, headings, slugCounts)
  }
}

function contentPlainText(nodes) {
  return nodes
    .map((node) => (node.type === 'text' ? node.value : contentPlainText(node.children ?? [])))
    .join('')
}

function wrapContentRootPlugin(options = {}) {
  return (tree) => {
    const children = tree.children ?? []
    tree.children = [
      {
        type: 'element',
        tagName: 'article',
        properties: {
          className: ['ruvyxa-content'],
          dataContentFormat: String(options.format ?? 'mdx'),
        },
        children,
      },
    ]
  }
}

async function writeIfChanged(file, contents) {
  try {
    if ((await readFile(file, 'utf8')) === contents) return
  } catch {
    // File does not exist yet.
  }
  await writeFile(file, contents)
}

/**
 * JavaScript language levels `build.target` accepts.
 *
 * Mirrors `EsTarget::ALL` in `crates/ruvyxa_bundler/src/types.rs`; the two are
 * held to `tests/fixtures/es-target-conformance.json`, because a project
 * renders through whichever graph built it and the two must not disagree about
 * what is configurable.
 */
const ES_TARGETS = [
  'es2015',
  'es2016',
  'es2017',
  'es2018',
  'es2019',
  'es2020',
  'es2021',
  'es2022',
  'es2023',
  'es2024',
  'es2025',
  'es2026',
  'esnext',
]

/**
 * The language level this process compiles to.
 *
 * Read from the environment for the same reason `RUVYXA_JSX_RUNTIME` is: a
 * prerender worker and the dev server's render process are separate processes
 * with no view of `ruvyxa.config.ts`. The Rust side validates the configured
 * value before setting it, so an unusable value arriving here means this module
 * was driven directly — worth failing on rather than quietly compiling to a
 * different level than the client bundle used. An *unset* variable is the
 * ordinary case and means the default; an empty one is a host that set it
 * wrong, and is refused for the same reason `EsTarget::parse` refuses it.
 */
export function resolveEsTarget(value = process.env.RUVYXA_ES_TARGET) {
  const raw = String(value ?? 'esnext')
    .trim()
    .toLowerCase()
  if (raw === 'es6') return 'es2015'
  if (!ES_TARGETS.includes(raw)) {
    throw new Error(`RUV1601 build.target must be one of: ${ES_TARGETS.join(', ')}, got \`${raw}\``)
  }
  return raw
}

/** Specifier prefix oxc emits for every helper import it adds. */
const HELPER_RUNTIME_PREFIX = '@oxc-project/runtime/helpers/'

/**
 * Helper-runtime imports in transformed output, or an empty list.
 *
 * oxc places these immediately after the directive prologue and before
 * everything else, one `import <ident> from "<specifier>";` per line, so this
 * walks that run and stops at the first line that is not such an import. A
 * string literal cannot reach the run: for it to be there, every line above it
 * would have to be an import statement and the string would have to sit inside
 * one.
 */
export function runtimeHelperImports(code) {
  const found = []
  let cursor = directivePrologueEnd(code)
  const importStatement = /^\s*import\s+[A-Za-z_$][\w$]*\s+from\s*(["'])([^"']+)\1;?/
  for (;;) {
    const match = importStatement.exec(code.slice(cursor))
    if (!match) break
    if (match[2].startsWith(HELPER_RUNTIME_PREFIX)) {
      found.push(match[2].slice(HELPER_RUNTIME_PREFIX.length))
    }
    cursor += match.index + match[0].length
  }
  return found
}

/** Which oxc parser dialect an extension asks for. Anything unlisted is plain JS. */
const TRANSFORM_LANG_BY_EXTENSION = {
  '.tsx': 'tsx',
  '.jsx': 'jsx',
  '.ts': 'ts',
  '.mts': 'ts',
  '.cts': 'ts',
}

function transformModuleSource(module) {
  // Resolve lazily so tools that copy compiler.mjs for path-isolation checks do
  // not need the package dependency beside the copied file until compilation.
  const filename = String(module.filePath || module.key || 'ruvyxa:module.ts')
  const extension = path.extname(filename).toLowerCase()
  const lang = TRANSFORM_LANG_BY_EXTENSION[extension] ?? 'js'
  const esTarget = resolveEsTarget()
  const transformKey = createHash('sha256')
    .update(lang)
    .update(esTarget)
    .update('\0')
    .update(module.jsxRuntime)
    .update('\0')
    .update(module.reactCompiler ? 'react-compiler' : 'baseline')
    .update('\0')
    .update(module.source)
    .digest('hex')
  const cached = compilerCache.transforms.get(transformKey)
  if (cached) {
    if (typeof cached === 'string') return cached
    module.transformLineMap = cached.lineMap
    return cached.code
  }
  const { transformSync } = createRequire(
    path.join(path.dirname(fileURLToPath(import.meta.url)), '__ruvyxa-transform.cjs'),
  )('oxc-transform')
  const reactCompiled = module.reactCompiler
    ? compilerSupport.transformWithReactCompiler(module.source, filename)
    : null
  const result = transformSync(filename, reactCompiled?.code ?? module.source, {
    lang,
    sourceType: 'module',
    sourcemap: true,
    target: esTarget,
    typescript: {
      onlyRemoveTypeImports: false,
      allowNamespaces: true,
      optimizeConstEnums: false,
      optimizeEnums: false,
    },
    jsx: {
      runtime: module.jsxRuntime,
      development: false,
      throwIfNamespace: false,
      pure: false,
      pragma: 'React.createElement',
      pragmaFrag: 'React.Fragment',
    },
  })

  if (result.errors.length > 0) {
    const detail = result.errors.map((error) => error.message).join('; ')
    throw new Error(`RUV1802 Oxc transform failed for ${filename}: ${detail}`)
  }
  // Downlevelling is not free of runtime support: oxc's helper loader defaults
  // to emitting `@oxc-project/runtime/helpers/*` imports, Ruvyxa ships no helper
  // runtime, and this graph leaves bare specifiers external — so the import
  // would reach production as a module nothing can resolve. The Rust bundler
  // refuses the same output in `compiler::reject_runtime_helpers`.
  const helpers = runtimeHelperImports(result.code)
  if (helpers.length > 0) {
    const named = [...new Set(helpers)].sort(compareCodeUnits).join(', ')
    throw new Error(
      `RUV1802 build.target \`${esTarget}\` needs the runtime helpers ${named} for ${filename}, and Ruvyxa ships no helper runtime — raise build.target (ordinary application code compiles helper-free at es2022 and above) or remove the syntax that needs downlevelling`,
    )
  }
  module.transformLineMap = composeLineMaps(result.map, reactCompiled?.rawMap)
  setBoundedCacheEntry(compilerCache.transforms, transformKey, {
    code: result.code,
    lineMap: module.transformLineMap,
  })
  return result.code
}

function composeLineMaps(outerMap, innerMap) {
  if (!outerMap) return undefined
  const outerLines = firstOriginalLineByGeneratedLine(outerMap)
  if (!innerMap) return outerLines
  const innerLines = firstOriginalLineByGeneratedLine(innerMap)
  return outerLines.map((line) => (line === null ? null : (innerLines[line] ?? null)))
}

function firstOriginalLineByGeneratedLine(map) {
  const { eachMapping, TraceMap } = compilerSupport
  const lines = []
  eachMapping(new TraceMap(map), (mapping) => {
    const generated = mapping.generatedLine - 1
    if (lines[generated] === undefined && mapping.originalLine !== null) {
      lines[generated] = mapping.originalLine - 1
    }
  })
  return lines.map((line) => line ?? null)
}

let compilerSupport
let compilerSupportPromise

/**
 * Load compile-only helpers on demand. Utilities such as `runtimeAliases()` are
 * intentionally usable from an isolated runtime directory without resolving
 * the compiler's package dependencies or sibling implementation modules.
 */
async function loadCompilerSupport() {
  compilerSupportPromise ??= Promise.all([
    import('@jridgewell/trace-mapping'),
    import('./paths.mjs'),
    import('./glob.mjs'),
    import('./react-compiler.mjs'),
  ]).then(([traceMapping, paths, glob, reactCompiler]) => {
    compilerSupport = {
      eachMapping: traceMapping.eachMapping,
      TraceMap: traceMapping.TraceMap,
      loadTsconfigPaths: paths.loadTsconfigPaths,
      resolveTsconfigPath: paths.resolveTsconfigPath,
      expandImportMetaGlob: glob.expandImportMetaGlob,
      transformWithReactCompiler: reactCompiler.transformWithReactCompiler,
    }
    return compilerSupport
  })
  return compilerSupportPromise
}

function normalizeJsxRuntime(value) {
  const runtime = String(value).toLowerCase()
  if (runtime === 'classic' || runtime === 'automatic') return runtime
  throw new Error(`RUV1804 JSX runtime must be \`classic\` or \`automatic\`, got \`${value}\``)
}

/**
 * Reject a module that must not be in a browser bundle.
 *
 * Every module this graph compiles on the browser platform is already inside a
 * client boundary's dependency closure, so the module's own lane is the whole
 * question — the Rust bundler reaches the same answer by walking that closure
 * from the other end and rejecting the crossing into it.
 */
function checkClientBoundary(root, filePath, source) {
  if (!filePath) return
  const normalized = path.relative(root, filePath).replaceAll('\\', '/')
  if (normalized.startsWith('server/')) {
    throw new Error(`RUV1007: Server-only file imported into client bundle: ${filePath}`)
  }
  const lane = moduleLane(filePath, source)
  if (lane === 'server' || lane === 'action') {
    throw new Error(
      `RUV1007: ${lane === 'server' ? 'Server-only' : 'Server action'} module imported into client bundle: ${filePath}`,
    )
  }
  if (extractSpecifiers(source).some(isServerOnlySpecifier)) {
    throw new Error(`RUV1007: Server-only module imported into client bundle: ${filePath}`)
  }
  const envNames = privateEnvReads(source)
  if (envNames.length > 0) {
    throw new Error(
      `RUV1008: Private environment variable ${envNames[0]} used in client bundle: ${filePath}`,
    )
  }
}

function isServerOnlySpecifier(specifier) {
  return ['server-only', '@ruvyxa/auth', '@ruvyxa/database'].includes(specifier)
}

/**
 * Every private `process.env` read that is really code.
 *
 * Found in the shared scanner's mask and parsed out of the raw source, the
 * find-in-masked/slice-from-raw shape `ruvyxa_graph` uses for the same reason:
 * a name is a value, and masking blanks values. A read inside a template
 * interpolation still counts — the mask treats interpolations as code — while
 * one inside a string, comment, or regex literal does not.
 */
function privateEnvReads(source) {
  const names = []
  for (const offset of findInCode(source, 'process.env')) {
    if (!isEnvReadBoundary(source, offset)) continue
    const parsed = parsePrivateEnvName(source, offset + 'process.env'.length)
    if (parsed && envReadIsPrivate(parsed.name)) names.push(parsed.name)
  }
  return names
}

function isEnvReadBoundary(source, index) {
  const previous = source[index - 1]
  return !previous || (!/[A-Za-z0-9_$]/.test(previous) && previous !== '.')
}

function parsePrivateEnvName(source, start) {
  let index = start
  while (/\s/.test(source[index] ?? '')) index += 1
  if (source[index] === '.') {
    index += 1
    const match = /^[A-Z_][A-Z0-9_]*/.exec(source.slice(index))
    if (!match) return null
    return { name: match[0], end: index + match[0].length }
  }
  if (source[index] !== '[') return null
  index += 1
  while (/\s/.test(source[index] ?? '')) index += 1
  const quote = source[index]
  if (quote !== '"' && quote !== "'") return null
  index += 1
  const match = /^[A-Z_][A-Z0-9_]*/.exec(source.slice(index))
  if (!match) return null
  index += match[0].length
  if (source[index] !== quote) return null
  index += 1
  while (/\s/.test(source[index] ?? '')) index += 1
  return source[index] === ']' ? { name: match[0], end: index + 1 } : null
}

function pushIfExists(collection, file) {
  if (existsSync(file)) collection.push(file)
}

function preferExisting(...files) {
  return files.find((file) => existsSync(file)) ?? files[0]
}
