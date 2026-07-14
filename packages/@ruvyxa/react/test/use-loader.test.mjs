import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

const source = await readFile(new URL('../src/use-loader.ts', import.meta.url), 'utf8')

describe('useRuvyxaLoader request lifecycle', () => {
  it('keeps inline loaders out of automatic refetch dependencies', () => {
    assert.match(source, /const loaderRef = useRef\(loader\)/)
    assert.match(source, /loaderRef\s*\.current\(\)/)
    assert.match(source, /\}, \[enabled\]\)/)
    assert.doesNotMatch(source, /\}, \[enabled, loader\]\)/)
  })
})
