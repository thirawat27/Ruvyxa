import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const runtimeDir = path.join(repoRoot, 'packages/ruvyxa/runtime')

test('worker local dependencies are packaged and included in prerender cache identity', async () => {
  const packageJson = JSON.parse(
    await readFile(path.join(repoRoot, 'packages/ruvyxa/package.json'), 'utf8'),
  )
  const artifactCacheSource = await readFile(
    path.join(repoRoot, 'crates/ruvyxa_cli/src/artifact_cache.rs'),
    'utf8',
  )
  const localDependencies = await localRuntimeGraph(['worker-pool.mjs'])

  for (const runtimeFile of localDependencies) {
    assert.ok(
      packageJson.files.includes(`runtime/${runtimeFile}`),
      `packages/ruvyxa/package.json must publish runtime/${runtimeFile}`,
    )
    assert.ok(
      artifactCacheSource.includes(`"${runtimeFile}"`),
      `prerender cache identity must include runtime/${runtimeFile}`,
    )
  }
})

async function localRuntimeGraph(entryFiles) {
  const pending = [...entryFiles]
  const visited = new Set()

  while (pending.length > 0) {
    const runtimeFile = pending.pop()
    if (visited.has(runtimeFile)) continue
    visited.add(runtimeFile)

    const sourcePath = path.join(runtimeDir, runtimeFile)
    const source = await readFile(sourcePath, 'utf8')
    const specifiers = [
      ...source.matchAll(/\b(?:import|export)\s+(?:[^'"]*?\s+from\s+)?['"](\.[^'"]+)['"]/g),
      ...source.matchAll(/\bimport\s*\(\s*['"](\.[^'"]+)['"]\s*\)/g),
    ]
    for (const match of specifiers) {
      const dependency = path.relative(runtimeDir, path.resolve(path.dirname(sourcePath), match[1]))
      assert.ok(
        dependency && dependency !== '..' && !dependency.startsWith(`..${path.sep}`),
        `runtime import escapes package/runtime: ${runtimeFile} -> ${match[1]}`,
      )
      pending.push(dependency.replaceAll(path.sep, '/'))
    }
  }

  return [...visited].sort()
}
