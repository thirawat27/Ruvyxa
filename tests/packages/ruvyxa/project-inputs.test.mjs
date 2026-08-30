import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'

import {
  compileBundleIfChanged,
  compileBundleWithMetadata,
} from '../../../packages/ruvyxa/runtime/compiler.mjs'

/**
 * What counts as *project* input, as opposed to anything the compile read.
 *
 * `is_project_local` in `crates/ruvyxa_bundler/src/resolver.rs` answers "is this
 * project source" — under the root *and* not under `node_modules`. This graph
 * asked only the first half, so a browser bundle, which inlines its packages
 * because a browser has no resolver, put every file of every dependency into
 * `inputs` and into the `dependencyHash`. `inputs` is what the dev-server
 * watcher watches, so a real application put thousands of `node_modules` paths
 * on that list and paid a file descriptor and a wake-up per rebuild for each.
 *
 * The exclusion is only safe because nothing that must invalidate depends on it:
 * `PROJECT_MANIFEST_FILES` still feeds the fingerprint, so an install changes it,
 * and bundle reuse is keyed on `readFiles` — deliberately wider than
 * `fingerprintInputs` — so a dependency edited in place by `patch-package` or a
 * linked workspace still forces a recompile. Both are asserted below.
 */
// `realpath`, because one assertion below compares an absolute path this file
// built against an absolute path the compiler reported — and the compiler
// canonicalises where `path.resolve` does not. On macOS `os.tmpdir()` is
// `/var/folders/...` and `/var` is a symlink to `/private/var`, so the two
// spellings of the same file never matched and `readFiles` looked as though it
// had dropped the dependency. macOS only: Linux hands back a real `/tmp`, and
// on Windows the compiler's own `\\?\` stripping already lines the two up.
const workspace = await realpath(await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-project-inputs-')))
after(() => rm(workspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

/** A project whose only import is a package installed inside its own root. */
async function createProject(name) {
  const root = path.join(workspace, name)
  const dependency = path.join(root, 'node_modules', 'tiny')
  await mkdir(path.join(root, 'src'), { recursive: true })
  await mkdir(dependency, { recursive: true })
  await writeFile(path.join(root, 'package.json'), JSON.stringify({ name, type: 'module' }))
  await writeFile(
    path.join(dependency, 'package.json'),
    JSON.stringify({ name: 'tiny', version: '1.0.0', type: 'module', main: 'index.js' }),
  )
  await writeFile(path.join(dependency, 'index.js'), 'export const n = 1\n')
  await writeFile(
    path.join(root, 'src', 'entry.ts'),
    "export { n } from 'tiny'\nexport const local = 2\n",
  )
  return {
    root,
    dependencyFile: path.join(dependency, 'index.js'),
    outfile: path.join(root, 'out', 'bundle.mjs'),
  }
}

function bundleOptions({ root, outfile }) {
  return {
    projectRoot: root,
    entrySource: `export * from ${JSON.stringify(`${root.replaceAll('\\', '/')}/src/entry.ts`)}`,
    sourcefile: 'ruvyxa:inputs-entry.ts',
    outfile,
    platform: 'browser',
    sourceMap: false,
  }
}

function nodeModulesEntries(paths) {
  return paths.filter((entry) => entry.split('/').includes('node_modules'))
}

describe('project inputs of a browser bundle', () => {
  it('reports no node_modules path in inputs or fingerprintInputs', async () => {
    const project = await createProject('excluded')
    const bundle = await compileBundleWithMetadata(bundleOptions(project))

    // The dependency really was walked into the bundle — otherwise the
    // assertions below would pass on a bundle that never reached it.
    assert.match(await readFile(project.outfile, 'utf8'), /const n = 1;/)
    assert.deepEqual(nodeModulesEntries(bundle.inputs), [])
    assert.deepEqual(nodeModulesEntries(bundle.fingerprintInputs), [])

    // …and the project's own source is still there, so the exclusion narrowed
    // the list rather than emptying it.
    assert.ok(bundle.inputs.includes('src/entry.ts'))
    assert.ok(bundle.fingerprintInputs.includes('src/entry.ts'))
    // The manifest is what still invalidates the fingerprint when an install
    // changes which files a bare specifier resolves to.
    assert.ok(bundle.fingerprintInputs.includes('package.json'))
  })

  it('still records the dependency among the files the compile read', async () => {
    const project = await createProject('read-files')
    const bundle = await compileBundleWithMetadata(bundleOptions(project))

    assert.ok(
      bundle.readFiles.includes(path.resolve(project.dependencyFile)),
      'readFiles answers "could these bytes still be produced" and must stay wider than the project fingerprint',
    )
  })

  it('recompiles when a dependency is edited in place', async () => {
    // The regression the exclusion could have caused: `patch-package`, or a
    // linked workspace built into `node_modules`, changes a file that no longer
    // feeds `dependencyHash`. Reuse is keyed on `readFiles`, so it still sees it.
    const project = await createProject('patched')
    const first = await compileBundleIfChanged(bundleOptions(project))
    assert.match(await readFile(project.outfile, 'utf8'), /const n = 1;/)

    await writeFile(project.dependencyFile, 'export const n = 99\n')
    const second = await compileBundleIfChanged(bundleOptions(project))

    assert.match(await readFile(project.outfile, 'utf8'), /const n = 99;/)
    assert.equal(
      second.dependencyHash,
      first.dependencyHash,
      'no project file changed, so the project fingerprint is deliberately unmoved',
    )
  })
})
