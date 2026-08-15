import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'

const RESERVED = /^(?:react(?:-dom)?|ruvyxa)(?:\/|$)|^@ruvyxa\//

/** Load the effective local tsconfig/jsconfig alias model with extends support. */
export function loadTsconfigPaths(projectRoot) {
  const root = path.resolve(projectRoot)
  for (const name of ['tsconfig.json', 'jsconfig.json']) {
    const file = path.join(root, name)
    if (!existsSync(file)) continue
    const loaded = loadConfig(file, root, new Set())
    loaded.model.problem = loaded.problem
    return loaded.model
  }
  return emptyModel(root)
}

export function resolveTsconfigPath(model, specifier, resolveFile) {
  if (RESERVED.test(specifier)) return null
  for (const declaration of [...model.paths].sort(comparePatterns)) {
    const wildcard = matchPattern(declaration.pattern, specifier)
    if (wildcard === null) continue
    for (const target of declaration.targets) {
      const candidate = path.resolve(declaration.base, target.replace('*', wildcard))
      const resolved = resolveFile(candidate)
      if (resolved && isWithin(model.projectRoot, resolved)) return resolved
    }
  }
  if (model.baseUrl && !specifier.startsWith('.') && !path.isAbsolute(specifier)) {
    const resolved = resolveFile(path.join(model.baseUrl, specifier))
    if (resolved && isWithin(model.projectRoot, resolved)) return resolved
  }
  return null
}

/** Resolve an alias-bearing glob to an absolute pattern without probing it as a file. */
export function resolveTsconfigGlobPattern(model, pattern) {
  if (RESERVED.test(pattern)) return null
  for (const declaration of [...model.paths].sort(comparePatterns)) {
    const wildcard = matchPattern(declaration.pattern, pattern)
    if (wildcard === null || declaration.targets.length === 0) continue
    const candidate = path.resolve(declaration.base, declaration.targets[0].replace('*', wildcard))
    return isWithin(model.projectRoot, staticGlobPrefix(candidate)) ? candidate : null
  }
  if (model.baseUrl && !pattern.startsWith('.') && !path.isAbsolute(pattern)) {
    const candidate = path.resolve(model.baseUrl, pattern)
    return isWithin(model.projectRoot, staticGlobPrefix(candidate)) ? candidate : null
  }
  return null
}

function staticGlobPrefix(pattern) {
  const wildcard = pattern.search(/[?*]/)
  return wildcard < 0 ? pattern : pattern.slice(0, wildcard)
}

function loadConfig(file, projectRoot, visiting) {
  const absolute = path.resolve(file)
  const cycleKey = absolute.toLowerCase()
  if (visiting.has(cycleKey)) {
    return { model: emptyModel(projectRoot), problem: `cyclic extends chain at ${absolute}` }
  }
  visiting.add(cycleKey)
  let value
  try {
    value = JSON.parse(stripJsonc(readFileSync(absolute, 'utf8')))
  } catch (error) {
    visiting.delete(cycleKey)
    return { model: emptyModel(projectRoot), problem: String(error?.message ?? error) }
  }

  const directory = path.dirname(absolute)
  let problem = null
  let model = emptyModel(projectRoot)
  if (typeof value.extends === 'string') {
    const parent = resolveExtends(value.extends, directory)
    if (parent) {
      const loaded = loadConfig(parent, projectRoot, visiting)
      model = loaded.model
      problem = loaded.problem
    } else {
      problem = `cannot resolve extended configuration ${JSON.stringify(value.extends)}`
    }
  }

  const options = value.compilerOptions
  if (options && typeof options === 'object' && !Array.isArray(options)) {
    if (typeof options.baseUrl === 'string')
      model.baseUrl = path.resolve(directory, options.baseUrl)
    if (options.paths && typeof options.paths === 'object' && !Array.isArray(options.paths)) {
      // TypeScript resolves `paths` against the effective `baseUrl` — including
      // one inherited through `extends` — and only falls back to the declaring
      // config's directory when no `baseUrl` is in effect. `model.baseUrl`
      // already holds local-over-inherited above, so reading it here is what
      // keeps a base config's `baseUrl` from being silently ignored by a child
      // that declares `paths`.
      const declarationBase = model.baseUrl ?? directory
      model.paths = Object.entries(options.paths).map(([pattern, targets]) => ({
        pattern,
        targets: Array.isArray(targets)
          ? targets.filter((target) => typeof target === 'string')
          : [],
        base: declarationBase,
        source: absolute,
      }))
    }
  }
  model.files = [...new Set([...model.files, absolute])].sort()
  visiting.delete(cycleKey)
  return { model, problem }
}

