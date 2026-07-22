import { createHash } from 'node:crypto'
import { existsSync, statSync } from 'node:fs'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { createRequire, isBuiltin } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const JS_EXTENSIONS = ['', '.ts', '.tsx', '.js', '.jsx', '.mts', '.mjs', '.md', '.mdx']
const ASSET_EXTENSIONS = new Set(['.css', '.scss', '.sass', '.less'])
const COMPILER_CACHE_MAX_ENTRIES = 512
const compilerCache = (globalThis.__RUVYXA_COMPILER_CACHE__ ??= {
  sources: new Map(),
  transforms: new Map(),
  rewrites: new Map(),
  content: new Map(),
})
compilerCache.transforms ??= new Map()

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
}

export function compilerCacheStats() {
  return {
    sources: compilerCache.sources.size,
    transforms: compilerCache.transforms.size,
    rewrites: compilerCache.rewrites.size,
    content: compilerCache.content.size,
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

export async function compileBundle(options) {
  return (await compileBundleWithMetadata(options)).outfile
}

/** Return the active server runtime without changing browser bundle semantics. */
export function serverPlatform() {
  return process.env.RUVYXA_RUNTIME === 'bun' ? 'bun' : 'node'
}

/** Compile a bundle and return a stable fingerprint of its project-local inputs. */
export async function compileBundleWithMetadata({
  projectRoot,
  entrySource,
  sourcefile = 'ruvyxa:entry.ts',
  outfile,
  platform = 'node',
  bundlePackages = false,
  bundleAliasDependencies = false,
  external = [],
  aliases = {},
  minify = false,
  sourceMap = true,
  jsxRuntime = process.env.RUVYXA_JSX_RUNTIME ?? 'automatic',
}) {
  const normalizedJsxRuntime = normalizeJsxRuntime(jsxRuntime)
  const root = path.resolve(projectRoot)
  const modules = []
  const byKey = new Map()
  const externals = new Map()
  const externalSet = new Set(external)
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
    aliases,
    platform,
    bundlePackages,
    bundleAliasDependencies,
    bundleDependencies: false,
    jsxRuntime: normalizedJsxRuntime,
  })

  const linked = linkModules(modules, externals, { minify, outfile, sourceMap })
  await mkdir(path.dirname(outfile), { recursive: true })
  await writeIfChanged(outfile, linked.code)
  if (linked.map) {
    await writeIfChanged(`${outfile}.map`, JSON.stringify(linked.map))
  }
  return {
    outfile,
    dependencyHash: await fingerprintProjectInputs(root, modules),
    inputs: projectInputPaths(root, modules),
  }
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

function projectInputPaths(root, modules) {
  return [
    ...new Set(
      modules.flatMap((module) =>
        [module.filePath, ...(module.assetInputs || [])]
          .filter((file) => file && isWithinProject(root, file))
          .map((file) => path.relative(root, file).replaceAll('\\', '/')),
      ),
    ),
  ].sort()
}

async function fingerprintProjectInputs(root, modules) {
  const hash = createHash('sha256')
  const projectModules = modules
    .filter((module) => module.filePath && isWithinProject(root, module.filePath))
    .map((module) => ({
      path: path.relative(root, module.filePath).replaceAll('\\', '/'),
      source: module.source,
    }))
    .sort((left, right) => left.path.localeCompare(right.path))

  for (const module of projectModules) {
    hash.update(module.path)
    hash.update('\0')
    hash.update(module.source)
    hash.update('\0')
  }

  for (const fileName of [
    'package.json',
    'pnpm-lock.yaml',
    'package-lock.json',
    'yarn.lock',
    'bun.lock',
    'bun.lockb',
  ]) {
    const file = path.join(root, fileName)
    if (!existsSync(file)) continue
    hash.update(fileName)
    hash.update('\0')
    hash.update(await readFile(file))
    hash.update('\0')
  }

  return hash.digest('hex')
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
  }
}

