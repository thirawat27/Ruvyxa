/**
 * A package subpath may not leave the package, symlinks included.
 *
 * `isSafePackageRelativePath` rules out every *lexical* escape — `..`, an
 * absolute path, a backslash — and both JavaScript resolvers stopped there,
 * then did `path.join` + `existsSync`. A symlink is not lexical. The Rust
 * mirrors canonicalize for the existence probe anyway and reuse it for
 * containment, so the two module graphs answered the same import with different
 * files: the failure this repository's resolution rules exist to prevent.
 *
 * Skipped where the host refuses to create a symlink — unprivileged Windows
 * without Developer Mode — and said out loud rather than passing quietly,
 * because a silent skip on this platform has hidden a real defect in this
 * repository before.
 */

import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'

import { resolvePackageRelative } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const root = mkdtempSync(path.join(tmpdir(), 'ruvyxa-containment-'))
after(() => rmSync(root, { recursive: true, force: true }))

/** Whether this host lets an unprivileged process create a directory symlink. */
function canSymlink() {
  const probe = path.join(root, 'symlink-probe')
  const target = path.join(root, 'symlink-target')
  try {
    mkdirSync(target, { recursive: true })
    symlinkSync(target, probe, 'junction')
    return true
  } catch {
    return false
  }
}

const symlinksWork = canSymlink()

describe('a package subpath stays inside its package', () => {
  it('reports whether this host can exercise the symlink case at all', () => {
    // Not an assertion about the code: an assertion that the reader is told.
    // A test that silently does nothing is worse than one that is not there.
    assert.ok(typeof symlinksWork === 'boolean', `symlink support on this host: ${symlinksWork}`)
  })

  it('resolves an ordinary subpath that really is inside the package', () => {
    const pkgDir = path.join(root, 'inside', 'node_modules', 'pkg')
    mkdirSync(path.join(pkgDir, 'dist'), { recursive: true })
    writeFileSync(path.join(pkgDir, 'dist', 'index.js'), 'export default 1\n')

    assert.equal(
      resolvePackageRelative(pkgDir, 'dist/index.js'),
      path.resolve(pkgDir, 'dist', 'index.js'),
    )
  })

  it('refuses a subpath that leaves the package through a symlink', (t) => {
    if (!symlinksWork) {
      t.skip('this host does not permit creating symlinks')
      return
    }

    const base = path.join(root, 'escape')
    const pkgDir = path.join(base, 'node_modules', 'pkg')
    mkdirSync(pkgDir, { recursive: true })
    const outside = path.join(base, 'outside')
    mkdirSync(outside, { recursive: true })
    writeFileSync(path.join(outside, 'secret.js'), 'export default "secret"\n')

    // Lexically `./escape-hatch/secret.js` is a perfectly ordinary subpath.
    symlinkSync(outside, path.join(pkgDir, 'escape-hatch'), 'junction')

    assert.equal(
      resolvePackageRelative(pkgDir, 'escape-hatch/secret.js'),
      null,
      'a file reached only by leaving the package is not part of the package',
    )
  })

  it('does not mistake a sibling directory with a shared prefix for the package', () => {
    const base = path.join(root, 'prefix')
    const pkgDir = path.join(base, 'node_modules', 'pkg')
    mkdirSync(pkgDir, { recursive: true })
    const sibling = path.join(base, 'node_modules', 'pkg-extra')
    mkdirSync(sibling, { recursive: true })
    writeFileSync(path.join(sibling, 'index.js'), 'export default 2\n')

    // A string-prefix containment check would accept this; `path.relative`
    // does not, which is why the guard uses it.
    assert.equal(resolvePackageRelative(pkgDir, '../pkg-extra/index.js'), null)
  })
})