function emptyModel(projectRoot) {
  return { projectRoot, baseUrl: null, paths: [], files: [], problem: null }
}

function resolveExtends(specifier, directory) {
  if (path.isAbsolute(specifier) || specifier.startsWith('.')) {
    return configCandidate(path.resolve(directory, specifier))
  }
  let current = directory
  while (true) {
    const candidate = path.join(current, 'node_modules', specifier)
    const direct = configCandidate(candidate)
    if (direct) return direct
    if (existsSync(path.join(candidate, 'package.json'))) {
      try {
        const manifest = JSON.parse(readFileSync(path.join(candidate, 'package.json'), 'utf8'))
        const packaged = configCandidate(path.join(candidate, manifest.tsconfig ?? 'tsconfig.json'))
        if (packaged) return packaged
      } catch {
        // Continue searching parent node_modules directories.
      }
    }
    const parent = path.dirname(current)
    if (parent === current) return null
    current = parent
  }
}

function configCandidate(candidate) {
  for (const file of [candidate, `${candidate}.json`, path.join(candidate, 'tsconfig.json')]) {
    if (existsSync(file)) return path.resolve(file)
  }
  return null
}

/**
 * Order alias declarations from most to least specific.
 *
 * TypeScript probes the most specific matching pattern first, so an exact
 * pattern must outrank a wildcard before either is tried as a file.
 *
 * The final tiebreak compares code units rather than using `localeCompare`,
 * matching `alias_pattern_order` in the Rust resolver. Two patterns of equal
 * specificity can never both match one specifier — equal literal prefix and
 * suffix lengths force the patterns to be identical — so this only decides a
 * stable order. Keeping it locale-independent means the two graphs sort a
 * config the same way on every machine.
 */
function comparePatterns(left, right) {
  const leftSpecificity = patternSpecificity(left.pattern)
  const rightSpecificity = patternSpecificity(right.pattern)
  for (let index = 0; index < leftSpecificity.length; index++) {
    if (leftSpecificity[index] !== rightSpecificity[index]) {
      return rightSpecificity[index] - leftSpecificity[index]
    }
  }
  if (left.pattern === right.pattern) return 0
  return left.pattern < right.pattern ? -1 : 1
}

/** Specificity components, most significant first. A higher value wins. */
function patternSpecificity(pattern) {
  const star = pattern.indexOf('*')
  const isExact = star < 0
  const literalPrefixLength = isExact ? pattern.length : star
  const literalSuffixLength = isExact ? 0 : pattern.length - star - 1
  return [isExact ? 1 : 0, literalPrefixLength, literalSuffixLength, pattern.length]
}

function matchPattern(pattern, specifier) {
  const star = pattern.indexOf('*')
  if (star < 0) return pattern === specifier ? '' : null
  if (pattern.indexOf('*', star + 1) >= 0) return null
  const prefix = pattern.slice(0, star)
  const suffix = pattern.slice(star + 1)
  if (!specifier.startsWith(prefix) || !specifier.endsWith(suffix)) return null
  const end = suffix.length === 0 ? specifier.length : specifier.length - suffix.length
  return specifier.slice(prefix.length, end)
}

function isWithin(root, file) {
  const relative = path.relative(root, file)
  return (
    relative === '' ||
    (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative))
  )
}

function stripJsonc(source) {
  let output = ''
  let inString = false
  let escaped = false
  for (let index = 0; index < source.length; index++) {
    const character = source[index]
    const next = source[index + 1]
    if (inString) {
      output += character
      if (escaped) escaped = false
      else if (character === '\\') escaped = true
      else if (character === '"') inString = false
      continue
    }
    if (character === '"') {
      inString = true
      output += character
    } else if (character === '/' && next === '/') {
      while (index < source.length && source[index] !== '\n') index++
      output += '\n'
    } else if (character === '/' && next === '*') {
      index += 2
      while (index < source.length && !(source[index - 1] === '*' && source[index] === '/')) {
        if (source[index] === '\n') output += '\n'
        index++
      }
    } else {
      output += character
    }
  }
  let normalized = ''
  inString = false
  escaped = false
  for (let index = 0; index < output.length; index++) {
    const character = output[index]
    if (inString) {
      normalized += character
      if (escaped) escaped = false
      else if (character === '\\') escaped = true
      else if (character === '"') inString = false
      continue
    }
    if (character === '"') {
      inString = true
      normalized += character
      continue
    }
    if (character === ',') {
      let lookahead = index + 1
      while (/\s/.test(output[lookahead] ?? '')) lookahead++
      if (output[lookahead] === '}' || output[lookahead] === ']') continue
    }
    normalized += character
  }
  return normalized
}