async function visitModule(context) {
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
    aliases,
    platform,
    bundlePackages,
    bundleAliasDependencies,
    bundleDependencies,
    jsxRuntime,
  } = context

  if (byKey.has(key)) return byKey.get(key)

  const styleModule = isCssModuleFile(filePath)
    ? await compileStyleModuleSource(source, filePath, root)
    : null
  const compiledSource = styleModule?.source ?? (await compileContentSource(source, filePath))
  const id = `__m${modules.length}`
  const module = {
    id,
    key,
    filePath,
    source: compiledSource,
    baseDir,
    deps: new Map(),
    assetInputs: styleModule?.inputs ?? [],
    jsxRuntime,
  }
  byKey.set(key, module)
  modules.push(module)

  if (platform === 'browser') {
    checkClientBoundary(root, filePath, compiledSource)
  }

  // Inspect the transformed module so automatic JSX helper imports are linked
  // like ordinary dependencies. Oxc adds `react/jsx-runtime` during transform;
  // scanning only the source would otherwise drop those bindings in wrapped
  // Node bundles and leave `_jsx` undefined at render time.
  const transformedSource = transformModuleSource(module)
  module.transformedSource = transformedSource
  for (const specifier of extractSpecifiers(transformedSource)) {
    if (isAssetSpecifier(specifier) && !isCssModuleSpecifier(specifier)) continue

    const resolvedAlias = aliases[specifier]
    const resolved = resolvedAlias
      ? resolveFile(path.resolve(resolvedAlias))
      : (resolveLocalSpecifier(baseDir, specifier) ??
        (platform === 'browser' || bundlePackages || bundleDependencies
          ? resolvePackage(baseDir, specifier)
          : null))

    if (
      resolved &&
      (resolvedAlias ||
        isProjectLocal(root, resolved) ||
        platform === 'browser' ||
        bundlePackages ||
        bundleDependencies)
    ) {
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
        aliases,
        platform,
        bundlePackages,
        bundleAliasDependencies,
        bundleDependencies:
          bundleDependencies || (bundleAliasDependencies && Boolean(resolvedAlias)),
        jsxRuntime,
      })
      module.deps.set(specifier, dep)
      continue
    }

    const externalSpecifier = resolvedAlias ? toImportPath(resolvedAlias) : specifier
    if (!externals.has(externalSpecifier)) {
      externals.set(externalSpecifier, `__ext${externals.size}`)
    }
    module.deps.set(specifier, {
      external: true,
      specifier: externalSpecifier,
      alias: externals.get(externalSpecifier),
    })

    if (!externalSet.has(specifier) && specifier.startsWith('.')) {
      throw new Error(`RUV1801 cannot resolve '${specifier}' from ${filePath || sourcefile}`)
    }
  }

  return module
}

