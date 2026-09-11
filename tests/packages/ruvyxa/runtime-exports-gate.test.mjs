/**
 * The rule `scripts/check-runtime-exports.mjs` applies, tested against snippets.
 *
 * `packages/ruvyxa/runtime/*.mjs` are `knip.json`'s entry points, so Knip
 * measures the rest of the workspace against their exports and can never
 * report one as unused. Eight names had already accumulated there and were
 * removed by hand; this gate is what makes the ninth fail a build instead.
 *
 * Both cases below are ones a plausible implementation gets wrong, and the
 * first of them is not hypothetical: the first draft of the gate searched the
 * head of each declaration for `, name =` to catch `export const a = 1, b = 2`,
 * and reported the default-valued *parameters* of two exported functions as
 * exported bindings. `entry-templates.mjs` supplies the other — it is a
 * generator, and its template literals contain `export const …` as output
 * text.
 */

import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { exportedNames, isTaggedPublic } from '../../../scripts/check-runtime-exports.mjs'

/** Just the names, in declaration order. */
const names = (source) => exportedNames(source).map((entry) => entry.name)

describe('what counts as a name a runtime module exports', () => {
  it('reads every declaration form', () => {
    const source = [
      "export const NAME = 'a'",
      'export let counter = 0',
      'export var legacy = 1',
      'export function plain() {}',
      'export async function waits() {}',
      'export function* generates() {}',
      'export async function* streams() {}',
      'export class Shape {}',
    ].join('\n')

    assert.deepEqual(names(source), [
      'NAME',
      'counter',
      'legacy',
      'plain',
      'waits',
      'generates',
      'streams',
      'Shape',
    ])
  })

  it('reads an export list, its aliases, and a Prettier-wrapped trailing comma', () => {
    const source = [
      "export { alpha, beta as gamma } from './other.mjs'",
      'export {',
      '  delta,',
      '}',
    ].join('\n')

    assert.deepEqual(names(source), ['alpha', 'gamma', 'delta'])
  })

  it('reads the second declarator of one statement, across a wrapped line', () => {
    const source = [
      'export const first = 1, second = 2',
      'export const third = 3,',
      '  fourth = 4',
    ].join('\n')

    assert.deepEqual(names(source), ['first', 'second', 'third', 'fourth'])
  })

  /**
   * The defect the first draft shipped with. A parameter list is bracket
   * depth, and a comma inside one is not a declarator: reading `namePrefix`
   * out of `metaSourceImports(importPaths, namePrefix = …)` reported two
   * exports that do not exist, and the fix has to be depth rather than a
   * narrower regex — the same shape appears in an array and an object literal.
   */
  it('does not read a default-valued parameter as an exported binding', () => {
    const source = [
      'export function metaSourceImports(importPaths, namePrefix = META_SOURCE_PREFIX) {',
      '  return [importPaths, namePrefix]',
      '}',
      'export const table = { first: 1, second: 2 }',
      'export const list = [one, two = 3]',
    ].join('\n')

    assert.deepEqual(names(source), ['metaSourceImports', 'table', 'list'])
  })

  /**
   * `entry-templates.mjs` and `output.rs` are the two generators of a route's
   * entry, and the JavaScript one holds its output in template literals. A
   * text walk that does not mask them declares whatever the generated module
   * declares.
   */
  it('does not read an export out of generated source text', () => {
    const source = [
      'export function entrySource(touched) {',
      '  return `',
      'export const linkedModules = [${touched}]',
      'export function boot() {}',
      '`',
      '}',
      "// export const commented = 'no'",
      '/* export const blockCommented = 1 */',
    ].join('\n')

    assert.deepEqual(names(source), ['entrySource'])
  })

  it('reports the line the declaration is on', () => {
    const source = ['', '/** doc */', "export const NAME = 'a'"].join('\n')
    assert.deepEqual(exportedNames(source), [{ name: 'NAME', line: 3 }])
  })
})

describe('the @public opt-out', () => {
  const tagged = [
    '/**',
    ' * Surface something outside reads.',
    ' * @public',
    ' */',
    'export const NAME = 1',
  ].join('\n')
  const untagged = ['/**', ' * Ordinary.', ' */', 'export const NAME = 1'].join('\n')

  it('honours the tag knip.json already excludes', () => {
    assert.equal(isTaggedPublic(tagged, 5), true)
    assert.equal(isTaggedPublic(untagged, 4), false)
  })

  it('does not carry a tag across an unrelated declaration', () => {
    const source = [
      '/**',
      ' * @public',
      ' */',
      'export const FIRST = 1',
      '',
      'export const SECOND = 2',
    ].join('\n')

    assert.equal(isTaggedPublic(source, 4), true)
    assert.equal(
      isTaggedPublic(source, 6),
      false,
      'a blank line and a declaration stand between SECOND and that comment',
    )
  })
})
