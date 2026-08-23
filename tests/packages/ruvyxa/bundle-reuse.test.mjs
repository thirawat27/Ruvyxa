import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'

import { compileBundleIfChanged } from '../../../packages/ruvyxa/runtime/compiler.mjs'

/**
 * What `compileBundleIfChanged` must and must not reuse.
 *
 * Skipping a compile is only safe if every reason the output could differ still
 * invalidates it, and the expensive part of the question is the part that is
 * easy to get wrong: a config bundle's inputs are overwhelmingly framework
 * modules living *outside* the project, so a cache keyed on the project's own
 * dependency fingerprint would look correct on every test that only edits
 * application code and serve a stale bundle to anyone developing the framework.
 *
 * Reuse is observed rather than inferred. After a compile the emitted file is
 * given a marker; a compile that actually ran rewrites the file and takes the
 * marker with it, so the marker surviving is proof that no compile happened.
 */
const MARKER = '\n// reuse-probe\n'

const workspace = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-bundle-reuse-'))
after(() => rm(workspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

/**
 * A project whose entry reaches one file inside it and one file outside it.
 *
 * The outside file arrives through an alias with `bundleAliasDependencies`,
 * which is how the real callers reach the framework: that setting is what pulls
 * a module into the emitted bytes instead of leaving it an external import, and
 * so it is also what makes that module's contents something reuse has to check.
 */
async function createProject(name) {
  const root = path.join(workspace, name)
  const outside = path.join(workspace, `${name}-vendor`)
  await mkdir(path.join(root, 'src'), { recursive: true })
  await mkdir(outside, { recursive: true })
  await writeFile(path.join(root, 'package.json'), JSON.stringify({ name, type: 'module' }))
  await writeFile(path.join(outside, 'helper.ts'), 'export const helper = 1\n')
  await writeFile(
    path.join(root, 'src', 'local.ts'),
    "import { helper } from 'vendor'\nexport default { local: helper }\n",
  )
  return { root, outside, outfile: path.join(root, 'out', 'bundle.mjs') }
}

function compile({ root, outside, outfile }) {
  return compileBundleIfChanged({
    projectRoot: root,
    entrySource: `export { default } from ${JSON.stringify(`${root.replaceAll('\\', '/')}/src/local.ts`)}`,
    sourcefile: 'ruvyxa:reuse-entry.ts',
    outfile,
    bundleAliasDependencies: true,
    aliases: { vendor: path.join(outside, 'helper.ts') },
    sourceMap: false,
  })
}

/** Mark the emitted bundle so a later compile is visible by erasing the mark. */
async function mark(outfile) {
  await writeFile(outfile, (await readFile(outfile, 'utf8')) + MARKER)
}

async function recompiled(outfile) {
  return !(await readFile(outfile, 'utf8')).includes(MARKER.trim())
}

describe('compiled bundle reuse', () => {
  it('reuses the previous compile when nothing it read has changed', async () => {
    const project = await createProject('unchanged')
    const first = await compile(project)
    await mark(project.outfile)

    const second = await compile(project)

    assert.equal(await recompiled(project.outfile), false)
    // A reused bundle has to be indistinguishable to the caller: the host keys
    // its own build caches on `dependencyHash`, so a reuse that reported a
    // different one would silently invalidate everything downstream.
    assert.equal(second.dependencyHash, first.dependencyHash)
    assert.equal(second.contentHash, first.contentHash)
    assert.deepEqual(second.fingerprintInputs, first.fingerprintInputs)
  })

  it('recompiles when a source inside the project changes', async () => {
    const project = await createProject('project-edit')
    const first = await compile(project)
    await mark(project.outfile)

    await writeFile(
      path.join(project.root, 'src', 'local.ts'),
      (await readFile(path.join(project.root, 'src', 'local.ts'), 'utf8')) +
        'export const added = 2\n',
    )
    const second = await compile(project)

    assert.equal(await recompiled(project.outfile), true)
    assert.notEqual(second.dependencyHash, first.dependencyHash)
  })

  /**
   * The case a project-scoped key cannot see. Editing the framework is what
   * developing the framework *is*, and the reused bundle would still carry the
   * previous version's code with nothing to indicate it.
   */
  it('recompiles when a source outside the project changes', async () => {
    const project = await createProject('vendor-edit')
    const first = await compile(project)
    await mark(project.outfile)

    await writeFile(path.join(project.outside, 'helper.ts'), 'export const helper = 99\n')
    const second = await compile(project)

    assert.equal(await recompiled(project.outfile), true)
    assert.notEqual(second.contentHash, first.contentHash)
    // Nothing in the project changed, so the project-scoped fingerprint is the
    // same for both — which is exactly why it cannot be the reuse key.
    assert.equal(second.dependencyHash, first.dependencyHash)
  })

  it('recompiles when a manifest that was absent appears', async () => {
    const project = await createProject('manifest-added')
    await compile(project)
    await mark(project.outfile)

    await writeFile(
      path.join(project.root, 'tsconfig.json'),
      JSON.stringify({ compilerOptions: {} }),
    )
    await compile(project)

    assert.equal(await recompiled(project.outfile), true)
  })

  it('recompiles when the emitted bundle is gone', async () => {
    const project = await createProject('output-removed')
    await compile(project)
    await rm(project.outfile)

    const rebuilt = await compile(project)

    assert.equal(rebuilt.outfile, project.outfile)
    assert.match(await readFile(project.outfile, 'utf8'), /local/)
  })

  it('recompiles when the recorded manifest cannot be read', async () => {
    const project = await createProject('manifest-corrupt')
    await compile(project)
    await mark(project.outfile)

    await writeFile(`${project.outfile}.inputs.json`, '{ not json')
    await compile(project)

    assert.equal(await recompiled(project.outfile), true)
  })
})
