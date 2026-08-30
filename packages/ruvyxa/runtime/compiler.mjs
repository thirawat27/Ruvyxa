import { createHash } from 'node:crypto'
import { existsSync, readdirSync, readFileSync, realpathSync, statSync } from 'node:fs'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createRequire, isBuiltin } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import {
  RSC_CLIENT_RUNTIME_SPECIFIER,
  RSC_RENDERER_SPECIFIER,
  clientModuleId,
  clientProxyModuleSource,
  serverModuleId,
  serverProxyModuleSource,
  serverRegistrationSource,
} from './client-references.mjs'
import { compareCodePoints } from './order.mjs'
import { toImportPath } from './paths.mjs'
import { createPluginRegistry, dispatchBuildTransform } from './plugin-http.mjs'
import {
  isSafePackageRelativePath,
  legacyEntryCandidates,
  packageNameAndExportKey,
  PACKAGE_EXPORT_TARGETS,
  resolveExportsEntry,
} from './package-exports.mjs'
import {
  containsJsx,
  createCodeIndex,
  directivePrologueEnd,
  findInCode,
  maskNonCode,
} from './scanner.mjs'
/**
 * File extensions probed when a specifier names no file directly, in priority
 * order — one list for project files and package files alike, mirroring
 * `PROBE_EXTENSIONS` in `crates/ruvyxa_bundler/src/resolver.rs`. A `.ts` source
 * beside a `.js` build is the one this project meant to compile.
 */
const FILE_PROBE_EXTENSIONS = ['ts', 'tsx', 'js', 'jsx', 'mts', 'cts', 'mjs', 'cjs', 'md', 'mdx']

/**
 * TypeScript sources a written extension may stand for: `./x.js` names `x.ts`
 * in a project whose TypeScript has not been emitted. Mirrors
 * `typescript_source_extensions` in the Rust resolver.
 *
 * Only these four are rewritten. Replacing the last dotted segment of anything
 * else asks for the wrong file and never asks for the right one:
 * `./util.inspect` becomes `util.js`, which does not exist, while
 * `util.inspect.js` — the file `object-inspect` ships, and the one Node finds
 * by appending — is never probed. Node appends; a dot inside a basename is
 * ordinary.
 */
const TYPESCRIPT_SOURCE_EXTENSIONS = {
  '.js': ['ts', 'tsx', 'jsx'],
  '.mjs': ['mts', 'ts'],
  '.cjs': ['cts', 'ts'],
  '.jsx': ['tsx'],
}
/**
 * A JavaScript identifier, as source text for a `u`-flagged regular expression.
 *
 * `[A-Za-z_$][\w$]*` — what every rewrite here used to match — is ASCII, and
 * JavaScript identifiers are not: `café` matched only `caf`, so the linker
 * emitted `__exports.caf = caf` and the module threw `caf is not defined` on
 * import, while `ชื่อ` and `Δelta` matched nothing at all and their exports were
 * dropped with no diagnostic. The Rust linker reads the same names with
 * `char::is_alphanumeric`, which is why only this graph was wrong.
 */
const IDENTIFIER_SOURCE = String.raw`[\p{ID_Start}$_][\p{ID_Continue}$\u200C\u200D]*`

/** `IDENTIFIER_SOURCE` spliced into `pattern`, compiled Unicode-aware. */
function identifierPattern(pattern) {
  return new RegExp(pattern.replaceAll('%IDENT%', IDENTIFIER_SOURCE), 'u')
}

/** Whether `text` is exactly one identifier. */
function isIdentifier(text) {
  return identifierPattern('^%IDENT%$').test(text)
}

/**
 * The name run in a `process.env` member access.
 *
 * Deliberately **not** `IDENTIFIER_SOURCE`. The name is read out of the same
 * bytes on both sides of the boundary check, and the Rust half reads it with
 * `skip_identifier` in `crates/ruvyxa_bundler/src/ast.rs` — a run of ASCII
 * identifier bytes with no start-character requirement. Matching a Unicode
 * identifier here would make `process.env["café"]` a private read in this graph
 * and no read at all in Rust, replacing one divergence with another; requiring
 * an identifier *start* would miss `process.env["1PASSWORD_TOKEN"]`, which is
 * legal source and which Rust does read. This used to be an upper-case-only
 * character class, which made `process.env.databaseUrl` invisible to
 * `ruvyxa dev` — the name matched nothing — and refused by
 * `ruvyxa build`, and truncated `MIXED_case` and `NODE_ENVx` — the second of
 * those into the public exemption. The `extraction` section of
 * `tests/fixtures/env-policy-conformance.json` holds both graphs to this.
 */
const ENV_NAME_PATTERN = /^[A-Za-z0-9_$]+/

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
  pluginTransforms: new Map(),
})
compilerCache.transforms ??= new Map()
compilerCache.markdownConfigurations ??= new Map()
compilerCache.pluginTransforms ??= new Map()

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
  projectRootListings.clear()
  compilerCache.sources.clear()
  compilerCache.transforms.clear()
  compilerCache.rewrites.clear()
  compilerCache.content.clear()
  compilerCache.markdownConfigurations.clear()
  compilerCache.pluginTransforms.clear()
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

/**
 * Which files compose a route, in one place.
 *
 * Layouts, templates, and parallel slots were discovered here *and* in
 * `worker-pool.mjs`, which carried its own `collectLayouts` alongside this one
 * — two implementations that happened to agree. They are one now because a
 * third caller arrived: `adapter-runner.mjs` composes a server-components route
 * for a deployed function, and a route composed one way by `ruvyxa start` and
 * another by its own build is the failure this file exists to prevent. Each
 * mirrors a rule in `crates/ruvyxa_graph/src/discovery.rs`, named on the function.
 */

/**
 * Extensions a route file written as a component may carry, in probe order.
 *
 * Mirrors `COMPONENT_EXTENSIONS` in `crates/ruvyxa_graph/src/discovery.rs`. The two
 * halves both used to pass a single literal `layout.tsx` / `template.tsx` down
 * their nested walk while accepting `page.jsx` as a route, so a project written
 * in `.jsx` lost every layout and template in both hosts at once — no
 * diagnostic, a successful build, and a page rendered without its
 * `<html>`/`<body>` shell. `.tsx` is probed first, so a project holding a stray
 * `layout.jsx` beside its `layout.tsx` composes the file it always did.
 *
 * Named for its Rust twin rather than in this file's own casing. It was
 * `componentExtensions` for one reason: `check-cross-language-constants.mjs`
 * pairs declarations by name, so a matching name would have failed as an
 * unregistered pair — and the fix was to rename the constant rather than to
 * register it. A rule kept out of the gate by its spelling is exactly what the
 * gate exists to stop.
 */
const COMPONENT_EXTENSIONS = ['tsx', 'jsx']

/** The file names `stem` may take, in probe order. */
function routeFileNames(stem, extensions = COMPONENT_EXTENSIONS) {
  return extensions.map((extension) => `${stem}.${extension}`)
}

/** `layout` files from the app root down to the route, root first. */
export function collectLayouts(appDir, routeDir) {
  return collectNested(appDir, routeDir, routeFileNames('layout'))
}

/**
 * `template` files from the app root down to the route, root first.
 *
 * Mirrors `template_chain()` in `crates/ruvyxa_graph/src/discovery.rs`.
 */
export function collectTemplates(appDir, routeDir) {
  return collectNested(appDir, routeDir, routeFileNames('template'))
}

/**
 * Files named by one of `fileNames` on the path from the app root to
 * `routeDir`, root first.
 *
 * One level contributes at most one entry: the names are one module spelled in
 * several extensions, and the first that exists wins. Mirrors `nested_chain()`
 * in `crates/ruvyxa_graph/src/discovery.rs`, held level with it by
 * `tests/fixtures/route-chain-conformance.json`.
 */
function collectNested(appDir, routeDir, fileNames) {
  const found = []
  let current = appDir

  pushFirstExisting(found, current, fileNames)

  const relative = path.relative(appDir, routeDir)
  if (relative && !relative.startsWith('..')) {
    for (const segment of relative.split(path.sep)) {
      if (!segment) continue
      current = path.join(current, segment)
      pushFirstExisting(found, current, fileNames)
    }
  }

  return found
}

/** Push the first of `fileNames` present in `directory`, if any. */
function pushFirstExisting(collection, directory, fileNames) {
  const found = fileNames
    .map((name) => path.join(directory, name))
    .find((candidate) => existsSync(candidate))
  if (found) collection.push(found)
}

/**
 * Parallel-route slots in scope for a route, level order then name order.
 *
 * Walks the same directory chain the layout and template chains do, and at each
 * level resolves every `@name` folder against the route's remaining segments: a
 * page inside the slot for that sub-path, else the slot's `default.tsx`, else
 * nothing at all. Mirrors `route_slots()` in `crates/ruvyxa_graph/src/parallel.rs`,
 * which decides the same thing for the Rust bundler — a slot one host composes
 * and the other does not is a panel that appears under `ruvyxa build` and
 * vanishes under `ruvyxa dev`.
 */
export function collectSlots(appDir, routeDir) {
  const relative = path.relative(appDir, routeDir)
  if (relative.startsWith('..')) return []
  const segments = relative ? relative.split(path.sep).filter(Boolean) : []

  const slots = []
  let level = appDir
  for (let depth = 0; depth <= segments.length; depth += 1) {
    if (depth > 0) level = path.join(level, segments[depth - 1])
    const remaining = segments.slice(depth)
    let names
    try {
      names = readdirSync(level, { withFileTypes: true })
        .filter(
          (entry) => entry.isDirectory() && entry.name.startsWith('@') && entry.name.length > 1,
        )
        .map((entry) => entry.name.slice(1))
        .sort()
    } catch {
      continue
    }
    for (const name of names) {
      const slotDir = path.join(level, `@${name}`)
      const file = slotPageFor(slotDir, remaining)
      if (file) slots.push({ level, name, file })
    }
  }
  return slots
}

/** The file a slot renders for the remaining URL segments, or null. */
function slotPageFor(slotDir, remaining) {
  const target = path.join(slotDir, ...remaining)
  for (const name of [...routeFileNames('page'), ...routeFileNames('page', ['md', 'mdx'])]) {
    const candidate = path.join(target, name)
    if (existsSync(candidate)) return candidate
  }
  for (const name of routeFileNames('default')) {
    const candidate = path.join(slotDir, name)
    if (existsSync(candidate)) return candidate
  }
  return null
}

/** File names of the special files a segment may declare, by kind. */
/**
 * Packages whose import is a declaration for the boundary checker and nothing
 * else, so no emitted bundle may import them.
 *
 * A browser bundle never reaches the rule: importing `server-only` there is
 * RUV1007 and the build fails before output exists.
 */
