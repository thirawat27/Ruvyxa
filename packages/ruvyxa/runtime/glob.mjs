import { existsSync } from 'node:fs'
import { readdir } from 'node:fs/promises'
import path from 'node:path'
import { resolveTsconfigGlobPattern } from './paths.mjs'
import { directivePrologueEnd, findInCode } from './scanner.mjs'

const IGNORED_DIRECTORIES = new Set(['.git', '.ruvyxa', 'dist', 'node_modules', 'target'])
const MARKER = 'import.meta.glob'

/** Expand literal import.meta.glob calls before Oxc runs. */
export async function expandImportMetaGlob(source, importerDir, projectRoot, tsconfigPaths) {
  const replacements = []
  const hoistedImports = []
  const inputs = new Set()
  const matches = new Set()
  let callIndex = 0
  for (const call of findCalls(source)) {
    const unresolved = call.pattern.startsWith('.')
      ? path.join(importerDir, call.pattern)
      : resolveTsconfigGlobPattern(tsconfigPaths, call.pattern)
    if (!unresolved) throw globError(`cannot resolve pattern ${JSON.stringify(call.pattern)}`)
    // Resolve the whole pattern, not just its static prefix, so a `..` segment
    // after a wildcard is rejected here exactly as the Rust resolver rejects it.
    const absolutePattern = path.resolve(unresolved)
    if (!isWithin(projectRoot, absolutePattern)) {
      throw globError(`pattern ${JSON.stringify(call.pattern)} escapes the project root`)
    }
    const watchRoot = findWatchRoot(absolutePattern, projectRoot)
    inputs.add(watchRoot)
    const files = await collectFiles(watchRoot, absolutePattern)
    const entries = files.map((file, matchIndex) => {
      matches.add(file)
      const specifier = relativeSpecifier(importerDir, file)
      if (!call.eager) {
        return `${JSON.stringify(specifier)}: () => import(${JSON.stringify(specifier)})`
      }
      // Eager matches must enter the static dependency graph, so they lower to
      // a real namespace import rather than a `require()` call: `require` is
      // undefined in an ES module and only the Rust linker rewrites it, which
      // made eager globs throw at runtime on this graph.
      const binding = `__ruvyxaGlob${callIndex}_${matchIndex}`
      hoistedImports.push(`import * as ${binding} from ${JSON.stringify(specifier)}`)
      return `${JSON.stringify(specifier)}: ${binding}`
    })
    replacements.push({ start: call.start, end: call.end, value: `{${entries.join(', ')}}` })
    callIndex += 1
  }

  let expanded = source
  for (const replacement of replacements.reverse()) {
    expanded =
      expanded.slice(0, replacement.start) + replacement.value + expanded.slice(replacement.end)
  }
  if (hoistedImports.length > 0) {
    // Insert after the directive prologue: above it would demote a
    // `'use client'` directive to a plain string, and below every use would put
    // the linker's rewritten `const` binding in the temporal dead zone.
    const insertAt = directivePrologueEnd(expanded)
    expanded = `${expanded.slice(0, insertAt)}\n${hoistedImports.join('\n')}\n${expanded.slice(insertAt)}`
  }
  return {
    source: expanded,
    inputs: [...inputs].sort(compareBySlashedPath),
    matches: [...matches].sort(compareBySlashedPath),
  }
}

function findCalls(source) {
  return findInCode(source, MARKER).map((index) => parseCall(source, index))
}

/**
 * Order paths by their slash-normalized code units.
 *
 * This must stay a plain code-unit comparison. `localeCompare` sorts `a.ts`
 * before `B.ts` while the Rust expander's `String` ordering sorts `B.ts` first,
 * so the two graphs generated different glob key orders — and `localeCompare`
 * additionally varies with the host ICU locale, which made builds
 * irreproducible across machines.
 */
function compareBySlashedPath(left, right) {
  const leftPath = slash(left)
  const rightPath = slash(right)
  return leftPath < rightPath ? -1 : leftPath > rightPath ? 1 : 0
}

