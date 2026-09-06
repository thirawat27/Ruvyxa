/**
 * Route rules: the path-to-regexp dialect behind `headers()`, `redirects()`,
 * `rewrites()`, and `proxy.matcher` in `ruvyxa.config.ts`.
 *
 * Every JavaScript host evaluates these rules through this one module — the
 * deployed handler through the copy `packages/ruvyxa/scripts/sync-shared-runtime.mjs` writes
 * into `runtime/route-rules.mjs` — and the Axum host through
 * `crates/ruvyxa_middleware/src/route_rules.rs`. The two cannot share code, so
 * they share `tests/fixtures/route-rules-conformance.json`, which states the
 * semantics once and is replayed by both. Change the fixture first.
 *
 * Dependency-free on purpose: the copy ships inside serverless function
 * bundles, where no bare specifier resolves.
 */

/** A condition on something other than the path. */
export interface RouteCondition {
  type: 'header' | 'cookie' | 'query' | 'host'
  /** Not used by `host`. Header keys compare case-insensitively. */
  key?: string
  /** A regex the whole value must match; named captures become parameters. */
  value?: string
}

export interface HeaderRule {
  source: string
  headers: readonly { key: string; value: string }[]
  has?: readonly RouteCondition[]
  missing?: readonly RouteCondition[]
}

export interface RedirectRule {
  source: string
  destination: string
  permanent?: boolean
  /** Overrides `permanent`; one of the two is required. */
  statusCode?: number
  has?: readonly RouteCondition[]
  missing?: readonly RouteCondition[]
}

export interface RewriteRule {
  source: string
  destination: string
  has?: readonly RouteCondition[]
  missing?: readonly RouteCondition[]
}

/** `rewrites()` may return one list or the three-phase object. */
export interface RewritePhases {
  beforeFiles?: readonly RewriteRule[]
  afterFiles?: readonly RewriteRule[]
  fallback?: readonly RewriteRule[]
}

export interface MatcherEntry {
  source: string
  has?: readonly RouteCondition[]
  missing?: readonly RouteCondition[]
}

/** What `proxy.matcher` accepts as written; normalized to {@link MatcherEntry}s. */
export type ProxyMatcher = string | readonly (string | MatcherEntry)[]

/** The rule lists a config carries, once every function form has been resolved. */
export interface RouteRuleSet {
  headers?: readonly HeaderRule[]
  redirects?: readonly RedirectRule[]
  rewrites?: RewritePhases
}

/** Every rule list compiled, in the shape both hosts evaluate per request. */
export interface CompiledRouteRules {
  headers: readonly CompiledHeaderRule[]
  redirects: readonly CompiledRedirectRule[]
  rewrites: {
    beforeFiles: readonly CompiledRewriteRule[]
    afterFiles: readonly CompiledRewriteRule[]
    fallback: readonly CompiledRewriteRule[]
  }
}

/** The parts of a request the rules read. */
export interface RuleRequest {
  /** Canonical path: decoded, no trailing slash, `/` for the root. */
  path: string
  /** Raw query string without the leading `?`. */
  query?: string
  /** Header name/value pairs as the client sent them. */
  headers?: Iterable<readonly [string, string]>
  host?: string
}

export type RuleParams = Record<string, string>

export interface CompiledSource {
  readonly source: string
  readonly regex: RegExp
  /** Capture names in group order; a digit string names an unnamed group. */
  readonly names: readonly string[]
}

const SEGMENT = '[^/]+'

/**
 * Compile a source pattern.
 *
 * The grammar is the common path-to-regexp subset: literal text, `:name` with an
 * optional `(regex)` and one of `* + ?`, a bare `(regex)` group, and `\\x`
 * escapes. A parameter's `/` prefix belongs to it, so `/blog/:slug*` still
 * matches `/blog`. Anchored, case-insensitive, whole-path. An optional trailing
 * slash rides along, as in path-to-regexp: canonical paths never carry one
 * except the root, and that is exactly what lets `/:path*` match `/`.
 */
