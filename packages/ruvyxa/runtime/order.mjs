/**
 * Code-unit string ordering, shared by everything that sorts into an artifact
 * or an identity.
 *
 * `String.prototype.localeCompare` orders by the host's ICU locale, so it sorts
 * `a.ts` before `B.ts` where a code-unit comparison — and the Rust side's
 * `String` ordering — puts `B.ts` first, and it can differ between two machines
 * running the same build. Anything whose output is a cache key, a content
 * fingerprint, or a file the build writes has to come out identical everywhere,
 * so those sites compare code units instead.
 *
 * This lives in one module because the rule had been written out three times as
 * a private helper (`compareBySlashedPath` here, `comparePatterns` in
 * `paths.mjs`, `compareStable` in `../src/plugins.ts`) while nine other call
 * sites still reached for `localeCompare` — among them the Flight cache key,
 * the project input fingerprint, and the config cache's env key. The Oxlint
 * `no-restricted-properties` entry that bans `localeCompare` is what keeps the
 * next one from appearing; this module is what it points at.
 */

/** Order two strings by their UTF-16 code units. */
export function compareCodeUnits(left, right) {
  if (left < right) return -1
  if (left > right) return 1
  return 0
}

/**
 * Order `[key, value]` pairs by key, for the `Object.fromEntries(...sort())`
 * shape that turns a map into a stable object literal or key string.
 */
export function compareEntryKeys([left], [right]) {
  return compareCodeUnits(left, right)
}
