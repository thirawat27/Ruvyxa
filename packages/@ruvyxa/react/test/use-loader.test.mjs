import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'
import { format } from 'prettier'

const sourceFile = fileURLToPath(new URL('../src/use-loader.ts', import.meta.url))
const source = await readFile(sourceFile, 'utf8')

function assertLoaderLifecycleContract(candidate) {
  const code = candidate.replace(/\s+/g, ' ')

  assert.match(code, /const loaderRef = useRef\(loader\)/)
  assert.match(code, /loaderRef\s*\.\s*current\(\)/)
  assert.match(code, /\}, \[enabled\]\)/)
  assert.doesNotMatch(code, /\}, \[enabled, loader\]\)/)
}

describe('useRuvyxaLoader request lifecycle', () => {
  it('keeps inline loaders out of automatic refetch dependencies after formatting', async () => {
    assertLoaderLifecycleContract(source)
    assertLoaderLifecycleContract(await format(source, { filepath: sourceFile }))
  })
})