export function compileSource(source: string): CompiledSource {
  if (typeof source !== 'string' || !source.startsWith('/')) {
    throw new TypeError(`RUV1602 route rule source must start with "/": ${JSON.stringify(source)}`)
  }
  let pattern = ''
  const names: string[] = []
  let unnamed = 0
  let index = 0
  while (index < source.length) {
    const char = source[index]
    if (char === '\\') {
      const next = source[index + 1]
      if (next === undefined) {
        throw new TypeError(
          `RUV1602 route rule source ends in an escape: ${JSON.stringify(source)}`,
        )
      }
      pattern += escapeRegex(next)
      index += 2
      continue
    }
    if (char === ':' || char === '(') {
      // A parameter owns the `/` written before it, so a modifier can make the
      // whole segment optional. Pull that slash back out of the literal text.
      let prefix = ''
      if (pattern.endsWith('\\/')) {
        pattern = pattern.slice(0, -2)
        prefix = '/'
      }
      let name = ''
      if (char === ':') {
        index += 1
        while (index < source.length && /[A-Za-z0-9_]/.test(source[index])) {
          name += source[index]
          index += 1
        }
        if (name === '') {
          throw new TypeError(
            `RUV1602 route rule parameter needs a name: ${JSON.stringify(source)}`,
          )
        }
      }
      let capture = SEGMENT
      if (source[index] === '(') {
        const end = matchingParen(source, index)
        if (end === -1) {
          throw new TypeError(`RUV1602 route rule group is not closed: ${JSON.stringify(source)}`)
        }
        capture = source.slice(index + 1, end)
        if (capture === '') {
          throw new TypeError(`RUV1602 route rule group is empty: ${JSON.stringify(source)}`)
        }
        index = end + 1
      }
      if (name === '') name = String(unnamed++)
      const modifier = source[index]
      if (modifier === '*' || modifier === '+' || modifier === '?') index += 1
      names.push(name)
      const escapedPrefix = escapeRegex(prefix)
      if (modifier === '*' || modifier === '+') {
        // The capture is the joined remainder; the inner group repeats the
        // segment class across slashes.
        const repeated = `(?:${capture})(?:/(?:${capture}))*`
        pattern +=
          modifier === '+' ? `${escapedPrefix}(${repeated})` : `(?:${escapedPrefix}(${repeated}))?`
      } else if (modifier === '?') {
        pattern += `(?:${escapedPrefix}(${capture}))?`
      } else {
        pattern += `${escapedPrefix}(${capture})`
      }
      continue
    }
    pattern += escapeRegex(char)
    index += 1
  }
  let regex: RegExp
  try {
    regex = new RegExp(`^${pattern}/?$`, 'i')
  } catch (error) {
    throw new TypeError(
      `RUV1602 route rule source ${JSON.stringify(source)} is not a valid pattern: ${
        error instanceof Error ? error.message : String(error)
      }`,
    )
  }
  return Object.freeze({ source, regex, names: Object.freeze(names) })
}

function matchingParen(text: string, open: number): number {
  let depth = 0
  for (let index = open; index < text.length; index += 1) {
    const char = text[index]
    if (char === '\\') {
      index += 1
      continue
    }
    if (char === '(') depth += 1
    if (char === ')') {
      depth -= 1
      if (depth === 0) return index
    }
  }
  return -1
}

function escapeRegex(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')
}

/** Parameters when `path` matches the compiled source, or `null`. */
export function matchSource(compiled: CompiledSource, path: string): RuleParams | null {
  const matched = compiled.regex.exec(path)
  if (!matched) return null
  const params: RuleParams = {}
  compiled.names.forEach((name, position) => {
    const value = matched[position + 1]
    if (value !== undefined) params[name] = value
  })
  return params
}

function headerValue(request: RuleRequest, name: string): string | undefined {
  const wanted = name.toLowerCase()
  for (const [key, value] of request.headers ?? []) {
    if (key.toLowerCase() === wanted) return value
  }
  return undefined
}

function cookieValue(request: RuleRequest, name: string): string | undefined {
  const header = headerValue(request, 'cookie')
  if (header === undefined) return undefined
  for (const part of header.split(';')) {
    const [key, ...rest] = part.trim().split('=')
    if (key === name) return rest.join('=')
  }
  return undefined
}

function queryValue(request: RuleRequest, name: string): string | undefined {
  if (!request.query) return undefined
  const value = new URLSearchParams(request.query).get(name)
  return value === null ? undefined : value
}

/** Whether one condition holds; its named captures when it does. */
function evaluateCondition(condition: RouteCondition, request: RuleRequest): RuleParams | null {
  let actual: string | undefined
  switch (condition.type) {
    case 'header':
      actual = condition.key === undefined ? undefined : headerValue(request, condition.key)
      break
    case 'cookie':
      actual = condition.key === undefined ? undefined : cookieValue(request, condition.key)
      break
    case 'query':
      actual = condition.key === undefined ? undefined : queryValue(request, condition.key)
      break
    case 'host':
      actual = request.host
      break
    default:
      return null
  }
  if (actual === undefined) return null
  if (condition.value === undefined) return {}
  const matched = new RegExp(`^(?:${condition.value})$`).exec(actual)
  if (!matched) return null
  const captured: RuleParams = {}
  for (const [name, value] of Object.entries(matched.groups ?? {})) {
    if (value !== undefined) captured[name] = value
  }
  return captured
}