const MARKER_PACKAGES = new Set(['server-only', 'client-only'])

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
  /**
   * Prefix for the identifiers the linker mints — `__m0`, `__ext0`, and so on.
   *
   * Exists because a finished bundle is sometimes inlined into another one: the
   * deployed server-components registry is compiled with React left external
   * so it shares the renderer's instance, which means it can only get that
   * instance by being linked *into* the module that has it. Both bundles number
   * their modules from zero, so the inner `const __m1` landed in the same scope
   * as the outer module's `const __ext1 = __m1` and shadowed it — the whole
   * deployment failed to import with `Cannot access '__m1' before
   * initialization`, a temporal dead zone nothing in either bundle could see.
   *
   * The default is the original spelling, so every existing bundle is
   * byte-identical; only a caller that inlines its own output passes one.
   */
  identifierPrefix = '__',
  aliases = {},
  minify = false,
  /**
   * Pin `process.env.NODE_ENV` inside the emitted bundle.
   *
   * Only a deployment passes it, and it passes `'production'`: an artifact that
   * `ruvyxa build` wrote is a production build whichever way the host happens to
   * start it, and its browser half already says so unconditionally. Left null
   * the bundle reads the ambient value exactly as before, which is what
   * `ruvyxa dev` needs — see `nodeEnvPrelude`.
   */
  nodeEnv = null,
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
  // Resolved once per compile rather than per module: the hooks are the same
  // for the whole graph, and loading them is a cached pointer read.
  //
  // `markdownConfig === false` is the config compile itself — the pointer this
  // reads is produced by compiling the config, so running project plugins there
  // would be circular. Skipping it also means a plugin can never transform the
  // file that declares it.
  const buildTransform =
    markdownConfig === false
      ? null
      : await projectBuildTransform(root, pluginEnvironmentFor(platform, resolvedBundleTarget))
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
    identifierPrefix,
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
    buildTransform,
  })

  const linked = linkModules(modules, externals, {
    minify,
    outfile,
    sourceMap,
    externalUrls,
    nodeEnv,
  })
  assertLinkedSyntax(linked.code, outfile, linked.lineOrigins)
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
      compareCodePoints(left.id, right.id),
    ),
    // Every `'use server'` module, in the same stable order and for the same
    // reasons: the host reads it to know which files a server-function call may
    // resolve to, and a call naming a module no graph reported is refused
    // rather than loaded.
    serverReferences: [...serverReferences.values()].sort((left, right) =>
      compareCodePoints(left.id, right.id),
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
    ].sort(compareCodePoints),
    // Files that were looked for and not found. A lockfile or a tsconfig
    // appearing changes what a bare specifier resolves to without changing any
    // file that was read, so their absence is part of what was observed.
    absentFiles: [
      ...PROJECT_MANIFEST_FILES,
      ...(tsconfigPaths.files.length === 0 ? ['tsconfig.json', 'jsconfig.json'] : []),
    ]
      .map((name) => path.join(root, name))
      .filter((file) => !existsSync(file))
      .sort(compareCodePoints),
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

// Re-exported rather than defined here: it lives in `paths.mjs` so a template
// module can reach it without importing this one, which would pull the whole
// build system into any bundle that reaches a template. Every existing caller
// keeps importing it from here.
export { toImportPath }

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
        .filter((file) => file && isProjectLocal(root, file))
        .map((file) => path.relative(root, file).replaceAll('\\', '/')),
    ),
  ].sort()
}

