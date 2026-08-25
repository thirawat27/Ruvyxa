/**
 * The JavaScript half of the source-scanner contract.
 *
 * `runtime/scanner.mjs` decides where code stops and text begins for every
 * reader on the JavaScript side; `crates/ruvyxa_bundler/src/ast.rs` answers the
 * same question for the Rust side and replays the same table. Neither parses,
 * so a construct one of them does not know desynchronizes it — and from there
 * the file is read inside out, with nothing downstream able to tell.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { maskNonCode } from '../../../packages/ruvyxa/runtime/scanner.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/source-scanner-conformance.json'), 'utf8'),
)

describe('the source scanner', () => {
  it('agrees with the shared table about what is code', () => {
    assert.ok(fixture.cases.length > 0, 'the fixture must carry cases')
    for (const testCase of fixture.cases) {
      const masked = maskNonCode(testCase.source, { preserveImportExportSpecifiers: true })
      assert.equal(
        masked.length,
        testCase.source.length,
        `${testCase.name}: a mask is the same length as its source, so offsets stay usable`,
      )
      for (const fragment of testCase.code) {
        assert.ok(
          masked.includes(fragment),
          `${testCase.name}: "${fragment}" must survive the mask — ${testCase.why}`,
        )
      }
      for (const fragment of testCase.text) {
        assert.ok(
          !masked.includes(fragment),
          `${testCase.name}: "${fragment}" is text and must be masked away — ${testCase.why}`,
        )
      }
    }
  })

  /**
   * The failure this whole file exists for, at the scale it actually happened.
   *
   * One unknown construct near the top of a file inverts everything below it.
   * A single-line assertion cannot show that; running the scan over a real
   * package and checking a statement 3,500 lines further down can.
   */
  it('stays synchronized across a whole real module', () => {
    const file = path.join(workspaceRoot, 'stress-lab/node_modules/js-yaml/dist/js-yaml.mjs')
    let source
    try {
      source = readFileSync(file, 'utf8')
    } catch {
      return // the stress project is not part of a published checkout
    }
    const masked = maskNonCode(source, { preserveImportExportSpecifiers: true })
    const sourceLines = source.split('\n')
    const maskedLines = masked.split('\n')
    const exportAt = sourceLines.findIndex((line) => line.trimStart().startsWith('export {'))
    assert.ok(exportAt > 0, 'the fixture module must carry a bare export list')
    assert.ok(
      maskedLines[exportAt].trimStart().startsWith('export {'),
      'an export thousands of lines below a tricky literal is still code',
    )
  })
})