/**
 * Every `has` must hold and no `missing` may; the merged captures, or `null`.
 */
export function evaluateConditions(
  has: readonly RouteCondition[] | undefined,
  missing: readonly RouteCondition[] | undefined,
  request: RuleRequest,
): RuleParams | null {
  const params: RuleParams = {}
  for (const condition of has ?? []) {
    const captured = evaluateCondition(condition, request)
    if (captured === null) return null
    Object.assign(params, captured)
  }
  for (const condition of missing ?? []) {
    if (evaluateCondition(condition, request) !== null) return null
  }
  return params
}

/** Match a rule's source and conditions together. */
export function matchRule(
  rule: { source: string; has?: readonly RouteCondition[]; missing?: readonly RouteCondition[] },
  compiled: CompiledSource,
  request: RuleRequest,
): RuleParams | null {
  const fromPath = matchSource(compiled, request.path)
  if (fromPath === null) return null
  const fromConditions = evaluateConditions(rule.has, rule.missing, request)
  if (fromConditions === null) return null
  return { ...fromPath, ...fromConditions }
}

const TOKEN = /:([A-Za-z0-9_]+)[*+?]?/g

/** Replace `:name` tokens; an absent parameter substitutes the empty string. */
export function substitute(template: string, params: RuleParams): string {
  return template.replace(TOKEN, (_, name: string) => params[name] ?? '')
}

function templateUsesParams(template: string): boolean {
  TOKEN.lastIndex = 0
  return TOKEN.test(template)
}

export interface CompiledHeaderRule {
  rule: HeaderRule
  compiled: CompiledSource
}

export function compileHeaderRules(rules: readonly HeaderRule[]): CompiledHeaderRule[] {
  return rules.map((rule) => ({ rule, compiled: compileSource(rule.source) }))
}

/** Response headers every matching rule sets, later rules overriding earlier. */
export function applyHeaderRules(
  rules: readonly CompiledHeaderRule[],
  request: RuleRequest,
): [string, string][] {
  const result = new Map<string, [string, string]>()
  for (const { rule, compiled } of rules) {
    const params = matchRule(rule, compiled, request)
    if (params === null) continue
    for (const header of rule.headers) {
      const key = substitute(header.key, params)
      result.set(key.toLowerCase(), [key, substitute(header.value, params)])
    }
  }
  return [...result.values()]
}

export interface CompiledRedirectRule {
  rule: RedirectRule
  compiled: CompiledSource
}

export function compileRedirectRules(rules: readonly RedirectRule[]): CompiledRedirectRule[] {
  return rules.map((rule) => {
    if (rule.statusCode === undefined && typeof rule.permanent !== 'boolean') {
      throw new TypeError(
        `RUV1602 redirect for ${JSON.stringify(rule.source)} needs permanent or statusCode`,
      )
    }
    return { rule, compiled: compileSource(rule.source) }
  })
}

export interface RedirectDecision {
  location: string
  status: number
}

/** The first redirect that matches, with the request query carried when the destination has none. */
export function matchRedirect(
  rules: readonly CompiledRedirectRule[],
  request: RuleRequest,
): RedirectDecision | null {
  for (const { rule, compiled } of rules) {
    const params = matchRule(rule, compiled, request)
    if (params === null) continue
    // Split before substitution: a `?` after a parameter name would otherwise
    // read as the optional modifier.
    const [base, destinationQuery] = splitQuery(rule.destination)
    let location = substitute(base, params)
    if (destinationQuery !== undefined) location += `?${substitute(destinationQuery, params)}`
    else if (request.query) location += `?${request.query}`
    const status = rule.statusCode ?? (rule.permanent ? 308 : 307)
    return { location, status }
  }
  return null
}

export interface CompiledRewriteRule {
  rule: RewriteRule
  compiled: CompiledSource
}

export function compileRewriteRules(rules: readonly RewriteRule[]): CompiledRewriteRule[] {
  return rules.map((rule) => ({ rule, compiled: compileSource(rule.source) }))
}

/**
 * The first rewrite that matches, as a target URL or path with query.
 *
 * Parameters the destination does not use are appended to its query when it
 * uses none; the request's own query is merged after.
 */