function parseCall(source, start) {
  let cursor = skipWhitespace(source, start + 'import.meta.glob'.length)
  if (source[cursor] !== '(') throw globError('import.meta.glob must be called directly')
  cursor = skipWhitespace(source, cursor + 1)
  if (source[cursor] !== '"' && source[cursor] !== "'") {
    throw globError('pattern must be a string literal')
  }
  const quote = source[cursor++]
  let pattern = ''
  let closed = false
  while (cursor < source.length) {
    const character = source[cursor++]
    if (character === '\\') {
      if (cursor >= source.length) break
      pattern += source[cursor++]
    } else if (character === quote) {
      closed = true
      break
    } else pattern += character
  }
  if (!closed) throw globError('unterminated pattern')
  cursor = skipWhitespace(source, cursor)
  let eager = false
  if (source[cursor] === ',') {
    const close = source.indexOf(')', ++cursor)
    if (close < 0) throw globError('unterminated call')
    const options = source.slice(cursor, close).replaceAll(/\s/g, '')
    if (options === '{eager:true}') eager = true
    else if (options !== '{eager:false}') {
      throw globError('options must be the literal `{ eager: true }`')
    }
    cursor = close
  }
  cursor = skipWhitespace(source, cursor)
  if (source[cursor] !== ')') {
    throw globError('call must contain one literal pattern and optional eager flag')
  }
  return { start, end: cursor + 1, pattern, eager }
}

async function collectFiles(root, absolutePattern) {
  if (!existsSync(root)) return []
  const pattern = slash(absolutePattern)
  const pending = [root]
  const files = []
  while (pending.length > 0) {
    const directory = pending.pop()
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name)
      if (entry.isDirectory() && !IGNORED_DIRECTORIES.has(entry.name)) pending.push(file)
      else if (entry.isFile() && globMatches(pattern, slash(file))) files.push(file)
    }
  }
  return files.sort(compareBySlashedPath)
}

function globMatches(pattern, value, patternIndex = 0, valueIndex = 0) {
  if (patternIndex === pattern.length) return valueIndex === value.length
  if (
    pattern[patternIndex] === '*' &&
    pattern[patternIndex + 1] === '*' &&
    pattern[patternIndex + 2] === '/'
  ) {
    return (
      globMatches(pattern, value, patternIndex + 3, valueIndex) ||
      (valueIndex < value.length && globMatches(pattern, value, patternIndex, valueIndex + 1))
    )
  }
  if (pattern[patternIndex] === '*' && pattern[patternIndex + 1] === '*') {
    return (
      globMatches(pattern, value, patternIndex + 2, valueIndex) ||
      (valueIndex < value.length && globMatches(pattern, value, patternIndex, valueIndex + 1))
    )
  }
  if (pattern[patternIndex] === '*') {
    return (
      globMatches(pattern, value, patternIndex + 1, valueIndex) ||
      (valueIndex < value.length &&
        value[valueIndex] !== '/' &&
        globMatches(pattern, value, patternIndex, valueIndex + 1))
    )
  }
  if (pattern[patternIndex] === '?') {
    return (
      valueIndex < value.length &&
      value[valueIndex] !== '/' &&
      globMatches(pattern, value, patternIndex + 1, valueIndex + 1)
    )
  }
  return (
    pattern[patternIndex] === value[valueIndex] &&
    globMatches(pattern, value, patternIndex + 1, valueIndex + 1)
  )
}

function findWatchRoot(pattern, projectRoot) {
  const prefix = staticPrefix(pattern)
  const endsAtDirectory = prefix.endsWith('/') || prefix.endsWith('\\')
  let candidate = endsAtDirectory ? prefix.replace(/[\\/]+$/, '') : path.dirname(prefix)
  while (!existsSync(candidate) && isWithin(projectRoot, candidate)) {
    const parent = path.dirname(candidate)
    if (parent === candidate) break
    candidate = parent
  }
  return isWithin(projectRoot, candidate) ? candidate : projectRoot
}

function staticPrefix(pattern) {
  const wildcard = pattern.search(/[?*]/)
  return wildcard < 0 ? pattern : pattern.slice(0, wildcard)
}

function relativeSpecifier(directory, file) {
  const relative = slash(path.relative(directory, file))
  return relative.startsWith('.') ? relative : `./${relative}`
}

function skipWhitespace(source, start) {
  let cursor = start
  while (/\s/.test(source[cursor] ?? '')) cursor += 1
  return cursor
}

function isWithin(root, file) {
  const relative = path.relative(path.resolve(root), path.resolve(file))
  return (
    relative === '' ||
    (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative))
  )
}

function slash(file) {
  return file.replaceAll('\\', '/')
}

function globError(message) {
  return new Error(`RUV1810 import.meta.glob: ${message}`)
}
