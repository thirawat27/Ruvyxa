import { existsSync } from 'node:fs'
import { readdir } from 'node:fs/promises'
import path from 'node:path'
import { resolveTsconfigGlobPattern } from './paths.mjs'

const IGNORED_DIRECTORIES = new Set(['.git', '.ruvyxa', 'dist', 'node_modules', 'target'])

/** Expand literal import.meta.glob calls before Oxc runs. */
export async function expandImportMetaGlob(source, importerDir, projectRoot, tsconfigPaths) {
  const replacements = []
  const inputs = new Set()
  const matches = new Set()
  for (const call of findCalls(source)) {
    const unresolved = call.pattern.startsWith('.')
      ? path.join(importerDir, call.pattern)
      : resolveTsconfigGlobPattern(tsconfigPaths, call.pattern)
    if (!unresolved) throw globError(`cannot resolve pattern ${JSON.stringify(call.pattern)}`)
    const absolutePattern = path.resolve(unresolved)
    if (!isWithin(projectRoot, staticPrefix(absolutePattern))) {
      throw globError(`pattern ${JSON.stringify(call.pattern)} escapes the project root`)
    }
    const watchRoot = findWatchRoot(absolutePattern, projectRoot)
    inputs.add(watchRoot)
    const files = await collectFiles(watchRoot, absolutePattern)
    const entries = files.map((file) => {
      matches.add(file)
      const specifier = relativeSpecifier(importerDir, file)
      const value = call.eager
        ? `require(${JSON.stringify(specifier)})`
        : `() => import(${JSON.stringify(specifier)})`
      return `${JSON.stringify(specifier)}: ${value}`
    })
    replacements.push({ start: call.start, end: call.end, value: `{${entries.join(', ')}}` })
  }

  let expanded = source
  for (const replacement of replacements.reverse()) {
    expanded =
      expanded.slice(0, replacement.start) + replacement.value + expanded.slice(replacement.end)
  }
  return { source: expanded, inputs: [...inputs].sort(), matches: [...matches].sort() }
}

function findCalls(source) {
  const calls = []
  scanCode(source, 0, calls, false)
  return calls
}

function scanCode(source, start, calls, stopAtClosingBrace) {
  const marker = 'import.meta.glob'
  let index = start
  let braceDepth = 0
  while (index < source.length) {
    const character = source[index]
    if (character === '/' && source[index + 1] === '/') {
      index = source.indexOf('\n', index + 2)
      if (index < 0) break
    } else if (character === '/' && source[index + 1] === '*') {
      const end = source.indexOf('*/', index + 2)
      index = end < 0 ? source.length : end + 2
    } else if (character === '"' || character === "'") {
      index = skipString(source, index)
    } else if (character === '`') {
      index = scanTemplate(source, index, calls)
    } else if (character === '{') {
      braceDepth += 1
      index += 1
    } else if (character === '}' && stopAtClosingBrace) {
      if (braceDepth === 0) return index + 1
      braceDepth -= 1
      index += 1
    } else if (source.startsWith(marker, index)) {
      const parsed = parseCall(source, index)
      calls.push(parsed)
      index = parsed.end
    } else index += 1
  }
  return index
}

function scanTemplate(source, start, calls) {
  let index = start + 1
  while (index < source.length) {
    if (source[index] === '\\') index += 2
    else if (source[index] === '`') return index + 1
    else if (source[index] === '$' && source[index + 1] === '{') {
      index = scanCode(source, index + 2, calls, true)
    } else index += 1
  }
  return index
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
  return files.sort((left, right) => slash(left).localeCompare(slash(right)))
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

function skipString(source, start) {
  const quote = source[start]
  let index = start + 1
  while (index < source.length) {
    if (source[index] === '\\') index += 2
    else if (source[index++] === quote) break
  }
  return index
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