export function matchRewrite(
  rules: readonly CompiledRewriteRule[],
  request: RuleRequest,
): string | null {
  for (const { rule, compiled } of rules) {
    const params = matchRule(rule, compiled, request)
    if (params === null) continue
    const usesParams = templateUsesParams(rule.destination)
    const [baseTemplate, queryTemplate] = splitQuery(rule.destination)
    const base = substitute(baseTemplate, params)
    const query = new URLSearchParams(
      queryTemplate === undefined ? '' : substitute(queryTemplate, params),
    )
    if (!usesParams) {
      for (const [name, value] of Object.entries(params)) query.append(name, value)
    }
    if (request.query) {
      for (const [name, value] of new URLSearchParams(request.query)) query.append(name, value)
    }
    const serialized = query.toString()
    return serialized ? `${base}?${serialized}` : base
  }
  return null
}

function splitQuery(target: string): [string, string | undefined] {
  const at = target.indexOf('?')
  return at === -1 ? [target, undefined] : [target.slice(0, at), target.slice(at + 1)]
}

export interface CompiledMatcherEntry {
  entry: MatcherEntry
  compiled: CompiledSource
}

/**
 * The matcher as written, reduced to one shape.
 *
 * `undefined` stays `undefined` — a proxy with no matcher runs on every
 * request, which is a different thing from an empty matcher that runs on none.
 */
export function normalizeMatcher(matcher: ProxyMatcher | undefined): MatcherEntry[] | undefined {
  if (matcher === undefined) return undefined
  const entries = typeof matcher === 'string' ? [matcher] : matcher
  if (!Array.isArray(entries)) {
    throw new TypeError('RUV1602 proxy.matcher must be a string or an array')
  }
  return entries.map((value) => {
    const entry: MatcherEntry = typeof value === 'string' ? { source: value } : value
    if (!entry || typeof entry !== 'object' || typeof entry.source !== 'string') {
      throw new TypeError('RUV1602 proxy.matcher entries need a source')
    }
    return {
      source: entry.source,
      ...(entry.has ? { has: entry.has } : {}),
      ...(entry.missing ? { missing: entry.missing } : {}),
    }
  })
}

/** Compile normalized matcher entries. */
export function compileMatcher(entries: readonly MatcherEntry[]): CompiledMatcherEntry[] {
  return entries.map((entry) => ({ entry, compiled: compileSource(entry.source) }))
}

/**
 * `rewrites()` as written, reduced to the three phases.
 *
 * A bare list is `afterFiles`: checked after static files and pages, before
 * dynamic routes.
 */
export function normalizeRewrites(
  value: readonly RewriteRule[] | RewritePhases | undefined,
): Required<RewritePhases> {
  if (value === undefined) return { beforeFiles: [], afterFiles: [], fallback: [] }
  if (Array.isArray(value)) return { beforeFiles: [], afterFiles: value, fallback: [] }
  const phases = value as RewritePhases
  return {
    beforeFiles: phases.beforeFiles ?? [],
    afterFiles: phases.afterFiles ?? [],
    fallback: phases.fallback ?? [],
  }
}

/** Compile every rule list a host evaluates, once at startup. */
export function compileRouteRules(rules: RouteRuleSet | undefined): CompiledRouteRules {
  const rewrites = normalizeRewrites(rules?.rewrites)
  return {
    headers: compileHeaderRules(rules?.headers ?? []),
    redirects: compileRedirectRules(rules?.redirects ?? []),
    rewrites: {
      beforeFiles: compileRewriteRules(rewrites.beforeFiles),
      afterFiles: compileRewriteRules(rewrites.afterFiles),
      fallback: compileRewriteRules(rewrites.fallback),
    },
  }
}

/** Whether any rule list is non-empty, so a host can skip the stage entirely. */
export function hasRouteRules(rules: CompiledRouteRules): boolean {
  return (
    rules.headers.length > 0 ||
    rules.redirects.length > 0 ||
    rules.rewrites.beforeFiles.length > 0 ||
    rules.rewrites.afterFiles.length > 0 ||
    rules.rewrites.fallback.length > 0
  )
}

/** Whether any matcher entry matches; an empty matcher matches nothing. */
export function matcherMatches(
  entries: readonly CompiledMatcherEntry[],
  request: RuleRequest,
): boolean {
  return entries.some(({ entry, compiled }) => matchRule(entry, compiled, request) !== null)
}

/** Split a request URL into the pieces the rules read, given headers and host. */
export function ruleRequestFromUrl(
  url: URL,
  headers: Iterable<readonly [string, string]>,
  canonicalPath: string,
): RuleRequest {
  return {
    path: canonicalPath,
    query: url.search.startsWith('?') ? url.search.slice(1) : url.search,
    headers,
    host: url.host,
  }
}