function linkModules(modules, externals, { minify, outfile, sourceMap }) {
  const out = []
  const lineMappings = []
  const mapSources = new Map()
  const push = (line, mapping = null) => {
    out.push(line)
    lineMappings.push(mapping)
  }

  for (const [specifier, alias] of externals) {
    push(`import * as ${alias} from ${JSON.stringify(specifier)};`)
  }
  const reactAlias = externals.get('react')
  if (reactAlias) push(`const React = ${reactAlias}.default ?? ${reactAlias};`)
  if (externals.size > 0) push('')

  const rewrittenModules = new Map(modules.map((module) => [module.id, rewriteModule(module)]))

  for (const module of orderModulesByDependencies(modules)) {
    const rewritten = rewrittenModules.get(module.id)
    const sourceIndex =
      sourceMap && module.filePath
        ? sourceMapIndex(mapSources, module.filePath, module.source)
        : null

    push(`const ${module.id} = (() => {`)
    push(`  const __exports = {};`)
    push(`  const module = { exports: __exports };`)
    push(`  const exports = module.exports;`)
    push(`  const process = globalThis.process ?? { env: { NODE_ENV: 'production' } };`)
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
  const normalized = toImportPath(filePath)
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
      lineMap.push(sourceLine)
      continue
    }

    if (/^import\b/.test(line)) {
      const rewritten = rewriteImport(rawLine.trim(), module)
      if (rewritten) {
        lines.push(rewritten)
        lineMap.push(sourceLine)
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
      lineMap.push(sourceLine)
      sourceLine = endLine
      continue
    }

    if (/^export\b/.test(line)) {
      const result = rewriteExport(rawLine.trim(), module, exported, reExportAll)
      if (result) {
        lines.push(result)
        lineMap.push(sourceLine)
      }
      continue
    }

    lines.push(rewriteCommonJsRequires(rewriteDynamicImports(rawLine, module), module))
    lineMap.push(sourceLine)
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

  if (/^export\s+(async\s+)?function\s+/.test(line)) {
    const name = line.match(/^export\s+(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/)?.[1]
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

function extractSpecifiers(source) {
  const codeOnly = maskNonCode(source, {
    preserveImportExportSpecifiers: true,
    preserveImportCallSpecifiers: true,
    preserveRequireCallSpecifiers: true,
  })
  const specifiers = []
  const patterns = [
    /\bimport\s+(?:type\s+)?[\s\S]*?\s+from\s+["']([^"']+)["']/g,
    /\bexport\s+[\s\S]*?\s+from\s+["']([^"']+)["']/g,
    /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g,
    /\brequire\s*\(\s*["']([^"']+)["']\s*\)/g,
    /^\s*import\s+["']([^"']+)["']/gm,
  ]
  for (const pattern of patterns) {
    for (const match of codeOnly.matchAll(pattern)) {
      if (isTypeOnlySpecifier(codeOnly, match.index ?? 0)) continue
      specifiers.push(match[1])
    }
  }
  return [...new Set(specifiers)]
}

function resolveLocalSpecifier(baseDir, specifier) {
  if (!specifier.startsWith('.') && !path.isAbsolute(specifier)) return null
  const base = path.isAbsolute(specifier) ? specifier : path.resolve(baseDir, specifier)
  return resolveFile(base)
}

function resolvePackage(baseDir, specifier) {
  if (isBuiltin(specifier)) return null
  try {
    return createRequire(path.join(baseDir, '__ruvyxa-resolve__.cjs')).resolve(specifier)
  } catch {
    return null
  }
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
  let quote = null
  let inComment = false
  let escaped = false

  while (index < chars.length) {
    const char = chars[index]
    const next = chars[index + 1]

    if (inComment) {
      output += char
      if (char === '*' && next === '/') {
        output += '/'
        index += 2
        inComment = false
      } else {
        index += 1
      }
      continue
    }

    if (quote) {
      output += char
      if (escaped) escaped = false
      else if (char === '\\') escaped = true
      else if (char === quote) quote = null
      index += 1
      continue
    }

    if (char === '/' && next === '*') {
      output += '/*'
      index += 2
      inComment = true
      continue
    }
    if (char === '"' || char === "'") {
      output += char
      prelude += char
      quote = char
      index += 1
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
      const scoped = scopedNames.get(local) ?? scopedClassName(file, root, local)
      scopedNames.set(local, scoped)
      if (!classes.has(local)) classes.set(local, scoped)
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
        const composed = composition.names.map((local) => {
          const scoped = scopedNames.get(local) ?? scopedClassName(file, root, local)
          scopedNames.set(local, scoped)
          if (!classes.has(local)) classes.set(local, scoped)
          return scoped
        })
        for (const owner of owners) {
          const ownerScoped = scopedNames.get(owner) ?? scopedClassName(file, root, owner)
          scopedNames.set(owner, ownerScoped)
          const exported = (classes.get(owner) ?? ownerScoped).split(/\s+/)
          for (const scoped of composed) if (!exported.includes(scoped)) exported.push(scoped)
          classes.set(owner, exported.join(' '))
        }
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

async function compileContentSource(source, filePath) {
  const extension = filePath ? path.extname(filePath).toLowerCase() : ''
  if (extension !== '.md' && extension !== '.mdx') return source

  const cacheKey = createHash('sha256').update(extension).update('\0').update(source).digest('hex')
  const cached = compilerCache.content.get(cacheKey)
  if (cached) return cached

  const { frontmatterSource, body } = splitContentFrontmatter(source)
  const [frontmatter, { compile }, { default: remarkGfm }] = await Promise.all([
    parseContentFrontmatter(frontmatterSource, filePath),
    import('@mdx-js/mdx'),
    import('remark-gfm'),
  ])
  const headings = []
  let compiled
  try {
    compiled = String(
      await compile(body, {
        format: extension === '.md' ? 'md' : 'mdx',
        jsx: false,
        outputFormat: 'program',
        development: false,
        remarkPlugins: [remarkGfm, createContentMetadataPlugin(headings)],
      }),
    )
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new Error(`RUV1311 ${filePath}: ${detail}`)
  }

  const prefix = [
    contentExport(compiled, 'frontmatter', JSON.stringify(frontmatter)),
    contentExport(compiled, 'meta', 'frontmatter'),
    contentExport(compiled, 'headings', JSON.stringify(headings)),
    contentExport(compiled, 'contentFormat', JSON.stringify(extension.slice(1))),
  ]
    .filter(Boolean)
    .join('\n')
  const output = `${compiled}\n${prefix}\n`
  setBoundedCacheEntry(compilerCache.content, cacheKey, output)
  return output
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
    if (tokens[cursor] === '{') {
      cursor += 1
      let specifier = []
      while (cursor < tokens.length) {
        const token = tokens[cursor]
        if (token === ',' || token === '}') {
          const asIndex = specifier.indexOf('as')
          const exported = asIndex >= 0 ? specifier[asIndex + 1] : specifier[0]
          if (exported === name) return true
          specifier = []
          if (token === '}') break
        } else if (token !== 'type') {
          specifier.push(token)
        }
        cursor += 1
      }
    }
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

function createContentMetadataPlugin(headings) {
  return function contentMetadataPlugin() {
    return (tree) => {
      const slugCounts = new Map()
      collectContentHeadingNodes(tree, headings, slugCounts)
    }
  }
}

function collectContentHeadingNodes(node, headings, slugCounts) {
  if (node?.type === 'heading') {
    const text = contentPlainText(node.children ?? [])
    const baseSlug =
      text
        .toLocaleLowerCase()
        .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
        .replace(/^-|-$/g, '') || 'section'
    const occurrence = slugCounts.get(baseSlug) ?? 0
    const slug = occurrence === 0 ? baseSlug : `${baseSlug}-${occurrence}`
    slugCounts.set(baseSlug, occurrence + 1)
    headings.push({ depth: node.depth, slug, text })
    node.data = {
      ...node.data,
      hProperties: { ...node.data?.hProperties, id: slug },
    }
  }

  for (const child of node?.children ?? []) {
    collectContentHeadingNodes(child, headings, slugCounts)
  }
}

function contentPlainText(nodes) {
  return nodes
    .map((node) => {
      if (node.type === 'text' || node.type === 'inlineCode') return node.value
      return contentPlainText(node.children ?? [])
    })
    .join('')
}

async function writeIfChanged(file, contents) {
  try {
    if ((await readFile(file, 'utf8')) === contents) return
  } catch {
    // File does not exist yet.
  }
  await writeFile(file, contents)
}

function transformModuleSource(module) {
  // Resolve lazily so tools that copy compiler.mjs for path-isolation checks do
  // not need the package dependency beside the copied file until compilation.
  const filename = String(module.filePath || module.key || 'ruvyxa:module.ts')
  const extension = path.extname(filename).toLowerCase()
  const lang =
    extension === '.tsx'
      ? 'tsx'
      : extension === '.jsx'
        ? 'jsx'
        : extension === '.ts' || extension === '.mts' || extension === '.cts'
          ? 'ts'
          : 'js'
  const transformKey = createHash('sha256')
    .update(lang)
    .update('\0')
    .update(module.jsxRuntime)
    .update('\0')
    .update(module.source)
    .digest('hex')
  const cached = compilerCache.transforms.get(transformKey)
  if (cached) return cached
  const { transformSync } = createRequire(
    path.join(path.dirname(fileURLToPath(import.meta.url)), '__ruvyxa-transform.cjs'),
  )('oxc-transform')
  const result = transformSync(filename, module.source, {
    lang,
    sourceType: 'module',
    target: 'esnext',
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
  setBoundedCacheEntry(compilerCache.transforms, transformKey, result.code)
  return result.code
}

function normalizeJsxRuntime(value) {
  const runtime = String(value).toLowerCase()
  if (runtime === 'classic' || runtime === 'automatic') return runtime
  throw new Error(`RUV1804 JSX runtime must be \`classic\` or \`automatic\`, got \`${value}\``)
}

function checkClientBoundary(root, filePath, source) {
  if (!filePath) return
  const normalized = path.relative(root, filePath).replaceAll('\\', '/')
  if (
    normalized === 'server.ts' ||
    normalized.startsWith('server/') ||
    normalized.endsWith('/server.ts')
  ) {
    throw new Error(`RUV1007: Server-only file imported into client bundle: ${filePath}`)
  }
  if (extractSpecifiers(source).some(isServerOnlySpecifier)) {
    throw new Error(`RUV1007: Server-only module imported into client bundle: ${filePath}`)
  }
  for (const envName of privateEnvReads(source)) {
    throw new Error(
      `RUV1008: Private environment variable ${envName} used in client bundle: ${filePath}`,
    )
  }
}

function isServerOnlySpecifier(specifier) {
  return ['server-only', '@ruvyxa/auth', '@ruvyxa/database'].includes(specifier)
}

function privateEnvReads(source) {
  const names = []
  scanPrivateEnvReads(source, 0, 0, names)
  return names
}

// Keep the runtime scanner structurally aligned with the Rust boundary scanner.
// A template literal is not wholly non-code: `${...}` contains executable code
// and must still be checked for private environment reads.
function scanPrivateEnvReads(source, start, templateExpressionDepth, names) {
  let index = start
  // Index of the last byte that can end a token. A `/` only opens a regular
  // expression when no value precedes it. Without this, `/['"]/` reads as a
  // division followed by an unterminated string, and every later env read in
  // the module goes unreported.
  let previousSignificant = -1

  while (index < source.length) {
    const char = source[index]
    const next = source[index + 1]
    const tokenStart = index

    if (char === '"' || char === "'") {
      index = readStringEnd(source, index, char)
      previousSignificant = tokenStart
      continue
    }
    if (char === '`') {
      index = scanTemplateForPrivateEnv(source, index, names)
      previousSignificant = tokenStart
      continue
    }
    if (char === '/' && next === '/') {
      const end = source.indexOf('\n', index + 2)
      index = end === -1 ? source.length : end
      continue
    }
    if (char === '/' && next === '*') {
      const end = source.indexOf('*/', index + 2)
      index = end === -1 ? source.length : end + 2
      continue
    }
    if (char === '/' && regexCanStart(source, previousSignificant)) {
      index = readRegexEnd(source, index)
      previousSignificant = tokenStart
      continue
    }
    if (templateExpressionDepth > 0 && char === '{') {
      templateExpressionDepth += 1
      index += 1
      previousSignificant = tokenStart
      continue
    }
    if (templateExpressionDepth > 0 && char === '}') {
      templateExpressionDepth -= 1
      index += 1
      if (templateExpressionDepth === 0) return index
      previousSignificant = tokenStart
      continue
    }

    if (source.startsWith('process.env', index) && isEnvReadBoundary(source, index)) {
      const parsed = parsePrivateEnvName(source, index + 'process.env'.length)
      if (parsed && parsed.name !== 'NODE_ENV' && !parsed.name.startsWith('RUVYXA_PUBLIC_')) {
        names.push(parsed.name)
      }
      index = parsed?.end ?? index + 'process.env'.length
      previousSignificant = index - 1
      continue
    }
    if (!/\s/.test(char)) previousSignificant = tokenStart
    index += 1
  }
  return index
}

const REGEX_PRECEDING_KEYWORDS = new Set([
  'await',
  'case',
  'delete',
  'do',
  'else',
  'in',
  'instanceof',
  'new',
  'of',
  'return',
  'throw',
  'typeof',
  'void',
  'yield',
])

/** A regex may only start where a value is expected, never after one. */
function regexCanStart(source, previousSignificant) {
  if (previousSignificant < 0) return true
  const previous = source[previousSignificant]
  if (previous === ')' || previous === ']' || previous === '}') return false
  if (previous === '"' || previous === "'" || previous === '`') return false
  if (!isIdentifierChar(previous)) return true

  let wordStart = previousSignificant
  while (wordStart > 0 && isIdentifierChar(source[wordStart - 1])) wordStart -= 1
  return REGEX_PRECEDING_KEYWORDS.has(source.slice(wordStart, previousSignificant + 1))
}

function isIdentifierChar(char) {
  return char !== undefined && /[\w$]/.test(char)
}

/** Return the index just past a regular expression literal and its flags. */
function readRegexEnd(source, start) {
  let index = start + 1
  let insideCharacterClass = false

  while (index < source.length) {
    const char = source[index]
    if (char === '\\') {
      index = Math.min(index + 2, source.length)
      continue
    }
    // An unterminated literal was a division after all; resume normal scanning.
    if (char === '\n') return index
    if (char === '[') insideCharacterClass = true
    else if (char === ']' && insideCharacterClass) insideCharacterClass = false
    else if (char === '/' && !insideCharacterClass) {
      index += 1
      break
    }
    index += 1
  }

  while (index < source.length && isIdentifierChar(source[index])) index += 1
  return index
}

function scanTemplateForPrivateEnv(source, start, names) {
  let index = start + 1
  while (index < source.length) {
    const char = source[index]
    if (char === '\\') {
      index = Math.min(index + 2, source.length)
      continue
    }
    if (char === '`') return index + 1
    if (char === '$' && source[index + 1] === '{') {
      index = scanPrivateEnvReads(source, index + 2, 1, names)
      continue
    }
    index += 1
  }
  return index
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

function maskNonCode(source, options = {}) {
  const preserveImportExportSpecifiers = options.preserveImportExportSpecifiers === true
  const preserveImportCallSpecifiers = options.preserveImportCallSpecifiers === true
  const preserveRequireCallSpecifiers = options.preserveRequireCallSpecifiers === true
  let output = ''
  let index = 0

  while (index < source.length) {
    const char = source[index]
    const next = source[index + 1]

    if (char === '/' && next === '/') {
      const end = source.indexOf('\n', index + 2)
      const stop = end === -1 ? source.length : end
      output += ' '.repeat(stop - index)
      index = stop
      continue
    }

    if (char === '/' && next === '*') {
      const end = source.indexOf('*/', index + 2)
      const stop = end === -1 ? source.length : end + 2
      output += maskRange(source.slice(index, stop))
      index = stop
      continue
    }

    if (char === '"' || char === "'") {
      const end = readStringEnd(source, index, char)
      const literal = source.slice(index, end)
      const previous = source.slice(Math.max(0, index - 32), index)
      const preserve =
        (preserveImportExportSpecifiers && /\b(?:from|import)\s*$/.test(previous)) ||
        (preserveImportCallSpecifiers && /\bimport\s*\(\s*$/.test(previous)) ||
        (preserveRequireCallSpecifiers && /\brequire\s*\(\s*$/.test(previous))
      output += preserve ? literal : maskRange(literal)
      index = end
      continue
    }

    if (char === '`') {
      const end = readTemplateEnd(source, index)
      output += maskRange(source.slice(index, end))
      index = end
      continue
    }

    output += char
    index++
  }

  return output
}

function readStringEnd(source, start, quote) {
  let index = start + 1
  while (index < source.length) {
    const char = source[index++]
    if (char === '\\') {
      index++
      continue
    }
    if (char === quote) break
  }
  return index
}

function readTemplateEnd(source, start) {
  let index = start + 1
  while (index < source.length) {
    const char = source[index++]
    if (char === '\\') {
      index++
      continue
    }
    if (char === '`') break
  }
  return index
}

function maskRange(value) {
  return value.replace(/[^\n]/g, ' ')
}

function pushIfExists(collection, file) {
  if (existsSync(file)) collection.push(file)
}

function preferExisting(...files) {
  return files.find((file) => existsSync(file)) ?? files[0]
}
