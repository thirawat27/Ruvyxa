import { existsSync } from 'node:fs'
import path from 'node:path'

/**
 * Test sources are compiled to `.test-build/<suite>/` before they run, so a
 * path built from `import.meta.dirname` sits one directory deeper than the
 * checked-in file it came from. Walking up to the workspace marker keeps
 * repository lookups identical whether a test runs from source or from the
 * compiled output.
 */
function findRepoRoot(): string {
  let current = import.meta.dirname
  while (true) {
    if (existsSync(path.join(current, 'pnpm-workspace.yaml'))) return current
    const parent = path.dirname(current)
    if (parent === current) {
      throw new Error('unable to locate the Ruvyxa workspace root from ' + import.meta.dirname)
    }
    current = parent
  }
}

export const repoRoot = findRepoRoot()

/** Resolve a path against the workspace root. */
export function repoPath(...segments: string[]): string {
  return path.join(repoRoot, ...segments)
}
