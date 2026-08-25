/**
 * The only JavaScript-side source scanner.
 *
 * This is a faithful port of `crates/ruvyxa_bundler/src/ast.rs`. It exists for
 * the same reason that module exists on the Rust side: every consumer that
 * needs to *search* source text has to agree on where strings, templates,
 * comments, and regular expressions end. Each time a consumer re-derived that
 * decision, it got the regex case wrong — a literal such as `/['"]/` starts a
 * string skip that runs to the next quote anywhere in the file, and everything
 * in between silently stops being seen as code.
 *
 * That failure class was fixed at the root in Rust by making `ast.rs` the only
 * byte scanner. The JavaScript graph never got the same treatment, so
 * `import.meta.glob` expansion shipped its own scanner and reintroduced the
 * exact same blindness. Route this module through every new JS-side text walk
 * rather than writing a second one.
 *
 * Offsets are JavaScript string indices (UTF-16 code units). The Rust scanner
 * works in UTF-8 byte offsets. The two never exchange offsets — they exchange
 * the *results* of scanning — so the units do not need to agree.
 */

const KEYWORDS_BEFORE_REGEX = new Set([
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

/**
 * Index the code regions of `source`.
 *
 * Returns an object whose `isCode(offset)` reports whether that offset falls in
 * code rather than in string, template text, comment, or regex-literal content.
 * Interpolated expressions inside a template literal are code and survive.
 */
export function createCodeIndex(source) {
  const code = new Uint8Array(source.length)
  maskRange(source, 0, source.length, code)
  return {
    isCode(offset) {
      return offset >= 0 && offset < code.length && code[offset] === 1
    },
  }
}

/**
 * Every offset in `source` where `marker` occurs in code.
 *
 * This is the JS mirror of the Rust `find` + `is_code_offset` loop: occurrences
 * inside strings, comments, template text, and regex literals are skipped.
 */
export function findInCode(source, marker) {
  const index = createCodeIndex(source)
  const found = []
  let cursor = source.indexOf(marker)
  while (cursor >= 0) {
    if (index.isCode(cursor)) found.push(cursor)
    cursor = source.indexOf(marker, cursor + marker.length)
  }
  return found
}

/**
 * Whether `source` contains something that opens a JSX element.
 *
 * The port of `looks_like_jsx_at` in `crates/ruvyxa_bundler/src/ast.rs`: a `<`
 * in code position followed by `>`, `/`, or a letter. Asked of the mask rather
 * than the raw text, so a `<` inside a string, a comment, or a regular
 * expression is not an element.
 *
 * This only decides the dialect for the `.js` family, where JSX is common in
 * the ecosystem and nothing in the extension says either way. A `.ts` file is
 * never JSX and a `.tsx` file always is — see the `parserDialect` section of
 * `tests/fixtures/module-kind-conformance.json`, which holds this graph and the
 * Rust one to the same answers.
 *
 * A comparison such as `a<b` reads as JSX here and is still parsed correctly,
 * because JSX can only begin where a value is expected.
 */
export function containsJsx(source) {
  // Asked before the transform cache is consulted, so it runs on every module
  // of the `.js` family on every compile — including the ones that hit. The
  // Rust side gets this fact free from the scan it already runs; here it costs
  // its own walk, so the cheap disqualifier comes first and the walk only
  // happens for a source that could answer yes.
  if (!source.includes('<')) return false
  const index = createCodeIndex(source)
  for (let at = source.indexOf('<'); at >= 0; at = source.indexOf('<', at + 1)) {
    if (!index.isCode(at)) continue
    const next = source[at + 1]
    if (next === '>' || next === '/' || (next !== undefined && /[A-Za-z]/.test(next))) return true
  }
  return false
}

/**
 * Blank everything that is not code, preserving offsets and line structure.
 *
 * The JavaScript mirror of `ast::masked_code` in
 * `crates/ruvyxa_bundler/src/ast.rs`: the result has the same length as
 * `source`, every `\n` stays where it was, and every other non-code character
 * becomes a space. That is what lets a caller find a position in the mask and
 * slice the value out of the raw source — the pattern the Rust side settled on
 * after reading raw source silently disabled two guards.
 *
 * `compiler.mjs` used to carry its own copy of this walk. That copy had no
 * template-interpolation state, so a backtick inside a string inside a `${…}`
 * ended the template early and every `import` after it in the file stopped
 * being seen as code — the module was dropped from the bundle with no
 * diagnostic. Interpolations are code here, as they already were for
 * `createCodeIndex`.
 *
 * `options` keeps the *text* of module specifiers that would otherwise be
 * blanked, because the rewriters have to read the specifier they are replacing:
 *
 * - `preserveImportExportSpecifiers` — a literal after `from` or `import`
 * - `preserveImportCallSpecifiers` — a literal inside `import(`
 * - `preserveRequireCallSpecifiers` — a literal inside `require(`
 */
export function maskNonCode(source, options = {}) {
  const code = new Uint8Array(source.length)
  const literals = []
  maskRange(source, 0, source.length, code, literals)
  for (const [start, end] of preservedLiterals(source, literals, options)) {
    for (let index = start; index < end; index += 1) code[index] = 1
  }
  const output = []
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index]
    output.push(code[index] === 1 || character === '\n' ? character : ' ')
  }
  return output.join('')
}

