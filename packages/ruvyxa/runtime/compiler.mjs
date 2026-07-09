import { createHash } from 'node:crypto'
import { existsSync, statSync } from 'node:fs'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { stripTypeScriptTypes } from 'node:module'

const JS_EXTENSIONS = ['', '.ts', '.tsx', '.js', '.jsx', '.mts', '.mjs']
const ASSET_EXTENSIONS = new Set(['.css', '.scss', '.sass', '.less'])
const compilerCache = (globalThis.__RUVYXA_COMPILER_CACHE__ ??= {
  sources: new Map(),
  rewrites: new Map(),
})

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

export async function compileBundle({
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
  })

  const linked = linkModules(modules, externals, { minify, outfile, sourceMap })
  await mkdir(path.dirname(outfile), { recursive: true })
  await writeIfChanged(outfile, linked.code)
  if (linked.map) {
    await writeIfChanged(`${outfile}.map`, JSON.stringify(linked.map))
  }
  return outfile
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

export function runtimeAliases(runtimeDir = path.dirname(new URL(import.meta.url).pathname)) {
  const normalizedRuntimeDir =
    process.platform === 'win32' && runtimeDir.startsWith('/') ? runtimeDir.slice(1) : runtimeDir
  const packageRoot = path.resolve(normalizedRuntimeDir, '..')
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
  } = context

  if (byKey.has(key)) return byKey.get(key)

  const id = `__m${modules.length}`
  const module = {
    id,
    key,
    filePath,
    source,
    baseDir,
    deps: new Map(),
  }
  byKey.set(key, module)
  modules.push(module)

  if (platform === 'browser') {
    checkClientBoundary(root, filePath, source)
  }

  for (const specifier of extractSpecifiers(source)) {
    if (isAssetSpecifier(specifier)) continue

    const resolvedAlias = aliases[specifier]
    const resolved = resolvedAlias
      ? resolveFile(path.resolve(resolvedAlias))
      : resolveLocalSpecifier(baseDir, specifier)

    if (resolved && (resolvedAlias || isProjectLocal(root, resolved))) {
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

  for (const module of modules.slice().reverse()) {
    const rewritten = rewrittenModules.get(module.id)
    const sourceIndex =
      sourceMap && module.filePath
        ? sourceMapIndex(mapSources, module.filePath, module.source)
        : null

    push(`const ${module.id} = (() => {`)
    push(`  const __exports = {};`)
    const codeLines = rewritten.code.split('\n')
    for (let index = 0; index < codeLines.length; index++) {
      const line = codeLines[index]
      const originalLine = rewritten.lineMap[index]
      push(
        line ? `  ${line}` : '',
        sourceIndex !== null && originalLine !== null ? { sourceIndex, originalLine } : null,
      )
    }
    push(`  return __exports;`)
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
    code: minify ? code.replace(/\s+/g, ' ') : code,
    map: sourceMap && !minify ? buildSourceMap(outfile, lineMappings, mapSources) : null,
  }
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
    [...module.deps.entries()]
      .map(([specifier, dep]) => `${specifier}:${dep.external ? dep.alias : dep.id}`)
      .join('|'),
  ].join('\0')
  const cached = compilerCache.rewrites.get(rewriteKey)
  if (cached) return cached

  let source = shouldTransformJsx(module) ? transformJsx(module.source) : module.source
  source = stripTypes(source)
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

    lines.push(rewriteDynamicImports(rawLine, module))
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
  compilerCache.rewrites.set(rewriteKey, result)
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

function shouldTransformJsx(module) {
  const name = module.filePath || module.key || ''
  return name.endsWith('.tsx') || name.endsWith('.jsx')
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

function isAssetSpecifier(specifier) {
  return ASSET_EXTENSIONS.has(path.extname(specifier).toLowerCase())
}

async function readSourceFile(file) {
  const stats = statSync(file)
  const cacheKey = path.resolve(file)
  const cached = compilerCache.sources.get(cacheKey)
  if (cached && cached.mtimeMs === stats.mtimeMs && cached.size === stats.size) {
    return cached.source
  }
  const source = await readFile(file, 'utf8')
  compilerCache.sources.set(cacheKey, {
    mtimeMs: stats.mtimeMs,
    size: stats.size,
    source,
  })
  return source
}

async function writeIfChanged(file, contents) {
  try {
    if ((await readFile(file, 'utf8')) === contents) return
  } catch {
    // File does not exist yet.
  }
  await writeFile(file, contents)
}

function stripTypes(source) {
  return stripTypeScriptTypes(source, { mode: 'strip' })
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
  if (extractSpecifiers(source).includes('server-only')) {
    throw new Error(`RUV1007: Server-only module imported into client bundle: ${filePath}`)
  }
  for (const envName of privateEnvReads(source)) {
    throw new Error(
      `RUV1008: Private environment variable ${envName} used in client bundle: ${filePath}`,
    )
  }
}

function transformJsx(source) {
  const parser = new JsxTransformer(source)
  return parser.run()
}

class JsxTransformer {
  constructor(source) {
    this.source = source
    this.index = 0
    this.output = ''
  }

  run() {
    while (this.index < this.source.length) {
      if (this.source.startsWith('<>', this.index)) {
        this.output += this.parseFragment()
      } else if (
        this.source[this.index] === '<' &&
        /[A-Za-z]/.test(this.source[this.index + 1] || '')
      ) {
        this.output += this.parseElement()
      } else {
        this.output += this.source[this.index++]
      }
    }
    return this.output
  }

  parseFragment() {
    this.index += 2
    const children = this.readChildren(null)
    return `React.createElement(React.Fragment, null${children.length ? `, ${children.join(', ')}` : ''})`
  }

  parseElement() {
    this.expect('<')
    const tag = this.readName()
    const props = []
    while (this.index < this.source.length) {
      this.skipWhitespace()
      if (this.source.startsWith('/>', this.index)) {
        this.index += 2
        return `React.createElement(${formatTag(tag)}, ${formatProps(props)})`
      }
      if (this.source[this.index] === '>') {
        this.index++
        break
      }
      props.push(this.readProp())
    }

    const children = this.readChildren(tag)
    return `React.createElement(${formatTag(tag)}, ${formatProps(props)}${children.length ? `, ${children.join(', ')}` : ''})`
  }

  readChildren(tag) {
    const children = []
    while (this.index < this.source.length) {
      if (tag === null && this.source.startsWith('</>', this.index)) {
        this.index += 3
        break
      }
      if (tag !== null && this.source.startsWith(`</${tag}`, this.index)) {
        this.index += tag.length + 2
        while (this.source[this.index] !== '>' && this.index < this.source.length) this.index++
        this.index++
        break
      }
      if (this.source.startsWith('<>', this.index)) {
        children.push(this.parseFragment())
        continue
      }
      if (this.source[this.index] === '<' && /[A-Za-z]/.test(this.source[this.index + 1] || '')) {
        children.push(this.parseElement())
        continue
      }
      if (this.source[this.index] === '{') {
        const expr = this.readBalanced('{', '}')
        const inner = expr.slice(1, -1).trim()
        if (inner && !isJsxComment(inner)) children.push(transformJsxExpression(inner))
        continue
      }
      const text = this.readText()
      if (text.trim()) children.push(JSON.stringify(text.replace(/\s+/g, ' ').trim()))
    }

    return children
  }

  readProp() {
    if (this.source.startsWith('{...', this.index)) {
      const expr = this.readBalanced('{', '}')
      return { spread: transformJsxExpression(expr.slice(4, -1).trim()) }
    }
    const name = this.readName()
    this.skipWhitespace()
    if (this.source[this.index] !== '=') return [name, 'true']
    this.index++
    this.skipWhitespace()
    const quote = this.source[this.index]
    if (quote === '"' || quote === "'") {
      this.index++
      const start = this.index
      while (this.source[this.index] !== quote && this.index < this.source.length) this.index++
      const value = this.source.slice(start, this.index)
      this.index++
      return [name, JSON.stringify(value)]
    }
    if (this.source[this.index] === '{') {
      const expr = this.readBalanced('{', '}')
      return [name, transformJsxExpression(expr.slice(1, -1).trim())]
    }
    return [name, 'true']
  }

  readText() {
    const start = this.index
    while (
      this.index < this.source.length &&
      this.source[this.index] !== '<' &&
      this.source[this.index] !== '{'
    ) {
      this.index++
    }
    return this.source.slice(start, this.index)
  }

  readBalanced(open, close) {
    const start = this.index
    let depth = 0
    while (this.index < this.source.length) {
      const char = this.source[this.index++]
      if (char === '"' || char === "'" || char === '`') {
        this.skipString(char)
        continue
      }
      if (char === open) depth++
      if (char === close && --depth === 0) break
    }
    return this.source.slice(start, this.index)
  }

  skipString(quote) {
    while (this.index < this.source.length) {
      const char = this.source[this.index++]
      if (char === '\\') {
        this.index++
        continue
      }
      if (char === quote) return
    }
  }

  readName() {
    const start = this.index
    while (/[A-Za-z0-9_$:.-]/.test(this.source[this.index] || '')) this.index++
    return this.source.slice(start, this.index)
  }

  skipWhitespace() {
    while (/\s/.test(this.source[this.index] || '')) this.index++
  }

  expect(char) {
    if (this.source[this.index] !== char) {
      const line = this.source.slice(0, this.index).split('\n').length
      const col = this.index - (this.source.lastIndexOf('\n', this.index - 1) + 1)
      const ctx = this.source
        .slice(Math.max(0, this.index - 20), this.index + 20)
        .replace(/\n/g, '\\n')
      throw new Error(`Expected '${char}' at line ${line}:${col} near "...${ctx}..."`)
    }
    this.index++
  }
}

function formatTag(tag) {
  return /^[a-z]/.test(tag) ? JSON.stringify(tag) : tag
}

function formatProps(props) {
  if (!props.length) return 'null'
  const objects = []
  let current = []

  for (const prop of props) {
    if (prop.spread) {
      if (current.length) {
        objects.push(`{ ${current.join(', ')} }`)
        current = []
      }
      objects.push(prop.spread)
      continue
    }
    const [name, value] = prop
    current.push(`${JSON.stringify(name)}: ${value}`)
  }

  if (current.length) objects.push(`{ ${current.join(', ')} }`)
  return objects.length === 1 ? objects[0] : `Object.assign({}, ${objects.join(', ')})`
}

function isJsxComment(value) {
  return value.startsWith('/*') && value.endsWith('*/')
}

function transformJsxExpression(value) {
  return maybeContainsJsx(value) ? transformJsx(value) : value
}

function maybeContainsJsx(value) {
  return /<[A-Za-z][\w:.-]*(\s|>|\/)/.test(value) || value.includes('<>')
}

function privateEnvReads(source) {
  const names = []
  const codeOnly = maskNonCode(source)
  for (const match of codeOnly.matchAll(/\bprocess\.env\.([A-Z_][A-Z0-9_]*)/g)) {
    const name = match[1]
    if (name !== 'NODE_ENV' && !name.startsWith('RUVYXA_PUBLIC_')) names.push(name)
  }
  return names
}

function maskNonCode(source, options = {}) {
  const preserveImportExportSpecifiers = options.preserveImportExportSpecifiers === true
  const preserveImportCallSpecifiers = options.preserveImportCallSpecifiers === true
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
        (preserveImportCallSpecifiers && /\bimport\s*\(\s*$/.test(previous))
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
