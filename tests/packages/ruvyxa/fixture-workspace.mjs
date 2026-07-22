import { cp, mkdir, mkdtemp, readFile, realpath, rm } from 'node:fs/promises'
import { createRequire } from 'node:module'
import os from 'node:os'
import path from 'node:path'

/**
 * Create an OS-temp project parent with real package copies for module lookup.
 * Test fixtures can live below it without relying on repository-local temp
 * directories or junctions that are unsafe to clean after forced termination.
 */
export async function createFixtureWorkspace(prefix, dependencyRoot) {
  const workspace = await mkdtemp(path.join(os.tmpdir(), prefix))
  try {
    const requireFromProject = createRequire(path.join(dependencyRoot, 'package.json'))
    const copied = new Set()
    await copyPackageTree('react', requireFromProject, workspace, copied)
    await copyPackageTree('react-dom', requireFromProject, workspace, copied)
    // Windows may expose the temp root through an 8.3 alias (NRITRO~1) while
    // Sass canonicalizes loaded URLs to the long path. Returning the canonical
    // root keeps project-containment checks and dependency metadata consistent.
    return await realpath(workspace)
  } catch (error) {
    await rm(workspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
    throw error
  }
}

async function copyPackageTree(packageName, requireFromParent, workspace, copied) {
  if (copied.has(packageName)) return
  copied.add(packageName)
  const packageJson = requireFromParent.resolve(`${packageName}/package.json`)
  const source = path.dirname(packageJson)
  const destination = path.join(workspace, 'node_modules', packageName)
  await mkdir(path.dirname(destination), { recursive: true })
  await cp(source, destination, { recursive: true, dereference: true })

  const manifest = JSON.parse(await readFile(packageJson, 'utf8'))
  const requireFromPackage = createRequire(packageJson)
  for (const dependency of Object.keys(manifest.dependencies ?? {})) {
    await copyPackageTree(dependency, requireFromPackage, workspace, copied)
  }
}