/** Bytes preceding a quote that decide whether it opens a module specifier. */
const SPECIFIER_LOOKBACK = 32

/** The string literals `maskNonCode` was asked to leave readable. */
function preservedLiterals(source, literals, options) {
  const patterns = [
    [options.preserveImportExportSpecifiers === true, /\b(?:from|import)\s*$/],
    [options.preserveImportCallSpecifiers === true, /\bimport\s*\(\s*$/],
    [options.preserveRequireCallSpecifiers === true, /\brequire\s*\(\s*$/],
  ].filter(([enabled]) => enabled)
  if (patterns.length === 0) return []
  return literals.filter(([start]) => {
    const preceding = source.slice(Math.max(0, start - SPECIFIER_LOOKBACK), start)
    return patterns.some(([, pattern]) => pattern.test(preceding))
  })
}

/**
 * Offset just past the module's directive prologue.
 *
 * Generated top-level statements must be inserted here. Not at the very start,
 * because `'use client'` is only a directive while it is the first statement in
 * the module — anything placed above it silently demotes it to a plain string
 * expression and the whole server/client boundary check stops seeing it. Not at
 * the end either, because the linker rewrites imports into `const` bindings at
 * their original position rather than hoisting them, so a trailing import is in
 * the temporal dead zone for every earlier use.
 *
 * Mirrors `reference_manifest::directive_prologue_end` on the Rust side.
 */
export function directivePrologueEnd(source) {
  let offset = source.startsWith('﻿') ? 1 : 0
  for (;;) {
    const afterTrivia = skipLeadingTrivia(source, offset)
    const quote = source[afterTrivia]
    if (quote !== "'" && quote !== '"') return offset
    const bodyStart = afterTrivia + 1
    const end = source.indexOf(quote, bodyStart)
    if (end < 0) return offset
    // A raw newline means this was never a directive string.
    if (source.slice(bodyStart, end).includes('\n')) return offset
    let next = end + 1
    while (source[next] === ' ' || source[next] === '\t') next += 1
    offset = source[next] === ';' ? next + 1 : next
  }
}

function skipLeadingTrivia(source, start) {
  let index = start
  for (;;) {
    while (index < source.length && /\s/.test(source[index])) index += 1
    if (source[index] === '/' && source[index + 1] === '/') {
      const newline = source.indexOf('\n', index + 2)
      if (newline < 0) return source.length
      index = newline + 1
      continue
    }
    if (source[index] === '/' && source[index + 1] === '*') {
      const close = source.indexOf('*/', index + 2)
      if (close < 0) return source.length
      index = close + 2
      continue
    }
    return index
  }
}

/**
 * Mark the code positions of `source[start..end]`. Mirrors `ast.rs::mask_range`.
 *
 * `literals`, when given, collects the `[start, end)` of every *closed* string
 * literal encountered. Only `maskNonCode` needs them, to decide which module
 * specifiers to leave readable; `createCodeIndex` omits the argument.
 */
function maskRange(source, start, end, code, literals) {
  let index = start
  let previousSignificant = -1
  while (index < end) {
    if (isCommentStart(source, index, end)) {
      index = skipComment(source, index, end)
      continue
    }
    if (source[index] === '`') {
      const { after, interpolations } = templateLiteral(source, index, end)
      for (const [codeStart, codeEnd] of interpolations) {
        maskRange(source, codeStart, codeEnd, code, literals)
      }
      previousSignificant = index
      index = after
      continue
    }
    if (source[index] === '"' || source[index] === "'") {
      const quote = index
      index = skipString(source, index, end)
      // An unterminated quote resumes at `quote + 1` and names no literal.
      if (literals && index > quote + 1) literals.push([quote, index])
      previousSignificant = quote
      continue
    }
    if (source[index] === '/' && regexCanStart(source, previousSignificant)) {
      const slash = index
      index = skipRegexLiteral(source, index, end)
      previousSignificant = slash
      continue
    }
    code[index] = 1
    if (!/\s/.test(source[index])) previousSignificant = index
    index += 1
  }
}

function isCommentStart(source, index, end) {
  return (
    source[index] === '/' &&
    index + 1 < end &&
    (source[index + 1] === '/' || source[index + 1] === '*')
  )
}

function skipComment(source, start, end) {
  if (source[start + 1] === '/') {
    const newline = source.indexOf('\n', start + 2)
    return newline < 0 || newline >= end ? end : newline + 1
  }
  let index = start + 2
  while (index + 1 < end) {
    if (source[index] === '*' && source[index + 1] === '/') return index + 2
    index += 1
  }
  return end
}

/**
 * Walk a template literal from its opening backtick.
 *
 * Returns the index past the closing backtick plus the code ranges of each
 * `${ … }` interpolation, so callers scan those as code instead of treating the
 * whole literal as opaque text.
 */
function templateLiteral(source, start, end) {
  let index = start + 1
  const interpolations = []
  while (index < end) {
    const character = source[index]
    if (character === '\\') {
      index = Math.min(index + 2, end)
    } else if (character === '`') {
      return { after: index + 1, interpolations }
    } else if (character === '$' && source[index + 1] === '{') {
      const codeStart = index + 2
      const codeEnd = interpolationEnd(source, codeStart, end)
      interpolations.push([codeStart, codeEnd])
      index = Math.min(codeEnd + 1, end)
    } else {
      index += 1
    }
  }
  return { after: end, interpolations }
}

/**
 * Index of the `}` closing an interpolation whose code begins at `start`.
 *
 * Braces inside nested strings, templates, and comments do not count, or a
 * literal such as `` `${obj["}"]}` `` would end the interpolation early and
 * desynchronize the rest of the scan.
 */
function interpolationEnd(source, start, end) {
  let index = start
  let depth = 1
  // Tracked for the same reason the outer scan tracks it: `/` is a regex only
  // where a value is expected, and the answer depends on the token before it.
  let previousSignificant = -1
  while (index < end) {
    if (isCommentStart(source, index, end)) {
      index = skipComment(source, index, end)
      continue
    }
    const character = source[index]
    if (character === '`') {
      previousSignificant = index
      index = templateLiteral(source, index, end).after
    } else if (character === '"' || character === "'") {
      previousSignificant = index
      index = skipString(source, index, end)
    } else if (character === '/' && regexCanStart(source, previousSignificant)) {
      // An interpolation is code, so it can hold a regex — and a regex can hold
      // a quote. Without this branch the `'` in
      // `` `'${value.replace(/'/g, "''")}'` `` opened a string that ran to the
      // next quote, and every comment and literal after it in the file was read
      // inside out: comment text survived as code and code was masked away. A
      // linker reading that mask copied `export { … }` through verbatim into a
      // module wrapper, and the bundle did not parse. `js-yaml` ships exactly
      // this line.
      previousSignificant = index
      index = skipRegexLiteral(source, index, end)
    } else if (character === '{') {
      depth += 1
      previousSignificant = index
      index += 1
    } else if (character === '}') {
      depth -= 1
      if (depth === 0) return index
      previousSignificant = index
      index += 1
    } else {
      if (!/\s/.test(character)) previousSignificant = index
      index += 1
    }
  }
  return end
}

/**
 * Skip a `'`/`"` literal, giving up at the end of its line.
 *
 * A JavaScript string cannot contain a raw newline, so a quote with no closing
 * partner on its own line was never a delimiter: it is an apostrophe in prose
 * or in JSX text — `React's`, `<p>don't</p>`. Running to the next quote
 * anywhere in the file is what desynchronizes the scan, and the cost is silent.
 * Resuming just past the opening quote keeps a stray apostrophe's blast radius
 * to its own line.
 */
function skipString(source, start, end) {
  const quote = source[start]
  let index = start + 1
  while (index < end) {
    const character = source[index]
    if (character === '\n') break
    // A backslash escapes what follows, a line terminator included: `\` at the
    // end of a line continues the literal onto the next one, and `\r\n` is one
    // terminator rather than two. Refusing that walked the string's *text* as
    // code, which invents facts — a continued line holding `import` became an
    // edge on a module that does not exist. Only an unescaped newline ends the
    // search, which is what keeps a stray apostrophe bounded.
    if (character === '\\' && index + 1 < end)
      index += source[index + 1] === '\r' && source[index + 2] === '\n' ? 3 : 2
    else if (character === quote) return index + 1
    else index += 1
  }
  return start + 1
}

/**
 * Whether a `/` at this position opens a regex literal rather than a division.
 *
 * A regex may only appear where a value is expected. When the preceding token
 * could end a value (identifier, number, string, closing bracket) the slash is
 * division. Keywords such as `return` are value-expected positions.
 */
function regexCanStart(source, previousSignificant) {
  if (previousSignificant < 0) return true
  const character = source[previousSignificant]
  if (')]}\'"`'.includes(character)) return false
  if (isIdentContinue(character)) return previousTokenIsKeyword(source, previousSignificant)
  // JavaScript identifiers are Unicode and this walk is ASCII, so a non-ASCII
  // character standing where a token ends is the tail of one: `café / 2` is a
  // division. Reading it as a regular expression blanked everything up to the
  // next `/` on that line out of every scan built on this walk, and a minified
  // dependency is one long line, so the newline that stops a runaway literal
  // never arrives. Whitespace never reaches here — the walks test `\s`, which
  // covers the non-ASCII kinds — so what is left is identifier text.
  if (character > '\x7f') return false
  return true
}

function previousTokenIsKeyword(source, end) {
  let start = end + 1
  while (start > 0 && isIdentContinue(source[start - 1])) start -= 1
  return KEYWORDS_BEFORE_REGEX.has(source.slice(start, end + 1))
}

/**
 * Skip past a regular expression literal, returning the index after it.
 *
 * Quotes and slashes inside a character class (`/[/"']/`) are literal, so the
 * class state has to be tracked or the literal ends in the wrong place.
 */
function skipRegexLiteral(source, start, end) {
  let index = start + 1
  let insideCharacterClass = false
  while (index < end) {
    const character = source[index]
    if (character === '\\') {
      index = Math.min(index + 2, end)
    } else if (character === '[') {
      insideCharacterClass = true
      index += 1
    } else if (character === ']' && insideCharacterClass) {
      insideCharacterClass = false
      index += 1
    } else if (character === '\n') {
      // An unterminated literal was a division after all. Stop here so the rest
      // of the line is still scanned normally.
      return index
    } else if (character === '/' && !insideCharacterClass) {
      index += 1
      break
    } else {
      index += 1
    }
  }
  // Trailing flags (`/x/gi`) are part of the literal, not a new identifier.
  while (index < end && isIdentContinue(source[index])) index += 1
  return index
}

function isIdentContinue(character) {
  return character !== undefined && /[\w$]/.test(character)
}
