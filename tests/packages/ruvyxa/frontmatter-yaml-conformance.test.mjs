/**
 * Markdown frontmatter, parsed by the JavaScript content compiler.
 *
 * Two YAML libraries read frontmatter: `serde-saphyr` in
 * `crates/ruvyxa_bundler/src/content.rs` and the `yaml` package in
 * `runtime/compiler.mjs`. Their defaults disagree on `yes`/`no`, `1_000`, `<<`,
 * duplicate keys and `.inf`, so the Rust half pins options to the core schema
 * this half uses. The table is `tests/fixtures/frontmatter-yaml-conformance.json`
 * and it is replayed from Rust too, by
 * `content::tests::parses_the_shared_frontmatter_yaml_conformance_cases`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { compileContentSource } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const fixture = JSON.parse(
  readFileSync(
    path.join(workspaceRoot, 'tests/fixtures/frontmatter-yaml-conformance.json'),
    'utf8',
  ),
)

const declaration = 'export const frontmatter = '

async function compiledFrontmatter(yaml) {
  const { source } = await compileContentSource(
    `---\n${yaml}---\n# Page\n`,
    path.join(workspaceRoot, 'page.md'),
    workspaceRoot,
  )
  const line = source.split('\n').find((candidate) => candidate.startsWith(declaration))
  assert.ok(line, 'the compiled module declares frontmatter')
  return JSON.parse(line.slice(declaration.length).replace(/;$/, ''))
}

describe('frontmatter YAML conformance', () => {
  assert.ok(fixture.cases.length > 0, 'the fixture must carry cases')
  for (const testCase of fixture.cases) {
    it(testCase.name, async () => {
      if (testCase.error) {
        await assert.rejects(compiledFrontmatter(testCase.yaml), /RUV1312/)
      } else {
        assert.deepEqual(await compiledFrontmatter(testCase.yaml), testCase.frontmatter)
      }
    })
  }
})
