/**
 * Code-point string ordering, shared by everything that sorts into an artifact
 * or an identity.
 *
 * `String.prototype.localeCompare` orders by the host's ICU locale, so it sorts
 * `a.ts` before `B.ts` where a scalar comparison — and the Rust side's `String`
 * ordering — puts `B.ts` first, and it can differ between two machines running
 * the same build. Anything whose output is a cache key, a content fingerprint,
 * or a file the build writes has to come out identical everywhere, so those
 * sites compare code points instead.
 *
 * **Code points, not code units.** This function used to be `compareCodeUnits`
 * and used `<`, which orders by UTF-16 code units, while Rust's `String: Ord`
 * compares UTF-8 bytes — which is code-point order. The two disagree wherever a
 * surrogate pair meets U+E000–U+FFFF: for `"x"` against `"\u{1F600}x"`,
 * `<` says the first is greater and Rust says it is smaller. The sites this
 * feeds are cache keys, content fingerprints, `import.meta.glob` key order and
 * emitted bytes, so the two module graphs would have written different bytes for
 * one project. It needs both character classes in one sorted set of names to
 * fire, which is why it never did; the point of this module is that ordering is
 * a contract, and the contract was documented as stronger than it was.
 * `tests/fixtures/ordering-conformance.json` is replayed by both languages.
 *
 * This lives in one module because the rule had been written out three times as
 * a private helper (`compareBySlashedPath` here, `comparePatterns` in
 * `paths.mjs`, `compareStable` in `../src/plugins.ts`) while nine other call
 * sites still reached for `localeCompare` — among them the Flight cache key,
 * the project input fingerprint, and the config cache's env key. The Oxlint
 * `no-restricted-properties` entry that bans `localeCompare` is what keeps the
 * next one from appearing; this module is what it points at.
 */

/** True for a string that holds no surrogate, where the two orders agree. */
const HAS_SURROGATE = /[\uD800-\uDFFF]/

/** Order two strings by their Unicode code points, as Rust orders a `String`. */
export function compareCodePoints(left, right) {
  if (left === right) return 0

  // Below the surrogate range the code-unit comparison already answers by code
  // point, and it is the comparison every call site here actually makes — file
  // names, module ids, environment keys. Paying for an iterator on all of them
  // to serve a case that needs an astral character is the wrong trade.
  if (!HAS_SURROGATE.test(left) && !HAS_SURROGATE.test(right)) {
    return left < right ? -1 : 1
  }

  // Spread iterates by code point, pairing a surrogate pair into one scalar.
  const leftPoints = [...left]
  const rightPoints = [...right]
  const shared = Math.min(leftPoints.length, rightPoints.length)
  for (let index = 0; index < shared; index += 1) {
    const a = leftPoints[index].codePointAt(0)
    const b = rightPoints[index].codePointAt(0)
    if (a !== b) return a < b ? -1 : 1
  }
  // A prefix sorts before the string that extends it, in both languages.
  if (leftPoints.length === rightPoints.length) return 0
  return leftPoints.length < rightPoints.length ? -1 : 1
}

/**
 * Order `[key, value]` pairs by key, for the `Object.fromEntries(...sort())`
 * shape that turns a map into a stable object literal or key string.
 */
export function compareEntryKeys([left], [right]) {
  return compareCodePoints(left, right)
}