async function fingerprintProjectInputs(root, modules, configurationFiles = []) {
  const hash = createHash('sha256')
  const projectModules = modules
    .filter((module) => module.filePath && isProjectLocal(root, module.filePath))
    .map((module) => ({
      path: path.relative(root, module.filePath).replaceAll('\\', '/'),
      source: module.source,
    }))
    .sort((left, right) => compareCodePoints(left.path, right.path))

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
    .filter((module) => module.filePath && isProjectLocal(root, module.filePath))
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
    // How a deployed route registry reaches the server-components renderer,
    // for the same reason and by the same mechanism as the line above.
    [RSC_RENDERER_SPECIFIER]: path.join(runtimeDir, 'server-components.mjs'),
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
    identifierPrefix,
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
    buildTransform,
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
  const id = `${identifierPrefix}m${modules.length}`
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

    // A marker package is a declaration, not a dependency. `server-only` and
    // `client-only` ship no runtime behaviour — importing one exists so the
    // boundary checker can see it, and that has already run. Registering it as
    // an external hoisted `import * as __ext0 from "server-only"` into the
    // bundle, so a deployed function directory, which carries no node_modules,
    // failed to start with ERR_MODULE_NOT_FOUND for a package whose only job
    // was to not be there. See `markerPackages` in
    // tests/fixtures/module-lane-conformance.json; the Rust linker drops the
    // same two in `is_marker_package`.
    if (MARKER_PACKAGES.has(specifier)) continue

    // A rewritten specifier is answered before resolution, not after. A browser
    // bundle inlines everything it can reach — that is what `platform:
    // 'browser'` means — so asking `shouldBundleResolved` about React would
    // always bundle it, and the URL the caller supplied would never be emitted.
    if (externalUrls?.[specifier]) {
      registerExternalDependency(module, specifier, null, externals, externalUrls, identifierPrefix)
      continue
    }

    // A specifier the caller named external is answered here for the same
    // reason. `external` used to hold only by accident: a server bundle leaves
    // packages alone anyway, so nobody noticed the list was never consulted —
    // until one asked for `bundlePackages` as well, resolved its own React
    // despite listing it, and rendered client components against a second copy
    // whose dispatcher was null.
    if (externalSet.has(specifier)) {
      registerExternalDependency(module, specifier, null, externals, externalUrls, identifierPrefix)
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
      assertImportCaseMatches(resolved, specifier, baseDir, filePath || sourcefile)
      const depSource = await readSourceFile(resolved, buildTransform)
      const dep = await visitModule({
        key: moduleGraphKey(resolved),
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
        identifierPrefix,
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
        buildTransform,
      })
      module.deps.set(specifier, dep)
      continue
    }

    registerExternalDependency(
      module,
      specifier,
      resolvedAlias,
      externals,
      externalUrls,
      identifierPrefix,
    )

    if (!externalSet.has(specifier) && specifier.startsWith('.')) {
      throw new Error(`RUV1801 cannot resolve '${specifier}' from ${filePath || sourcefile}`)
    }

    if (!resolved && !externalSet.has(specifier)) {
      const message = unresolvedBareSpecifierMessage(specifier, {
        baseDir,
        root,
        bundleTarget,
        importer: filePath || sourcefile,
      })
      if (message) throw new Error(message)
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
  identifierPattern(
    String.raw`(?:^|[;{}])\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*(%IDENT%)$`,
  ),
  identifierPattern(
    String.raw`(?:^|[;{}])\s*(?:export\s+)?(?:const|let|var)\s+(%IDENT%)\s*=\s*(?:async\s*)?$`,
  ),
  identifierPattern(
    String.raw`(?:^|[;{}])\s*(?:export\s+)?(?:const|let|var)\s+(%IDENT%)\s*=\s*(?:async\s+)?function\s*\*?\s*(?:%IDENT%)?$`,
  ),
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
 * `tsconfig` mapping (`paths`, then `baseUrl`), then a relative file, then
 * `node_modules` — and that last step only when the output is actually meant to
 * carry its dependencies.
 *
 * There is deliberately no project-root step for a bare specifier: a bare
 * specifier names a package, the way Node and `tsc` both read it, and `baseUrl`
 * is how a project asks for root-relative resolution out loud. `resolver.rs`
 * used to probe the project root between `tsconfig` and `node_modules` and this
 * graph never did, so one `import 'utils'` took `<root>/utils/index.ts` into the
 * client bundle while the dev server and every prerender worker took
 * `node_modules/utils`. The order both hosts hold is the `resolutionOrder`
 * section of `tests/fixtures/module-resolution-conformance.json`.
 */
export function resolveSpecifierPath(
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
 * Names directly under the project root, cached against the directory's mtime.
 *
 * `unresolvedBareSpecifierMessage` is asked about every bare specifier a build
 * leaves external — for a server bundle that is every package it imports — and
 * the file probe behind it costs some thirty `stat` calls. One `readdir`,
 * revalidated by one `stat`, answers "could the project root hold this at all?"
 * for all of them, so the probe only runs on the handful that could.
 */
const projectRootListings = new Map()

function projectRootNames(root) {
  try {
    const mtimeMs = statSync(root).mtimeMs
    const cached = projectRootListings.get(root)
    if (cached && cached.mtimeMs === mtimeMs) return cached.names
    const names = readdirSync(root)
    projectRootListings.set(root, { mtimeMs, names })
    return names
  } catch {
    return null
  }
}

/**
 * The project file a bare specifier would have named, or null.
 *
 * Compared without ASCII case, because `existsSync` answers that way on Windows
 * and on default macOS: the Rust half probes the filesystem directly and would
 * find `Utils/index.ts` for `utils` there, and a diagnostic that fires on one
 * host and not the other is the divergence this whole section exists to close.
 */
function projectRootShadow(root, specifier) {
  const first = specifier.split('/')[0]
  // A scheme (`node:fs`) or a scope (`@ruvyxa/react`) never names a file at the
  // project root, and on Windows `node:fs` is not even a legal path.
  if (!first || first.startsWith('@') || first.includes(':')) return null
  const names = projectRootNames(root)
  if (!names) return null
  const stem = `${first}.`
  const plausible = names.some(
    (name) =>
      equalIgnoringAsciiCase(name, first) ||
      equalIgnoringAsciiCase(name.slice(0, stem.length), stem),
  )
  if (!plausible) return null
  return probeFileCandidate(path.resolve(root, specifier))
}

/**
 * RUV1808: a bare specifier nothing answered, with a project file behind it.
 *
 * Removing the project-root probe from `resolver.rs` would otherwise have been
 * a *silent* change for any project that relied on it — the specifier becomes an
 * unresolved external, which no host reports and every host fails at run time.
 * So the drop is loud. The message names the file because the author wrote a
 * package specifier and meant a project file, and the two ways to say that — a
 * relative import, or `baseUrl`/`paths` — are both understood by `tsc`, by the
 * editor, and by both module graphs. The `crates/ruvyxa_bundler/src/resolver.rs`
 * half is `project_root_shadow_message`.
 */
export function unresolvedBareSpecifierMessage(
  specifier,
  { baseDir, root, bundleTarget, importer },
) {
  if (specifier.startsWith('.') || path.isAbsolute(specifier)) return null
  const shadow = projectRootShadow(root, specifier)
  if (!shadow) return null
  // Only when `node_modules` has nothing either. A server bundle never walks
  // packages, so an installed dependency reaches here unresolved and must not
  // be mistaken for a project file the author spelled as a package.
  if (resolvePackage(baseDir, specifier, bundleTarget, root)) return null
  return (
    `RUV1808 import '${specifier}' from ${importer} names no package, but ${shadow} exists. ` +
    'A bare specifier names a package here, the way Node and TypeScript both read it. Import the ' +
    'project file relatively, or declare `compilerOptions.baseUrl` (or a `paths` entry) in ' +
    'tsconfig.json so the type checker and the bundler answer it the same way.'
  )
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
function registerExternalDependency(
  module,
  specifier,
  resolvedAlias,
  externals,
  externalUrls,
  identifierPrefix = '__',
) {
  // A shared module keeps its own name as the key. The URL is where the browser
  // loads it from; the key is what the registry stores it under, and both ends
  // of that lookup are generated from this one string.
  const shared = Boolean(externalUrls?.[specifier])
  let externalSpecifier = specifier
  if (!shared && resolvedAlias) externalSpecifier = toImportPath(resolvedAlias)
  if (!externals.has(externalSpecifier)) {
    externals.set(externalSpecifier, `${identifierPrefix}ext${externals.size}`)
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

/**
 * The statement that pins a deployed bundle to the build it was compiled for.
 *
 * A deployment is a production artifact by construction: the browser half is
 * compiled by the Rust bundler, which folds `process.env.NODE_ENV` to
 * `"production"` and cannot be told otherwise (`linker.rs`). The server half
 * read the *ambient* value, and nothing in an emitted deployment sets one — so
 * `node server/index.mjs`, the documented way to run the Node adapter's output,
 * ran React's development build against a production browser bundle. For an
 * ordinary page that is a size and speed cost. For a server-components route it
 * is fatal and silent over HTTP: the document renders, every status code is
 * 200, and the browser throws `Failed to read a RSC payload created by a
 * development version of React` and shows a blank page. The development payload
 * also carries the build machine's absolute source paths — the smoke's document
 * was 11,420 bytes of which 9,878 were stack frames naming `D:/Ruvyxa/...`.
 *
 * Emitted once per bundle, ahead of every module factory, because that is the
 * earliest point inside a module body — a statement in the *entry* cannot do it,
 * since ESM evaluates imports before any statement of the importer. Each linked
 * bundle carries its own copy for the same reason: `route-modules.mjs` imports
 * the `react-server` bundle, so that sibling's body runs first and has to pin
 * the value for itself.
 */
function nodeEnvPrelude(nodeEnv) {
  return `if (globalThis.process?.env) globalThis.process.env.NODE_ENV = ${JSON.stringify(nodeEnv)};`
}

function linkModules(
  modules,
  externals,
  { minify, outfile, sourceMap, externalUrls, nodeEnv = null },
) {
  const out = []
  const lineMappings = []
  const mapSources = new Map()
  const push = (line, mapping = null) => {
    out.push(line)
    lineMappings.push(mapping)
  }

  writeExternalImports(push, externals, externalUrls)

  // Ahead of every module factory: React reads `NODE_ENV` while its own factory
  // runs, and a host that has a real `process` ignores the stand-in below.
  if (nodeEnv) {
    push(nodeEnvPrelude(nodeEnv))
    push('')
  }

  // Which modules sit in an import cycle, and with whom. A cycle is legal ESM
  // and common in published packages — `zod` has one between `schemas.js` and
  // `iso.js` — so the linker orders around it rather than refusing the graph.
  const cycles = findCycleGroups(modules)
  for (const module of modules) {
    module.cycleGroup = cycles.get(module.id) ?? null
  }

  const rewrittenModules = new Map(modules.map((module) => [module.id, rewriteModule(module)]))

  const ordered = orderModulesByDependencies(modules)
  // Where each cycle starts and finishes. Neither is always the module beside
  // the group: an acyclic dependency of one member can be emitted between two
  // of them.
  const { firstOfCycleGroup, lastOfCycleGroup, membersOfCycleGroup } = cycleLayout(ordered)
  if (ordered.some((module) => module.cycleGroup !== null)) writeCycleRuntime(push)

  for (const [position, module] of ordered.entries()) {
    const rewritten = rewrittenModules.get(module.id)
    const sourceIndex = sourceMap
      ? sourceMapIndex(mapSources, module.sourceName, module.source)
      : null

    // A module in a cycle publishes its exports object before its body runs, so
    // a module further into the cycle has something to hold. An acyclic module
    // keeps the original shape, so its bytes do not change.
    // A module that awaits in its own body needs an async wrapper, and the
    // bundle's top level — where the call sits — is allowed to await it.
    const awaits = module.cycleGroup === null && hasTopLevelAwait(rewritten.code)
    if (module.cycleGroup === null) {
      push(`const ${module.id} = ${awaits ? 'await (async () => {' : '(() => {'}`)
      push(`  const __exports = {};`)
    } else {
      // Every namespace in the group is declared before the first body runs:
      // the member that closes the cycle reads the one that opened it.
      if (firstOfCycleGroup.get(module.cycleGroup) === position) {
        for (const member of membersOfCycleGroup.get(module.cycleGroup)) {
          push(`const ${member.id} = {};`)
        }
      }
      push(`;(() => {`)
      push(`  const __exports = ${module.id};`)
    }
    const ownsModule = writeModuleScope(push, rewritten.code, nodeEnv)
    // The body is indented because it sits inside the wrapper, but a line that
    // *begins* inside a template literal or a `\`-continued string is text the
    // module means to keep: two spaces in front of it land in the string, not
    // in the source. Harmless for CSS-in-JS; wrong for YAML, a `<pre>` block,
    // a Markdown fence, or anything else indentation-significant. Both linkers
    // corrupted it identically, so the two halves agreed with each other and
    // disagreed with plain ESM — nothing ever surfaced as a mismatch.
    //
    // Asked of the one JavaScript scanner, per line start, which is the mirror
    // of `ModuleAst::is_code_offset` gating the same two spaces in
    // `crates/ruvyxa_bundler/src/linker.rs`.
    const codeIndex = createCodeIndex(rewritten.code)
    const codeLines = rewritten.code.split('\n')
    let lineStart = 0
    for (let index = 0; index < codeLines.length; index++) {
      const line = codeLines[index]
      const originalLine = rewritten.lineMap[index]
      const indent = line !== '' && codeIndex.isCode(lineStart)
      push(
        indent ? `  ${line}` : line,
        sourceIndex !== null && originalLine !== null ? { sourceIndex, originalLine } : null,
      )
      lineStart += line.length + 1
    }
    // `module.exports` unless the module declared its own `module`, in which
    // case that name means something else entirely and only `__exports` holds
    // what this module exported.
    const exportsExpression = ownsModule ? '__exports' : 'module.exports'
    if (module.cycleGroup === null) {
      push(`  return ${exportsExpression};`)
      push(`})();`)
    } else {
      // A CommonJS module in the cycle may have replaced `module.exports`
      // wholesale; the identity its importers hold is the one published above.
      push(
        `  if (${exportsExpression} !== __exports) Object.assign(__exports, ${exportsExpression});`,
      )
      push(`})();`)
    }
    push('')

    // The moment a cycle is complete is the moment its members can read each
    // other's named bindings, so the fixups run there rather than at the end.
    if (module.cycleGroup !== null && lastOfCycleGroup.get(module.cycleGroup) === position) {
      push(`__ruvyxaRebind.splice(0).forEach((rebind) => rebind());`)
      push('')
    }
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
  const sourceNames = [...mapSources.keys()]
  return {
    // Whitespace replacement is not JavaScript minification: it corrupts strings,
    // regexes, template literals, and line comments. Native production builds use
    // the Oxc minifier; the runtime compiler keeps generated code semantically exact.
    code,
    map: sourceMap ? buildSourceMap(outfile, lineMappings, mapSources) : null,
    // Where each emitted line came from, so a failure found in the linked text
    // can name project source rather than a line in a file nobody wrote. Built
    // whether or not a source map was asked for.
    lineOrigins: lineMappings.map((mapping) =>
      mapping ? { source: sourceNames[mapping.sourceIndex], line: mapping.originalLine } : null,
    ),
  }
}

/**
 * Emit the import (or registry read) for every external the bundle needs.
 *
 * A shared browser build hands the linker a URL per package and expects the
 * module to come out of a runtime registry; everything else is an ordinary
 * namespace import.
 */
function writeExternalImports(push, externals, externalUrls) {
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
    // a default import and a namespace read both find what they look for.
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
  if (reactAlias) push(`const React = ${defaultImportExpression(reactAlias)};`)
  if (externals.size > 0) push('')
}

/**
 * Where each import cycle starts and finishes in the emitted order, and who
 * belongs to it.
 *
 * Neither end is always the module beside the group: an acyclic dependency of
 * one member can be emitted between two of them.
 */
function cycleLayout(ordered) {
  const firstOfCycleGroup = new Map()
  const lastOfCycleGroup = new Map()
  const membersOfCycleGroup = new Map()
  for (const [position, module] of ordered.entries()) {
    if (module.cycleGroup === null) continue
    if (!firstOfCycleGroup.has(module.cycleGroup))
      firstOfCycleGroup.set(module.cycleGroup, position)
    lastOfCycleGroup.set(module.cycleGroup, position)
    const members = membersOfCycleGroup.get(module.cycleGroup) ?? []
    members.push(module)
    membersOfCycleGroup.set(module.cycleGroup, members)
  }
  return { firstOfCycleGroup, lastOfCycleGroup, membersOfCycleGroup }
}

/**
 * Declare what a cyclic import needs at run time.
 *
 * `__ruvyxaRebind` holds the re-reads that give a cyclic binding its value
 * once its group has finished initialising, and `__ruvyxaCycleTdz` is what one
 * holds until then — ESM answers a read of an unfinished binding with a
 * ReferenceError, and the linked form would otherwise answer `undefined` and
 * carry on with nothing to trace it back to.
 */
function writeCycleRuntime(push) {
  // Bindings a cyclic import could not read yet, re-read once its group has
  // finished initialising. See `rewriteImportClause`.
  push(`const __ruvyxaRebind = [];`)
  // What such a binding holds until then. ESM answers a read of a binding
  // whose module has not finished with a ReferenceError, and the linked form
  // would otherwise answer `undefined` and carry on — the same wrong value,
  // with nothing to trace it back to.
  push(
    `const __ruvyxaCycleTdz = (name, from) => new Proxy(function () {}, ` +
      `{ get(target, key) { if (key === Symbol.toStringTag) return 'Uninitialized'; ` +
      `throw new ReferenceError(\`Cannot access '\${name}' before initialization: it is imported from \${from}, which imports this module back, and the value is read while that cycle is still running.\`) }, ` +
      `apply() { throw new ReferenceError(\`Cannot call '\${name}' before initialization (import cycle with \${from}).\`) }, ` +
      `construct() { throw new ReferenceError(\`Cannot construct '\${name}' before initialization (import cycle with \${from}).\`) } });`,
  )
  push('')
}

/**
 * Declare the CommonJS-shaped scope a wrapped module body expects.
 *
 * A module that declares `module`, `exports`, or `process` itself keeps its
 * own — `zod` imports a function called `process` from a sibling — because the
 * wrapper would otherwise redeclare the name in the same scope and the whole
 * chunk fails to parse with an error naming a line the author never wrote.
 *
 * Returns whether the module owns `module`, which decides where its exports
 * are read from afterwards.
 */
function writeModuleScope(push, code, nodeEnv) {
  // A module that declares one of these itself keeps its own; the wrapper
  // would otherwise redeclare a name in the same scope and the bundle would
  // not parse.
  const declared = topLevelDeclaredNames(code)
  const ownsModule = declared.has('module')
  if (!ownsModule) push(`  const module = { exports: __exports };`)
  if (!declared.has('exports')) {
    push(`  const exports = ${ownsModule ? '__exports' : 'module.exports'};`)
  }
  if (!declared.has('process')) {
    push(
      `  const process = globalThis.process ?? { env: { NODE_ENV: ${JSON.stringify(nodeEnv ?? browserNodeEnv())} } };`,
    )
  }
  return ownsModule
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
  const onStack = new Set()
  const visited = new Set()

  const visit = (module) => {
    if (visited.has(module.id)) return
    // A dependency already being initialised further up this walk is a cycle
    // edge. ESM does not re-enter one either: evaluation is a depth-first
    // post-order, and the module that closes the cycle sees the partially
    // filled namespace of the one that started it. Refusing the graph instead
    // — which this did — made every package with an internal cycle, `zod`
    // among them, impossible to bundle at all.
    if (onStack.has(module.id)) return

    onStack.add(module.id)
    for (const dependency of module.deps.values()) {
      if (!dependency.external) visit(dependency)
    }
    onStack.delete(module.id)
    visited.add(module.id)
    ordered.push(module)
  }

  for (const module of modules) visit(module)
  return ordered
}

/**
 * Group the modules that sit in an import cycle, by Tarjan's algorithm.
 *
 * Returns a map from module id to a group id, holding only modules in a cycle:
 * a strongly connected component with more than one member, or a module that
 * imports itself. Everything else links exactly as it did before, which is what
 * keeps an ordinary bundle's bytes unchanged.
 */
function findCycleGroups(modules) {
  const index = new Map()
  const lowLink = new Map()
  const onStack = new Set()
  const stack = []
  const groups = new Map()
  let counter = 0
  let groupId = 0

  const strongConnect = (module) => {
    index.set(module.id, counter)
    lowLink.set(module.id, counter)
    counter += 1
    stack.push(module)
    onStack.add(module.id)

    let selfReferential = false
    for (const dependency of module.deps.values()) {
      if (dependency.external) continue
      if (dependency.id === module.id) selfReferential = true
      if (!index.has(dependency.id)) {
        strongConnect(dependency)
        lowLink.set(module.id, Math.min(lowLink.get(module.id), lowLink.get(dependency.id)))
      } else if (onStack.has(dependency.id)) {
        lowLink.set(module.id, Math.min(lowLink.get(module.id), index.get(dependency.id)))
      }
    }

    if (lowLink.get(module.id) !== index.get(module.id)) return
    const component = []
    let member
    do {
      member = stack.pop()
      onStack.delete(member.id)
      component.push(member)
    } while (member.id !== module.id)
    if (component.length > 1 || selfReferential) {
      groupId += 1
      for (const each of component) groups.set(each.id, groupId)
    }
  }

  for (const module of modules) {
    if (!index.has(module.id)) strongConnect(module)
  }
  return groups
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

/**
 * The identity two paths to one file must share.
 *
 * pnpm links a package into every dependent's `node_modules`, so one physical
 * `react` is reachable by several paths — `examples/demo/node_modules/react`,
 * `packages/@ruvyxa/react/node_modules/react`, the workspace root's — and
 * keying the graph by the path that was walked put **five** React instances in
 * one server bundle. Ordinary SSR survived that by luck: its components and
 * `react-dom/server` happened to land on the same copy. The server-components
 * SSR pass did not, and every client component in a deployed RSC route threw
 * `Cannot read properties of null (reading 'useRef')` — React's "more than one
 * copy" failure, with nothing in the bundle naming the cause.
 *
 * Only the graph key is normalized. `filePath` stays the path that was resolved
 * because client-reference ids are measured from it, and those must keep naming
 * a module the way the browser graph names it.
 */
function moduleGraphKey(resolved) {
  try {
    return realpathSync(resolved)
  } catch {
    // A path that cannot be resolved is its own identity; the read that
    // follows reports the real problem.
    return resolved
  }
}

/**
 * Emit every line one statement consumed exactly as it was written.
 *
 * Used where a rewrite produced nothing trustworthy: the masked text a rewriter
 * reads has its string literals blanked, so emitting *that* would delete the
 * module's own strings. Returns the last line consumed.
 */
function passThroughStatement({ module, sourceLines, lines, lineMap }, from, to) {
  for (let at = from; at <= to; at += 1) {
    lines.push(sourceLines[at])
    lineMap.push(module.transformLineMap?.[at] ?? at)
  }
  return to
}

/** Rewrite one import statement, however many lines its clause spans. */
function writeRewrittenImport(statements, sourceLine) {
  const { module, codeLines, lines, lineMap } = statements
  const statement = gatherClauseStatement(codeLines, sourceLine)
  const rewritten = rewriteImport(statement.text, module)
  // `null` means the text only looked like an import.
  if (rewritten === null) {
    passThroughStatement(statements, sourceLine, statement.endLine)
  } else if (rewritten) {
    lines.push(rewritten)
    lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
  }
  return statement.endLine
}

/**
 * Rewrite `export default <expression>` into an assignment.
 *
 * The expression is collected until it is complete, because a default export is
 * routinely an object, a call, or a template that spans lines, and half of one
 * assigned to `__exports.default` either does not parse or — the expensive case
 * — parses and means something else.
 *
 * The collected lines are joined *raw*, exactly as written. They used to be
 * trimmed, which is invisible for an object literal and destructive for a
 * template: the indentation of a continuation line inside a template literal is
 * part of the string the module exports, and trimming rewrote it. For the same
 * reason the statement is emitted across its own lines with the `;` after the
 * real end of the expression, rather than folded onto the first one.
 */
function writeRewrittenDefaultExport(statements, sourceLine) {
  const { module, sourceLines, lines, lineMap } = statements
  const collected = [sourceLines[sourceLine]]
  let endLine = sourceLine
  while (!isCompleteDefaultExpression(collected) && endLine + 1 < sourceLines.length) {
    endLine += 1
    collected.push(sourceLines[endLine])
  }

  // Only the keyword and a trailing `;` are removed. Everything between them is
  // the author's text, and inside a template literal every byte of it is a byte
  // of the exported string.
  const expression = collected
    .join('\n')
    .replace(/^\s*export\s+default\s+/, '')
    .replace(/;\s*$/, '')
  const statement = `__exports.default = ${rewriteDynamicImports(expression, module)};`
  // Pushed one physical line at a time so `lineMap` keeps one entry per emitted
  // line. A multi-line statement pushed as a single entry shifts the origin of
  // every line after it in the module, which is how a stack trace comes to name
  // the wrong source line.
  const statementLines = statement.split('\n')
  for (let index = 0; index < statementLines.length; index += 1) {
    const origin = Math.min(sourceLine + index, endLine)
    lines.push(statementLines[index])
    lineMap.push(module.transformLineMap?.[origin] ?? origin)
  }
  return endLine
}

/** Rewrite one export statement, clause form or declaration form. */
function writeRewrittenExport(statements, sourceLine, exported, reExportAll) {
  const { module, sourceLines, codeLines, lines, lineMap } = statements
  // A clause form (`export { … }`, `export * …`) is rewritten into generated
  // assignments, so it is read from the joined masked statement. A declaration
  // form (`export const x = {`) keeps its own text and its continuation lines
  // pass through untouched, as they always have.
  const isClause = /^export\s*(?:\{|\*)/.test((codeLines[sourceLine] ?? '').trim())
  const statement = isClause
    ? gatherClauseStatement(codeLines, sourceLine)
    : { text: sourceLines[sourceLine].trim(), endLine: sourceLine }
  const result = rewriteExport(statement.text, module, exported, reExportAll)
  if (isClause && result === statement.text) {
    passThroughStatement(statements, sourceLine, statement.endLine)
  } else if (result) {
    lines.push(result)
    lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
  }
  return statement.endLine
}

function rewriteModule(module) {
  const rewriteKey = [
    module.key,
    createHash('sha256').update(module.source).digest('hex'),
    module.jsxRuntime,
    module.reactCompiler ? 'react-compiler' : 'baseline',
    // Which imports close a cycle changes what this rewrite emits, so it is
    // part of the identity of the result. A key that named only the dependency
    // ids would hand a cyclic graph the copy-binding rewrite it cached earlier.
    `cycle:${module.cycleGroup ?? 'none'}`,
    [...module.deps.entries()]
      .map(
        ([specifier, dep]) =>
          `${specifier}:${dep.external ? dep.alias : dep.id}:${dep.cycleGroup ?? 'none'}`,
      )
      .join('|'),
  ].join('\0')
  const cached = compilerCache.rewrites.get(rewriteKey)
  if (cached) return cached

  const source = module.transformedSource ?? transformModuleSource(module)
  // Specifiers stay readable in the mask because an `import`/`export` clause is
  // rewritten from the masked text, not from the raw line: a clause wrapped
  // across lines has to be joined before it can be read, and joining raw lines
  // would fold a `// comment` between them over everything after it.
  const codeOnly = maskNonCode(source, { preserveImportExportSpecifiers: true })

  const lines = []
  const lineMap = []
  const exported = []
  const reExportAll = []

  const sourceLines = source.split('\n')
  const codeLines = codeOnly.split('\n')
  // What every statement rewriter below reads and writes. Passed as one value
  // because they are one walk over one module, and threading five parameters
  // through each of them said nothing the name does not.
  const statements = { module, sourceLines, codeLines, lines, lineMap }
  for (let sourceLine = 0; sourceLine < sourceLines.length; sourceLine++) {
    const rawLine = sourceLines[sourceLine]
    const line = (codeLines[sourceLine] ?? '').trim()
    if (!line) {
      lines.push(rawLine)
      lineMap.push(module.transformLineMap?.[sourceLine] ?? sourceLine)
      continue
    }

    if (IMPORT_STATEMENT.test(line)) {
      sourceLine = writeRewrittenImport(statements, sourceLine)
      continue
    }

    if (/^export\s+default\b/.test(line) && !line.startsWith('export default function ')) {
      sourceLine = writeRewrittenDefaultExport(statements, sourceLine)
      continue
    }

    if (EXPORT_STATEMENT.test(line)) {
      sourceLine = writeRewrittenExport(statements, sourceLine, exported, reExportAll)
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
      .map((item) => item.match(identifierPattern(String.raw`__exports\.(%IDENT%)\s=`))?.[1])
      .filter(Boolean),
    reExportAll,
  }
  setBoundedCacheEntry(compilerCache.rewrites, rewriteKey, result)
  return result
}

/**
 * Names a module declares at its own top level.
 *
 * The wrapper each module is linked into declares `module`, `exports`, and
 * `process` for it, and a module that declares one of those itself collided
 * with the wrapper: `zod` has a top-level `function process(schema, ctx)`, and
 * the emitted bundle failed to parse with "Identifier 'process' has already
 * been declared" — from a file the author never wrote. Depth is counted over
 * the mask, so a brace inside a string cannot close a block early.
 */
function topLevelDeclaredNames(code) {
  const masked = maskNonCode(code)
  const declaration = identifierPattern(
    String.raw`^(?:function|class|const|let|var)\s*\*?\s+(%IDENT%)`,
  )
  const names = new Set()
  let depth = 0
  for (let index = 0; index < masked.length; index += 1) {
    const character = masked[index]
    if (character === '{' || character === '(' || character === '[') {
      depth += 1
      continue
    }
    if (character === '}' || character === ')' || character === ']') {
      depth -= 1
      continue
    }
    if (depth !== 0 || !/[a-z]/.test(character)) continue
    const before = masked[index - 1]
    if (before && /[\w$.]/.test(before)) continue
    const match = declaration.exec(masked.slice(index, index + 200))
    if (match) names.add(match[1])
  }
  return names
}

/**
 * An `import` statement, and not the reserved word used as something else.
 *
 * A reserved word is a legal property name, so `{ import: './x' }` opens a line
 * with `import` and is not a statement; neither is `import(…)` or
 * `import.meta`. The distinction never mattered while a non-statement fell
 * through returning its own text, and mattered immediately once the rewriter
 * started reading the masked line, where a string literal is blanked.
 */
const IMPORT_STATEMENT = /^import(?=[\s{*"'])(?!\s*:)/

/** An `export` statement, held apart from `{ export: … }` the same way. */
const EXPORT_STATEMENT = /^export(?=[\s{*])(?!\s*:)/

/**
 * Join an `import`/`export` clause statement that spans lines into one.
 *
 * Both linkers rewrite a line at a time. A clause list wrapped across lines —
 * what Prettier produces, and what `chalk` ships — used to have its first line
 * rewritten and its continuation lines copied through as bare tokens, so the
 * bundle carried `modifierNames as modifiers, … } from "…"` on its own and did
 * not parse. The build reported success; the deployed server died on the
 * `SyntaxError` at startup.
 *
 * Lines are joined from the mask, where a comment between clause members is
 * already blanked and the specifier is preserved.
 */
function gatherClauseStatement(codeLines, start) {
  const parts = []
  let endLine = start
  while (endLine < codeLines.length) {
    parts.push((codeLines[endLine] ?? '').trim())
    if (isCompleteClauseStatement(parts.join(' ').trim(), codeLines, endLine)) break
    endLine += 1
  }
  return {
    text: parts.join(' ').replace(/\s+/g, ' ').trim(),
    endLine: Math.min(endLine, codeLines.length - 1),
  }
}

/** Whether a gathered `import`/`export` statement needs no further line. */
function isCompleteClauseStatement(text, codeLines, endLine) {
  let depth = 0
  for (const character of text) {
    if (character === '{' || character === '[' || character === '(') depth += 1
    else if (character === '}' || character === ']' || character === ')') depth -= 1
  }
  if (depth > 0) return false
  const trimmed = text.trim()
  if (trimmed.endsWith(',')) return false
  // `from` written, specifier still to come.
  if (/\bfrom\s*$/.test(trimmed)) return false
  // A closed clause with no `from` may still be followed by one.
  if (trimmed.endsWith('}') && !/\bfrom\b/.test(trimmed)) {
    for (let next = endLine + 1; next < codeLines.length; next += 1) {
      const candidate = codeLines[next].trim()
      if (!candidate) continue
      return !/^from\b/.test(candidate)
    }
  }
  return true
}

/**
 * An identifier appended after collected text to ask the mask where it ends.
 *
 * `maskNonCode` blanks a literal's delimiters along with its text, so a closed
 * template and an open one both end in blanks and the mask alone cannot be
 * asked which it was. A probe past the end can: it survives as code when every
 * literal, template, and block comment the text opened has been closed, and is
 * blanked when one is still open. The name is deliberately unlikely so that a
 * source ending mid-token cannot join with it into something meaningful.
 */
const LITERAL_PROBE = '\n__ruvyxaLiteralProbe'

/**
 * Whether a collected `export default` expression needs no further line.
 *
 * Completeness used to be bracket depth alone, which a template literal does
 * not contribute to: an open template read as balanced, the collector stopped
 * on the statement's first line, and the `;` this rewriter appends landed
 * *inside the exported string*. The module still parsed — the lines that
 * followed were swallowed by the unterminated backtick — so a default-exported
 * prompt, SQL statement, or CSS block shipped a character its author never
 * wrote, with no diagnostic.
 *
 * So the question is asked of the mask, which is this file's only answer to
 * where a literal ends: brackets are counted over masked text, where a brace
 * inside a string cannot close anything, and a literal left open keeps the
 * statement incomplete. One mask serves both halves.
 */
function isCompleteDefaultExpression(rawLines) {
  const text = rawLines.join('\n')
  const masked = maskNonCode(text + LITERAL_PROBE)
  if (!masked.endsWith(LITERAL_PROBE)) return false
  let depth = 0
  for (let index = 0; index < text.length; index += 1) {
    const character = masked[index]
    if (character === '(' || character === '{' || character === '[') depth += 1
    else if (character === ')' || character === '}' || character === ']') depth -= 1
  }
  return depth <= 0
}

function rewriteImport(line, module) {
  if (/^import\s+type\b/.test(line)) return ''
  if (/^import\s+["']/.test(line)) return ''

  const match = line.match(/^import\s+(.+?)\s+from\s+["'](.+?)["'];?$/)
  if (!match) return null

  const [, clause, specifier] = match
  const source = module.deps.get(specifier)
  if (!source) return ''

  const sourceRef = source.external ? source.alias : source.id
  // An import that closes a cycle reads a namespace whose body has not run yet,
  // so a copied binding would hold `undefined` for the life of the bundle. The
  // binding is re-read once the cycle finishes instead, which is the closest a
  // concatenating linker gets to ESM's live bindings. A namespace import needs
  // none of this: it holds the object itself.
  const cyclic =
    !source.external && module.cycleGroup !== null && module.cycleGroup === source.cycleGroup
  return rewriteImportClause(clause, sourceRef, cyclic)
}

function rewriteExport(line, module, exported, reExportAll) {
  line = rewriteDynamicImports(line, module)

  if (line.startsWith('export default function ')) {
    const name = line.match(
      identifierPattern(String.raw`^export\s+default\s+function\s+(%IDENT%)`),
    )?.[1]
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
    const name = line.match(
      identifierPattern(String.raw`^export\s+(?:const|let|var)\s+(%IDENT%)`),
    )?.[1]
    if (name) exported.push(`__exports.${name} = ${name};`)
    // A destructuring declaration binds several names and matches no single
    // one, so it used to export nothing at all. The Rust linker reads them
    // with `destructured_binding_names`; this is the same walk.
    else {
      for (const bound of destructuredBindingNames(line.replace(/^export\s+/, ''))) {
        exported.push(`__exports.${bound} = ${bound};`)
      }
    }
    return line.replace(/^export\s+/, '')
  }

  // `\s*\*?\s*` rather than `\s+`: a generator's `*` binds to the keyword, so
  // `export function* stream()` and `export async function* stream()` matched
  // nothing here and fell through with their `export` intact. Node then parsed
  // the wrapped module and reported `RUV1700 Unexpected token 'export'` from
  // inside generated code. `declared_lane`'s neighbour in
  // `crates/ruvyxa_bundler/src/linker.rs` had the same blind spot, written as a
  // list of prefixes with trailing spaces — one bug, once per module graph.
  if (identifierPattern(String.raw`^export\s+(?:async\s+)?function\s*\*?\s*%IDENT%`).test(line)) {
    const name = line.match(
      identifierPattern(String.raw`^export\s+(?:async\s+)?function\s*\*?\s*(%IDENT%)`),
    )?.[1]
    if (name) exported.push(`__exports.${name} = ${name};`)
    return line.replace(/^export\s+/, '')
  }

  if (/^export\s+class\s+/.test(line)) {
    const name = line.match(identifierPattern(String.raw`^export\s+class\s+(%IDENT%)`))?.[1]
    if (name) exported.push(`__exports.${name} = ${name};`)
    return line.replace(/^export\s+/, '')
  }

  // The re-export form, recognised by matching it rather than by looking for
  // the word `from` anywhere in the line. `export { source as from }` renames a
  // binding *to* `from`, which is a perfectly ordinary identifier: the substring
  // test claimed it, the pattern below then failed, and the export was dropped
  // with no diagnostic — the importing module simply saw `undefined`.
  const reExport = line.match(/^export\s+(.+?)\s+from\s+["'](.+?)["'];?$/)
  if (reExport) {
    const match = reExport
    const [, clause, specifier] = match
    const source = module.deps.get(specifier)
    if (!source) return ''
    const sourceRef = source.external ? source.alias : source.id
    // A re-export that closes a cycle copies from a namespace whose body has
    // not run; it is copied again once the cycle finishes. See `rewriteImport`.
    const cyclic =
      !source.external && module.cycleGroup !== null && module.cycleGroup === source.cycleGroup
    const rebind = (statement) =>
      cyclic ? `${statement} __ruvyxaRebind.push(() => { ${statement} });` : statement
    if (clause.trim() === '*') {
      if (!source.external) reExportAll.push(source.id)
      return rebind(`Object.assign(__exports, ${sourceRef});`)
    }
    // `export * as ns from '…'` names the namespace object. Read as a named
    // binding list it produced `__exports.ns = __m8.*`, which does not parse —
    // `zod` re-exports its `util` module exactly this way.
    const namespaceAlias = clause.trim().match(identifierPattern(String.raw`^\*\s+as\s+(%IDENT%)$`))
    if (namespaceAlias) {
      const assignment = `__exports.${namespaceAlias[1]} = ${sourceRef};`
      exported.push(assignment)
      return assignment
    }
    const assignments = parseNamedBindings(clause).map(([original, alias]) => {
      const assignment = `__exports.${alias} = ${sourceRef}.${original};`
      exported.push(assignment)
      return rebind(assignment)
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

function rewriteImportClause(clause, sourceRef, cyclic = false) {
  /**
   * A binding, plus — when the import closes a cycle — the re-read that gives
   * it its value once the cycle finishes, and a stand-in that refuses to be
   * used until then.
   */
  const bind = (name, expression) =>
    cyclic
      ? `let ${name} = ${expression} ?? __ruvyxaCycleTdz(${JSON.stringify(name)}, ${JSON.stringify(sourceRef)}); ` +
        `__ruvyxaRebind.push(() => { ${name} = ${expression}; });`
      : `const ${name} = ${expression};`

  const cleaned = clause.trim()
  if (cleaned.startsWith('* as ')) {
    // The namespace object itself, which is published before the cycle runs.
    return `const ${cleaned.slice(5).trim()} = ${sourceRef};`
  }
  if (cleaned.startsWith('{')) {
    return parseNamedBindings(cleaned)
      .map(([original, alias]) => bind(alias, `${sourceRef}.${original}`))
      .join(' ')
  }
  if (cleaned.includes(',')) {
    const [defaultName, rest] = cleaned.split(/,(.+)/)
    return [
      bind(defaultName.trim(), defaultImportExpression(sourceRef)),
      rewriteImportClause(rest.trim(), sourceRef, cyclic),
    ].join(' ')
  }
  return bind(cleaned, defaultImportExpression(sourceRef))
}

/**
 * The expression a default import reads.
 *
 * `X.default ?? X` looks equivalent and is not. A CommonJS module that assigns
 * `module.exports = undefined` — which lodash's `_WeakMap.js` does on any
 * runtime where its native-function check fails — records `default: undefined`,
 * and `??` then falls through to the exports *object*. The importer receives a
 * truthy object where the module said `undefined`, so the guard written for
 * exactly that case (`WeakMap && new WeakMap()`) passes and the `new` throws
 * `Object is not a constructor`. Node happened to satisfy the native check and
 * Bun did not, so a deployment built from identical sources started on one
 * runtime and died on the other.
 *
 * Asking whether the property exists is closer to the question, but not the
 * whole of it: a `'use client'` module is replaced by a `Proxy` whose `get`
 * mints a client reference for any name, and `in` on that target answers
 * `false` while `.default` answers correctly. Reading the value first and
 * falling back to the property check keeps all three straight — a live default,
 * a deliberate `undefined`, and a module with only named exports. The leading
 * null check covers the fourth: a CommonJS module may assign
 * `module.exports = undefined` outright, and reading any property off that
 * throws before the interop can decide anything.
 */
/**
 * Whether a module's own body awaits — as opposed to awaiting inside one of its
 * functions.
 *
 * Every module is emitted as an immediately-invoked function, and `await` is
 * illegal in a synchronous one. A dependency that uses top-level await (an
 * ESM-only package initialising a WASM module, a route that awaits a dynamic
 * import) therefore produced a bundle that would not parse — a hard build
 * failure with the module named, but a failure all the same.
 *
 * The wrapper is made `async` only for the modules that need it, and awaited at
 * the call site, which is the bundle's own top level and may await. A module
 * that does not use the construct keeps the bytes it had.
 *
 * Depth-counted rather than pattern-matched: `await` inside a function body is
 * ordinary and must not count. The one shape this over-reports is a
 * brace-less async arrow (`async () => await x`), where the token sits at depth
 * zero inside a function; the cost of being wrong that way is a wrapper that
 * awaits a promise it did not need to, which changes nothing an application can
 * observe.
 */
function hasTopLevelAwait(code) {
  const masked = maskNonCode(code)
  let depth = 0
  for (let index = 0; index < masked.length; index++) {
    const character = masked[index]
    if (character === '{' || character === '(' || character === '[') depth += 1
    else if (character === '}' || character === ')' || character === ']') depth -= 1
    else if (depth === 0 && character === 'a' && masked.startsWith('await', index)) {
      const before = masked[index - 1]
      const after = masked[index + 5]
      const boundary = (value) => value === undefined || !/[\w$]/.test(value)
      if (boundary(before) && boundary(after)) return true
    }
  }
  return false
}

function defaultImportExpression(sourceRef) {
  return (
    `${sourceRef} == null ? ${sourceRef} : ` +
    `(${sourceRef}.default !== undefined || "default" in ${sourceRef} ? ${sourceRef}.default : ${sourceRef})`
  )
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

/**
 * Names bound by a destructuring declaration: `const { a, b: c } = …`.
 *
 * Empty for anything that is not a destructuring pattern, and for a pattern
 * whose closing delimiter is not in the text handed over — the caller rewrites
 * one statement at a time and cannot see past it. Mirrors
 * `destructured_binding_names` in `crates/ruvyxa_bundler/src/linker.rs`.
 */
function destructuredBindingNames(declaration) {
  const rest = declaration.trim().replace(/^(?:const|let|var)\s+/, '')
  if (rest === declaration.trim()) return []
  const pattern = balancedPattern(rest.trimStart())
  if (!pattern) return []
  const names = []
  collectPatternNames(pattern, names)
  return names
}

/**
 * The leading `{…}` or `[…]` of `source`, when it closes within the text.
 *
 * Delimiters are counted over `maskNonCode`, so a brace inside a string or a
 * comment cannot close the pattern early.
 */
function balancedPattern(source) {
  const open = source[0]
  if (open !== '{' && open !== '[') return null
  const close = open === '{' ? '}' : ']'
  const masked = maskNonCode(source)
  let depth = 0
  for (let index = 0; index < masked.length; index += 1) {
    if (masked[index] === open) depth += 1
    else if (masked[index] === close) {
      depth -= 1
      if (depth === 0) return source.slice(0, index + 1)
    }
  }
  return null
}

/**
 * Collect the identifiers a destructuring pattern introduces.
 *
 * Object elements bind the target after `:` when there is one and the key
 * otherwise; array elements bind their own target; `...rest` binds `rest`; a
 * default (`= expr`) belongs to the target, not to the names. Nested patterns
 * recurse, which is why `{ a: { b } }` reports `b` and not `a`.
 */
function collectPatternNames(pattern, names) {
  for (const raw of splitTopLevel(pattern.slice(1, -1))) {
    const element = raw
      .trim()
      .replace(/^\.\.\./, '')
      .trim()
    if (!element) continue
    const afterKey = splitTopLevelOnce(element, ':')
    const withDefault = afterKey === null ? element : afterKey[1].trim()
    const beforeDefault = splitTopLevelOnce(withDefault, '=')
    const target = (beforeDefault === null ? withDefault : beforeDefault[0]).trim()
    if (target.startsWith('{') || target.startsWith('[')) {
      const nested = balancedPattern(target)
      if (nested) collectPatternNames(nested, names)
      continue
    }
    if (isIdentifier(target)) names.push(target)
  }
}

/** Split on commas that sit at depth zero of `source`. */
function splitTopLevel(source) {
  const masked = maskNonCode(source)
  const parts = []
  let depth = 0
  let start = 0
  for (let index = 0; index < masked.length; index += 1) {
    const character = masked[index]
    if (character === '{' || character === '[' || character === '(') depth += 1
    else if (character === '}' || character === ']' || character === ')') depth -= 1
    else if (character === ',' && depth === 0) {
      parts.push(source.slice(start, index))
      start = index + 1
    }
  }
  parts.push(source.slice(start))
  return parts
}

/** Split `source` once on the first `separator` at depth zero. */
function splitTopLevelOnce(source, separator) {
  const masked = maskNonCode(source)
  let depth = 0
  for (let index = 0; index < masked.length; index += 1) {
    const character = masked[index]
    if (character === '{' || character === '[' || character === '(') depth += 1
    else if (character === '}' || character === ']' || character === ')') depth -= 1
    else if (character === separator && depth === 0) {
      // `=>` and `==` are not an assignment; neither is `:` inside `?:`.
      if (separator === '=' && (masked[index + 1] === '=' || masked[index + 1] === '>')) continue
      if (separator === '=' && masked[index - 1] === '=') continue
      return [source.slice(0, index), source.slice(index + 1)]
    }
  }
  return null
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

/**
 * Whether `resolved` really lives inside `pkgDir`, symlinks included.
 *
 * `isSafePackageRelativePath` rules out every *lexical* escape -- `..`, an
 * absolute path, a backslash -- and that is where both resolvers stopped. A
 * symlink is not lexical: `./dist/x.js` inside the package can point anywhere on
 * the disk, and `path.join` plus `existsSync` accept it. The Rust mirrors take
 * the canonicalization they need for the existence probe anyway and reuse it for
 * containment (`candidate.starts_with(package_root)`), so the two module graphs
 * answered the same import differently -- which is the failure this repository's
 * resolution rules exist to prevent.
 *
 * The returned path is deliberately *not* changed to the real one. Rust returns
 * the canonical path and this graph the lexical one, so under pnpm the same
 * module is named by its symlink path here and its store path there; correcting
 * that moves `module.filePath`, and with it `isProjectLocal`, `projectInputPaths`,
 * `readFiles`, and every client-reference id measured from a package file. The
 * containment check is the half that closes the hole, and it goes first.
 *
 * A path that cannot be realpath'd falls back to the lexical comparison rather
 * than being refused, matching `realImporterDir` above: the file's existence has
 * already been established by the probe, so a failure here is a filesystem
 * fault, not an escape.
 */
function containedInPackage(pkgDir, resolved) {
  if (resolved === null) return null
  const real = (value) => {
    try {
      return realpathSync(value)
    } catch {
      return path.resolve(value)
    }
  }
  const root = real(pkgDir)
  const candidate = real(resolved)
  // `path.relative` rather than a string prefix: `pkg-extra` must not count as
  // inside `pkg`, which a `startsWith` on the directory name would accept.
  const inside = path.relative(root, candidate)
  if (inside === '' || inside.startsWith('..') || path.isAbsolute(inside)) return null
  return resolved
}

/** Probe a package-relative path, refusing anything that escapes the package. */
// Exported for `tests/packages/ruvyxa/package-containment.test.mjs`, which
// replays the symlink-escape case against this resolver and its Rust mirror.
export function resolvePackageRelative(pkgDir, relative) {
  if (!isSafePackageRelativePath(relative)) return null
  return containedInPackage(pkgDir, probeFileCandidate(path.join(pkgDir, relative)))
}

/** An `exports` target names an exact file: no extension probing applies. */
function resolveExportTarget(pkgDir, target) {
  if (!target.startsWith('./')) return null
  const relative = target.slice('./'.length)
  if (!isSafePackageRelativePath(relative)) return null
  const candidate = path.join(pkgDir, relative)
  const resolved = existsSync(candidate) && !isDirectory(candidate) ? path.resolve(candidate) : null
  return containedInPackage(pkgDir, resolved)
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

/**
 * Probe `base` for a file: the TypeScript source a written extension stands
 * for, then the exact path, then an appended extension, then a directory index.
 *
 * Replays the `fileProbe` section of
 * `tests/fixtures/module-resolution-conformance.json`; the Rust half is
 * `resolve_file_candidate` in `crates/ruvyxa_bundler/src/resolver.rs`.
 */
export function probeFileCandidate(base) {
  const written = path.extname(base)
  const withoutExtension = written ? base.slice(0, base.length - written.length) : base

  for (const extension of TYPESCRIPT_SOURCE_EXTENSIONS[written] ?? []) {
    const candidate = `${withoutExtension}.${extension}`
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  if (existsSync(base) && !isDirectory(base)) return path.resolve(base)
  for (const extension of FILE_PROBE_EXTENSIONS) {
    const candidate = `${base}.${extension}`
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  for (const extension of FILE_PROBE_EXTENSIONS) {
    const candidate = path.join(base, `index.${extension}`)
    if (existsSync(candidate) && !isDirectory(candidate)) return path.resolve(candidate)
  }
  return null
}

function resolveFile(base) {
  return probeFileCandidate(base)
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

/**
 * Whether a file is project *source*, the question `is_project_local` in
 * `crates/ruvyxa_bundler/src/resolver.rs` answers: under the root, and not
 * under `node_modules`.
 *
 * The exclusion is the half this graph was missing. It decides two things at
 * once — which modules a bundle walks, and which files count as application
 * inputs — and without it a browser bundle, which inlines its packages because
 * a browser has no resolver, reported every file of every dependency as project
 * input: thousands of `node_modules` paths handed to the dev-server watcher,
 * and megabytes of dependency source re-hashed into `dependencyHash` on every
 * rebuild. Nothing that must invalidate rests on it: `PROJECT_MANIFEST_FILES`
 * still feeds the fingerprint, and bundle reuse is keyed on `readFiles`, which
 * is deliberately wider and still carries the dependency.
 */
function isProjectLocal(root, file) {
  const relative = path.relative(root, file)
  return Boolean(
    relative &&
    !relative.startsWith('..') &&
    !path.isAbsolute(relative) &&
    !startsWithNodeModules(relative),
  )
}

/**
 * Whether a path is under the project root at all, `node_modules` included.
 *
 * Not the same question as [`isProjectLocal`], and the two must stay apart:
 * this one bounds the upward walk in `findMdxComponentsFile`, whose Rust half
 * `resolve_mdx_components_file_with_root` bounds it with a plain
 * `starts_with(root)` and no exclusion. It also has to answer true for the root
 * itself, which is a directory rather than source and so is not project-local.
 */
function isWithinProject(root, file) {
  const relative = path.relative(root, file)
  return !relative.startsWith('..') && !path.isAbsolute(relative)
}

/**
 * Whether a project-relative path's first segment is `node_modules`.
 *
 * `Path::starts_with` in Rust compares whole components, so only the first
 * segment counts — `app/node_modules_notes.ts` is project source and so is
 * `app/node_modules/x` under a project that keeps a directory by that name
 * below the root. `path.relative` returns `\`-separated paths on Windows, so
 * the split has to accept both separators; testing for a `node_modules/`
 * prefix would have excluded nothing on the host where most of this is written.
 */
function startsWithNodeModules(relative) {
  return relative.split(/[\\/]/)[0] === 'node_modules'
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
/** Path separator on either host: a forward slash, or a Windows backslash. */
const SEPARATOR = /[\\/]+/
/** A `C:` style drive prefix, which no import spells. */
const DRIVE_PREFIX = /^[A-Za-z]:$/
/** Platforms whose filesystem folds case by default, and only where it can fire. */
const CASE_FOLDING_PLATFORM = process.platform === 'win32' || process.platform === 'darwin'

/// Segments of `p`, with `.` dropped and `..` applied lexically.
///
/// Lexical rather than through the filesystem: the point is to compare what an
/// import *asked for* against what the filesystem *answered*, so the request
/// must not be run through the filesystem first. Mirrors `lexical_segments` in
/// `crates/ruvyxa_bundler/src/resolver.rs`.
function lexicalSegments(p) {
  const segments = []
  for (const raw of String(p).split(SEPARATOR)) {
    // A leading empty segment is the root and a `C:` segment is a drive
    // prefix; neither carries a spelling an import chose.
    if (raw === '' || raw === '.' || DRIVE_PREFIX.test(raw)) continue
    if (raw === '..') {
      segments.pop()
      continue
    }
    segments.push(raw)
  }
  return segments
}

/// Fold one ASCII letter to lower case, leaving every other code unit alone.
///
/// ASCII-only on purpose: case outside ASCII is decided by the host's locale
/// tables, the reason `localeCompare` and `toLocaleLowerCase` are banned here.
/// `toLowerCase()` would fold `U+00DC` where the Rust half does not, and the
/// two graphs would answer differently on the same file.
function foldAscii(code) {
  return code >= 0x41 && code <= 0x5a ? code + 0x20 : code
}

function equalIgnoringAsciiCase(left, right) {
  if (left.length !== right.length) return false
  for (let index = 0; index < left.length; index += 1) {
    if (foldAscii(left.charCodeAt(index)) !== foldAscii(right.charCodeAt(index))) return false
  }
  return true
}

/**
 * The first segment an import spelled in a case the filesystem does not hold.
 *
 * `existsSync` answers case-insensitively on Windows and on default macOS, so
 * `import './Header'` resolves `header.tsx` and the project builds. On Linux the
 * same import resolves nothing, so the failure is invisible on the machine that
 * writes it and arrives in CI or on the deployed host.
 *
 * Replays `tests/fixtures/import-case-conformance.json`; the Rust half is
 * `import_case_mismatch` in `crates/ruvyxa_bundler/src/resolver.rs`.
 *
 * @returns {{requested: string, resolved: string}|null}
 */
export function importCaseMismatch(requested, resolved) {
  const requestedSegments = lexicalSegments(requested)
  const resolvedSegments = lexicalSegments(resolved)
  if (requestedSegments.length === 0 || resolvedSegments.length < requestedSegments.length) {
    return null
  }

  const last = requestedSegments.length - 1
  for (const [index, requestedSegment] of requestedSegments.entries()) {
    const resolvedSegment = resolvedSegments[index]
    // The resolver appends an extension, so the last requested segment can be
    // a prefix of what is on disk. Compare only the characters the import
    // actually spelled; every other segment is a directory name and must
    // match whole.
    const comparable =
      index === last && resolvedSegment.length > requestedSegment.length
        ? resolvedSegment.slice(0, requestedSegment.length)
        : resolvedSegment

    if (comparable !== requestedSegment && equalIgnoringAsciiCase(requestedSegment, comparable)) {
      return { requested: requestedSegment, resolved: comparable }
    }
  }
  return null
}

/**
 * Refuse a relative import whose spelling does not match the file on disk.
 *
 * Only relative specifiers, matching the Rust graph: `baseDir` is itself a
 * resolved path, so a segment differing only in case came from this specifier
 * and nothing above it. Package specifiers stay out of scope because pnpm
 * reaches a package through a symlink farm, where the real path differs from
 * the request for reasons that have nothing to do with spelling.
 *
 * Skipped where the filesystem is case-sensitive. That is not an optimization
 * standing in for the check: there, a mis-spelled import resolves nothing in
 * the first place, so there is never anything to report — and it keeps the
 * realpath syscall off the Linux builds that do the deploying.
 */
function assertImportCaseMatches(resolved, specifier, baseDir, importer) {
  if (!CASE_FOLDING_PLATFORM || !specifier.startsWith('.')) return

  let onDisk
  try {
    onDisk = realpathSync.native(resolved)
  } catch {
    // Unreadable here means the read that follows reports the real problem.
    return
  }

  const mismatch = importCaseMismatch(path.resolve(baseDir, specifier), onDisk)
  if (!mismatch) return
  throw new Error(
    `RUV1807 import '${specifier}' from ${importer} asks for '${mismatch.requested}', but the ` +
      `file on disk is named '${mismatch.resolved}'. This filesystem matches names ` +
      `case-insensitively, so the import resolves here and resolves nothing on a case-sensitive ` +
      `one — Linux CI, or the host the build is deployed to. Spell the import the way the file ` +
      `is named.`,
  )
}

export function assertSupportedModuleKind(resolved, specifier, importer) {
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
  // Node hands a default import the whole document, and the linker reads the
  // module's `default` property when it has one — so attach a self-reference
  // to make a default import the whole document. Never when the document has
  // its own `default` key: overwriting it would change data the application
  // can read through `require()`.
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

async function readSourceFile(file, transform = null) {
  const stats = statSync(file)
  // The plugin identity is part of the key, not merely consulted: the same file
  // at the same mtime compiles to different text once a plugin rewrites it, and
  // a cache that answered from the path alone would serve the untransformed
  // source for the rest of the process.
  const cacheKey = transform ? `${transform.identity}\0${path.resolve(file)}` : path.resolve(file)
  const cached = compilerCache.sources.get(cacheKey)
  if (cached && cached.mtimeMs === stats.mtimeMs && cached.size === stats.size) {
    return cached.source
  }
  let source = await readFile(file, 'utf8')
  if (transform) source = await transform.apply(source, file)
  setBoundedCacheEntry(compilerCache.sources, cacheKey, {
    mtimeMs: stats.mtimeMs,
    size: stats.size,
    source,
  })
  return source
}

/**
 * The `environment` a build hook is told it is transforming for.
 *
 * The same five values the Rust bundler reports, derived from the same two
 * facts — where the code will run, and which React graph it belongs to — so a
 * plugin sees one vocabulary whichever compiler called it. Before both
 * compilers ran hooks, `'client'` was the only value that ever occurred, and a
 * plugin that guarded on `environment` was guarding on a constant.
 *
 * @param {'browser'|'node'|'neutral'|string} platform
 * @param {string} bundleTarget
 */
function pluginEnvironmentFor(platform, bundleTarget) {
  if (bundleTarget === 'edge') return 'edge'
  // The server-components graph runs on the server; `react-server` is a
  // resolution condition, not a different host.
  if (bundleTarget === 'react-server') return 'server'
  return platform === 'browser' ? 'client' : 'server'
}

/**
 * The project's `build.onTransform` hooks, ready to run against a module.
 *
 * Returns `null` when the project has no build plugins, which is the common
 * case and costs one cached pointer read.
 *
 * Why this exists at all: a plugin transform used to be applied by the Rust
 * bundler and by nothing else. The browser bundle was rewritten; every server
 * render — `dev`, `start`, pre-rendering, and each deployed function — read the
 * same file through this compiler, which never called a hook. A rewritten value
 * that reached markup therefore made the two documents disagree, React threw
 * away the server tree and re-rendered (#418), and the only symptom was a
 * flicker. Both graphs now run the same hooks over the same source.
 *
 * Loaded from the pointer module the CLI writes for its config, so no project
 * config is compiled here and there is no way for this to recurse into itself:
 * a project whose config has not been rendered yet simply has no plugins to
 * run. `identity` is the pointer's own fingerprint, which changes whenever the
 * config or any plugin behind it does — that is what makes it safe as a cache
 * key.
 *
 * @param {string} projectRoot
 * @param {'client'|'server'|'edge'|'worker'|'shared'} environment
 */
async function projectBuildTransform(projectRoot, environment) {
  const root = path.resolve(projectRoot)
  const pointer = path.join(root, '.ruvyxa', 'cache', 'config', 'runtime-config.mjs')
  let pointerSource
  try {
    pointerSource = await readFile(pointer, 'utf8')
  } catch {
    return null
  }

  const fingerprint = createHash('sha256').update(pointerSource).digest('hex').slice(0, 32)
  const cacheKey = `${root}\0${environment}`
  const cached = compilerCache.pluginTransforms.get(cacheKey)
  if (cached?.fingerprint === fingerprint) return cached.transform

  let plugins = []
  try {
    const url = `${pathToFileURL(pointer).href}?v=${fingerprint}`
    const loaded = await import(url)
    plugins = Array.isArray(loaded.plugins) ? loaded.plugins : []
  } catch {
    // A pointer written by an older CLI has no `plugins` export, and a config
    // that cannot be imported is already reported by the host that renders it.
    plugins = []
  }

  let transform = null
  if (plugins.length > 0) {
    const registry = await createPluginRegistry({ root, plugins, environment: 'production' })
    if (registry.buildTransform.length > 0) {
      const identity = `${fingerprint}:${environment}`
      transform = {
        identity,
        async apply(code, file) {
          const result = await dispatchBuildTransform(registry, {
            code,
            id: file,
            environment,
          })
          return result ? result.code : code
        },
      }
    }
  }
  setBoundedCacheEntry(compilerCache.pluginTransforms, cacheKey, { fingerprint, transform })
  return transform
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

/**
 * A declaration export, read from just past the `export` keyword.
 *
 * `export function * gen` puts the star between the keyword and the name, so it
 * cannot be folded into the keyword alternatives. Mirrors the prefix list in
 * `ast::has_named_runtime_export`.
 */
const DECLARED_EXPORT_NAME =
  /^\s+(?:async\s+)?(?:function\s*\*\s*|function\s+|class\s+|const\s+|let\s+|var\s+)([\p{ID_Start}$_][\p{ID_Continue}$]*)/u

/**
 * An `export { … }` clause, read from just past the `export` keyword.
 *
 * `export type { … }` is erased at compile time and leaves no runtime binding,
 * so the `type` keyword deliberately fails this match. `}` inside a specifier's
 * string name is blanked by the mask, so the body cannot end early.
 */
const EXPORT_CLAUSE = /^\s*\{([^}]*)\}/u

/** Identifier runs inside an export clause. */
const CLAUSE_WORDS = /[\p{ID_Start}$_][\p{ID_Continue}$]*/gu

/**
 * Whether `source` already publishes a runtime export named `name`.
 *
 * The content compiler appends `frontmatter`, `meta`, `headings` and
 * `contentFormat` only when the page did not export them itself, so this
 * decides whether a generated declaration would collide with the author's.
 *
 * It runs over `findInCode`, which is the shared scanner. The private tokenizer
 * this replaced — and its character-for-character twin in
 * `crates/ruvyxa_bundler/src/content.rs` — knew line comments, block comments
 * and the three quote characters but had no regular-expression branch, so a
 * page whose export block wrote `/['"]/` above its own `export const
 * frontmatter` opened a string skip that ran to the next quote anywhere in the
 * file. Everything past the regex stopped being seen as code, the generated
 * export was appended beside the author's, and the module failed to parse with
 * a declaration nobody wrote. The reverse was reachable too: a desync that made
 * an unrelated `export` visible dropped the real frontmatter and headings
 * silently.
 *
 * Both the declaration form and the clause form count. `ast::
 * has_named_runtime_export` ignores re-export forms on purpose, because a route
 * capability must not be advertised across an unproven graph edge; that is not
 * this question. An appended `export const NAME` collides with
 * `export { x as NAME } from './y'` exactly as hard as with a local one, which
 * is why `ast::has_named_clause_export` sits beside it on the Rust side.
 *
 * The shared cases are in `tests/fixtures/content-conformance.json`.
 */
function hasNamedExport(source, name) {
  const masked = maskNonCode(source)
  for (const offset of findInCode(source, 'export')) {
    if (!isIdentifierBoundary(source, offset, 'export'.length)) continue
    const after = offset + 'export'.length
    // The declaration form reads the raw source, because its Rust counterpart
    // reads the text after the keyword with `trim_start` — whitespace only, so
    // a comment between the keyword and the declaration is not skipped there
    // and must not be skipped here. The clause form reads the mask, because
    // `named_clause_exports` skips comments and string specifier names the same
    // way the mask blanks them.
    if (DECLARED_EXPORT_NAME.exec(source.slice(after))?.[1] === name) return true
    const clause = EXPORT_CLAUSE.exec(masked.slice(after))
    if (clause && exportClauseBinds(clause[1], name)) return true
  }
  return false
}

/** Whether neither neighbour of `source[start, start + length)` continues an identifier. */
function isIdentifierBoundary(source, start, length) {
  return !(
    (start > 0 && IDENTIFIER_CHARACTER.test(source[start - 1])) ||
    (start + length < source.length && IDENTIFIER_CHARACTER.test(source[start + length]))
  )
}

const IDENTIFIER_CHARACTER = /[\p{ID_Continue}$]/u

/**
 * Whether an `export { ... }` clause publishes `name`.
 *
 * `a as b` publishes `b`, so the name after `as` is the one that counts. A
 * leading `type` is dropped rather than treated as an identifier: `export {
 * type Foo, bar }` publishes `Foo` and `bar`, and counting `type` would shift
 * every specifier in the list by one. Mirrors `named_clause_exports` in
 * `crates/ruvyxa_bundler/src/ast.rs`.
 */
function exportClauseBinds(clause, name) {
  for (const specifier of clause.split(',')) {
    const words = specifier.match(CLAUSE_WORDS) ?? []
    const named = words[0] === 'type' && words.length > 1 ? words.slice(1) : words
    const asIndex = named.indexOf('as')
    if ((asIndex === -1 ? named[0] : named[asIndex + 1]) === name) return true
  }
  return false
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

/**
 * Which oxc parser dialect an extension asks for.
 *
 * TypeScript settles this by extension rather than by contents, because the two
 * readings are mutually exclusive: in a `.ts` file `<T>(x)` is a generic
 * parameter list and `<string>v` a type assertion, while the same bytes in a
 * `.tsx` file open an element. An extension not listed here is the `.js` family,
 * where nothing in the name decides and the source is asked instead — JSX in a
 * `.js` file is common in the ecosystem, and the Rust graph already accepted it,
 * so refusing it here compiled a package for the browser and then failed the
 * same package at prerender.
 *
 * Held level with `crates/ruvyxa_bundler/src/compiler.rs` by the `parserDialect`
 * section of `tests/fixtures/module-kind-conformance.json`.
 */
const TRANSFORM_LANG_BY_EXTENSION = {
  '.tsx': 'tsx',
  '.jsx': 'jsx',
  '.ts': 'ts',
  '.mts': 'ts',
  '.cts': 'ts',
}

/**
 * The oxc dialect for one module, from its extension and — only when the
 * extension does not decide — its source.
 *
 * Replayed against the Rust graph by the `parserDialect` section of
 * `tests/fixtures/module-kind-conformance.json`.
 */
export function transformLangFor(extension, source) {
  const named = TRANSFORM_LANG_BY_EXTENSION[String(extension).toLowerCase()]
  if (named) return named
  return containsJsx(source) ? 'jsx' : 'js'
}

/**
 * Blank a leading `#!` line, keeping the line itself.
 *
 * A shebang is legal at the top of a module and Node removes it when it loads
 * one directly — but a bundled module is wrapped in a function, where `#!` is a
 * syntax error. Packages that double as executables ship one on their entry
 * file, so importing such a package failed the build with a parse error that
 * named a character rather than a cause.
 *
 * The line is emptied rather than deleted so every line below keeps its number.
 */
function stripShebang(source) {
  const start = source.charCodeAt(0) === 0xfeff ? 1 : 0
  if (source.charCodeAt(start) !== 0x23 || source.charCodeAt(start + 1) !== 0x21) return source
  const lineEnd = source.indexOf('\n', start)
  const prefix = source.slice(0, start)
  return lineEnd === -1 ? prefix : prefix + source.slice(lineEnd)
}

/**
 * Remove decorators, keeping every line number.
 *
 * Ruvyxa accepts decorators and strips them — the emitted bundle is plain
 * JavaScript, which has no such syntax. The Rust compiler has always done this
 * (`strip_decorators_with_plan`); this graph did not, so a decorated class
 * compiled for the browser and threw `Invalid or unexpected token` on every
 * server render of the same route. One rule, two compilers, and only one of
 * them was applying it.
 *
 * Blank lines replace the ones a decorator occupied, so diagnostics and source
 * maps still address the line the author wrote.
 */
function stripDecorators(source) {
  if (!source.includes('@')) return source
  const masked = maskNonCode(source)
  let out = ''
  let index = 0
  while (index < source.length) {
    if (masked[index] !== '@' || !decoratorCanStart(masked, index)) {
      out += source[index]
      index += 1
      continue
    }
    const end = skipDecorator(masked, index)
    // Only the newlines it spanned, so nothing below it moves.
    out += masked.slice(index, end).replace(/[^\n]/g, '')
    index = end
  }
  return out
}

/** Whether an `@` begins a decorator rather than sitting inside a larger token. */
function decoratorCanStart(masked, at) {
  for (let index = at - 1; index >= 0; index -= 1) {
    const character = masked[index]
    if (/\s/.test(character)) continue
    return /[{};)\]\w$]/.test(character)
  }
  // Nothing before it in the file: a decorator on the first declaration.
  return true
}

/** The index just past a decorator that begins at `at`. */
function skipDecorator(masked, at) {
  let index = at + 1
  while (index < masked.length && /[\w$.]/.test(masked[index])) index += 1
  if (masked[index] !== '(') return index
  let depth = 1
  index += 1
  while (index < masked.length && depth > 0) {
    if (masked[index] === '(') depth += 1
    else if (masked[index] === ')') depth -= 1
    index += 1
  }
  return index
}

function transformModuleSource(module) {
  // Resolve lazily so tools that copy compiler.mjs for path-isolation checks do
  // not need the package dependency beside the copied file until compilation.
  const filename = String(module.filePath || module.key || 'ruvyxa:module.ts')
  const extension = path.extname(filename).toLowerCase()
  const lang = transformLangFor(extension, module.source)
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
  const source = stripShebang(stripDecorators(reactCompiled?.code ?? module.source))
  const result = transformSync(filename, source, {
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
    const named = [...new Set(helpers)].sort(compareCodePoints).join(', ')
    throw new Error(
      `RUV1802 build.target \`${esTarget}\` needs the runtime helpers ${named} for ${filename}, and Ruvyxa ships no helper runtime — raise build.target (ordinary application code compiles helper-free at es2022 and above) or remove the syntax that needs downlevelling`,
    )
  }
  const code = substitutePublicEnv(result.code)
  module.transformLineMap = composeLineMaps(result.map, reactCompiled?.rawMap)
  setBoundedCacheEntry(compilerCache.transforms, transformKey, {
    code,
    lineMap: module.transformLineMap,
  })
  return code
}

/**
 * Replace `import.meta.env` with the public environment, as a literal.
 *
 * Documented in the configuration chapter and implemented by nothing: the
 * expression was left as written, `import.meta` carries no `env` in Node or in
 * a browser, and the documented client component threw
 * `Cannot read properties of undefined` during the server render of its own
 * page. Substituting at compile time is what makes the value the same in both
 * graphs and keeps the boundary intact — only `RUVYXA_PUBLIC_*` names are in
 * the object, so this can never widen what a browser bundle may read.
 *
 * The replacement is length-preserving-agnostic: line structure is kept because
 * the literal contains no newline, so the transform's line map still describes
 * the file.
 */
function substitutePublicEnv(code) {
  const marker = 'import.meta.env'
  const positions = findInCode(code, marker)
  if (positions.length === 0) return code

  const literal = `Object.freeze(${JSON.stringify(publicEnvValues())})`
  let out = ''
  let cursor = 0
  for (const at of positions) {
    out += code.slice(cursor, at) + literal
    cursor = at + marker.length
  }
  return out + code.slice(cursor)
}

/** Every `RUVYXA_PUBLIC_*` value this process can see, in name order. */
function publicEnvValues() {
  const values = {}
  for (const name of Object.keys(process.env).sort(compareCodePoints)) {
    if (name.startsWith('RUVYXA_PUBLIC_')) values[name] = process.env[name] ?? ''
  }
  return values
}

/**
 * Refuse a linked bundle that does not parse, before anything can run it.
 *
 * Any pass that rewrites or deletes code needs its output parsed by a test —
 * and by the build. This linker rewrites ESM one statement at a time, and when
 * it got a statement wrong (a clause list wrapped across lines, `chalk`'s
 * shape) the bundle it wrote was accepted, the build reported success, and the
 * `SyntaxError` arrived when a deployed server tried to import it. The check is
 * one parse of a file this compile just produced, at the moment it is cheapest
 * to explain.
 */
function assertLinkedSyntax(code, outfile, lineOrigins = []) {
  const { transformSync } = createRequire(
    path.join(path.dirname(fileURLToPath(import.meta.url)), '__ruvyxa-transform.cjs'),
  )('oxc-transform')
  const parsed = transformSync('ruvyxa-linked.mjs', code, { lang: 'js', sourceType: 'module' })
  if (parsed.errors.length === 0) return

  const where = describeSyntaxError(code, parsed.errors[0], lineOrigins)
  // A top-level `await` is the one shape a caller cannot read out of the
  // parser's own words: every module is wrapped in an ordinary function, so it
  // is a syntax error only after linking, in a file the author never wrote.
  const hint = /await/.test(where)
    ? '\n\nA module in this graph awaits at its top level. A linked bundle wraps each module in an ordinary function, so a top-level `await` cannot survive it — move the await inside an async function, or into a server module the browser graph does not reach.'
    : ''
  throw new Error(
    `RUV1804 the linked bundle for ${path.basename(outfile)} does not parse: ${where}${hint}`,
  )
}

/** One oxc diagnostic, named against the module the failing line came from. */
function describeSyntaxError(code, error, lineOrigins) {
  const message = error.message ?? String(error)
  const offset = error.labels?.[0]?.start
  if (typeof offset !== 'number') return message
  const generatedLine = code.slice(0, offset).split('\n').length - 1
  const text = code.split('\n')[generatedLine]?.trim() ?? ''
  const origin = lineOrigins?.[generatedLine]
  const where = origin ? `${origin.source}:${origin.line + 1}` : 'code the linker wrote itself'
  return `${message} (from ${where}: ${text.slice(0, 120)})`
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
 *
 * Exported for the `extraction` section of
 * `tests/fixtures/env-policy-conformance.json`, which the Rust half replays
 * through `private_env_reads`. Classification alone was fixture-held and
 * extraction was not, and extraction is where the two graphs had drifted.
 *
 * @param {string} source Module source, before transformation.
 * @returns {string[]} Private names, in source order.
 * @public
 */
export function privateEnvReads(source) {
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
    const match = ENV_NAME_PATTERN.exec(source.slice(index))
    if (!match) return null
    return { name: match[0], end: index + match[0].length }
  }
  if (source[index] !== '[') return null
  index += 1
  while (/\s/.test(source[index] ?? '')) index += 1
  const quote = source[index]
  if (quote !== '"' && quote !== "'") return null
  index += 1
  const match = ENV_NAME_PATTERN.exec(source.slice(index))
  if (!match) return null
  index += match[0].length
  if (source[index] !== quote) return null
  index += 1
  while (/\s/.test(source[index] ?? '')) index += 1
  return source[index] === ']' ? { name: match[0], end: index + 1 } : null
}

function preferExisting(...files) {
  return files.find((file) => existsSync(file)) ?? files[0]
}
