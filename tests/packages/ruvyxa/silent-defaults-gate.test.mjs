/**
 * The rule `check-silent-defaults.mjs` applies, tested against snippets.
 *
 * The check had no test, and three spellings passed it for as long as it
 * existed: `unwrap_or(value)`, an `unwrap_or_else` whose error is *named*
 * rather than written `_`, and a chain whose combinator landed past the
 * statement window. Two of those had no live instance in the repository, so
 * running the check proved nothing about them — which is the shape this
 * programme keeps finding, a gate that passes while the thing it names walks
 * past it.
 */

import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { fabricatedSites, swallowedReads } from '../../../scripts/check-silent-defaults.mjs'

/** The line numbers reported for one snippet. */
const sites = (source, options) => fabricatedSites(source, options).map((site) => site.line)

describe('what counts as inventing a value from a failed read', () => {
  it('reports all four spellings of the combinator', () => {
    const source = [
      'fn a(p: &Path) -> String { std::fs::read_to_string(p).unwrap_or_default() }',
      'fn b(p: &Path) -> String { std::fs::read_to_string(p).unwrap_or_else(|_| String::new()) }',
      'fn c(p: &Path) -> String { std::fs::read_to_string(p).unwrap_or_else(|error| String::new()) }',
      'fn d(t: &str) -> u64 { t.parse::<u64>().unwrap_or(7) }',
    ].join('\n')

    assert.deepEqual(
      sites(source),
      [1, 2, 3, 4],
      'each of the four is a failed read answered with a value the caller cannot tell apart',
    )
  })

  it('reads a chain that rustfmt wrapped across lines as one statement', () => {
    const source = [
      'fn wrapped(raw: &str) -> u64 {',
      '    raw.trim()',
      '        .parse::<u64>()',
      '        .unwrap_or(0)',
      '}',
    ].join('\n')
    // Reported at the line carrying the fallible call, not the line the chain
    // starts on: `raw.trim()` is not itself a read.
    assert.deepEqual(sites(source), [3])
  })

  /**
   * The reason the window is four lines and the statement is split on `;`.
   * `Option::unwrap_or` is ordinary and everywhere; pairing one with an
   * unrelated read on a neighbouring line reports a site that is not one.
   */
  it('does not pair a read with an Option default in a different statement', () => {
    const source = [
      'fn unrelated(file: &Path, root: &Path) -> Result<()> {',
      '    let source = std::fs::read_to_string(file)?;',
      '    let base = file.parent().unwrap_or(root);',
      '    Ok(())',
      '}',
    ].join('\n')
    assert.deepEqual(sites(source), [])
  })

  it('does not report a closure that panics, which reports the failure', () => {
    const source =
      'fn loud(p: &Path) -> String {\n' +
      '    std::fs::read_to_string(p).unwrap_or_else(|error| panic!("read {p:?}: {error}"))\n' +
      '}'
    assert.deepEqual(sites(source), [])
  })

  it('does not report `.ok()`, which hands the caller an honest absence', () => {
    const source = 'fn maybe(p: &Path) -> Option<String> { std::fs::read_to_string(p).ok() }'
    assert.deepEqual(sites(source), [])
  })

  it('stops at the test module, where fabricating is the point', () => {
    const source = [
      'fn real(p: &Path) -> String { std::fs::read_to_string(p).unwrap_or_default() }',
      '',
      '#[cfg(test)]',
      'mod tests {',
      '    fn fixture(p: &Path) -> String { std::fs::read_to_string(p).unwrap_or_default() }',
      '}',
    ].join('\n')
    assert.deepEqual(sites(source), [1])
  })

  it('honours a reviewed entry, and says which one it used', () => {
    const source = 'fn a(t: &str) -> u64 { t.parse::<u64>().unwrap_or(u64::MAX) }'
    const allowed = [{ file: 'x.rs', contains: 'unwrap_or(u64::MAX)', reason: 'past the end' }]
    const usedEntries = []

    assert.deepEqual(
      sites(source, { allowed, file: 'x.rs', onAllowed: (entry) => usedEntries.push(entry) }),
      [],
    )
    assert.equal(usedEntries.length, 1)

    // The same entry against a different file is not the same site.
    assert.deepEqual(sites(source, { allowed, file: 'other.rs' }), [1])
  })
})

describe('what counts as swallowing a failed read, on the JavaScript side', () => {
  const lines = (source, options) => swallowedReads(source, options).map((site) => site.line)

  it('reports an empty catch around a read', () => {
    const source = [
      'async function load(file) {',
      '  try {',
      '    return JSON.parse(await readFile(file, "utf8"))',
      '  } catch {}',
      '  return fallback',
      '}',
    ].join('\n')
    assert.deepEqual(lines(source), [2])
  })

  it('reports it with a named binding too, which reads as more deliberate', () => {
    const source = 'try {\n  readFileSync(p)\n} catch (error) {\n}'
    assert.deepEqual(lines(source), [1])
  })

  it('does not report a catch that says something', () => {
    const withLog = 'try {\n  readFileSync(p)\n} catch (error) {\n  warn(error)\n}'
    const withRethrow = 'try {\n  readFileSync(p)\n} catch (error) {\n  throw error\n}'
    assert.deepEqual(lines(withLog), [])
    assert.deepEqual(lines(withRethrow), [])
  })

  /**
   * The reason this pass is narrower than the Rust one. A `catch` is legitimate
   * far more often in JavaScript, so a `try` that reads nothing is not this
   * check's business however empty its handler is.
   */
  it('does not report an empty catch around something that is not a read', () => {
    const source = 'try {\n  maybeUnsupportedApi()\n} catch {}'
    assert.deepEqual(lines(source), [])
  })

  it('judges the handler by what runs, not by what it explains', () => {
    const source = 'try {\n  readFileSync(p)\n} catch {\n  // absent is fine here\n}'
    assert.deepEqual(
      lines(source),
      [1],
      'a comment explaining why the error is dropped still drops it',
    )
  })

  it('honours a reviewed entry', () => {
    const source = 'try {\n  readFileSync(cache)\n} catch {}'
    const allowed = [{ file: 'a.mjs', contains: 'readFileSync(cache)', reason: 'absent is legal' }]
    assert.deepEqual(lines(source, { allowed, file: 'a.mjs' }), [])
    assert.deepEqual(lines(source, { allowed, file: 'b.mjs' }), [1])
  })
})
