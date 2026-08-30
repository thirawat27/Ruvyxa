import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { railway } from '../../../packages/@ruvyxa/adapter-railway/dist/index.js'

describe('railway', () => {
  it('produces a standalone full-stack deployment and safe Railway config', async () => {
    const adapter = railway()
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })

    assert.equal(output.platform, 'railway')
    assert.equal(output.target, 'node')
    assert.equal(output.runtime, 'node')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'function', path: 'deploy/railway/server', scope: undefined },
        { kind: 'static-site', path: 'deploy/railway/public', scope: undefined },
        { kind: 'file', path: 'deploy/railway/railway.json', scope: undefined },
        { kind: 'file', path: 'deploy/railway/README.md', scope: undefined },
        { kind: 'file', path: 'railway.json', scope: 'project' },
      ],
    )

    const server = output.artifacts?.find((artifact) => artifact.kind === 'function')
    assert.match(String(server && 'handlerSource' in server ? server.handlerSource : ''), /PORT/)
    assert.match(
      String(server && 'handlerSource' in server ? server.handlerSource : ''),
      /0\.0\.0\.0/,
    )

    const configArtifact = output.artifacts?.find((artifact) => artifact.path === 'railway.json')
    assert.equal(configArtifact?.skipIfExists, true)
    const config = JSON.parse(
      configArtifact && 'contents' in configArtifact ? String(configArtifact.contents) : '{}',
    )
    assert.equal(config.build.builder, 'RAILPACK')
    assert.equal(config.build.buildCommand, 'npm run build')
    assert.equal(config.deploy.startCommand, 'node .ruvyxa/deploy/railway/server/index.mjs')
  })

  it('can avoid project-root configuration', async () => {
    const output = await railway({ projectConfig: false }).build({
      root: '.',
      outDir: '.ruvyxa',
    })
    assert.equal(
      output.artifacts?.some((artifact) => artifact.scope === 'project'),
      false,
    )
  })

  // Railway runs the start command from the repository root, so the path has
  // to follow this build's outDir rather than the `.ruvyxa` default.
  it('points the start command at the configured out directory', async () => {
    const output = await railway().build({ root: '/srv/app', outDir: '/srv/app/build' })
    const artifact = output.artifacts?.find((item) => item.path === 'railway.json')
    const config = JSON.parse(artifact && 'contents' in artifact ? String(artifact.contents) : '{}')
    assert.equal(config.deploy.startCommand, 'node build/deploy/railway/server/index.mjs')
  })
})

/**
 * An `outDir` that cannot survive interpolation is refused at build time.
 *
 * The generated file is assembled by string concatenation and the path was not
 * quoted: `#` starts a YAML comment and truncates the command, `: ` turns the
 * scalar into a nested mapping and fails the parse, and a space produces valid
 * YAML and an invalid shell command. The value comes from `ruvyxa.config.ts`, so
 * the failure landed on the platform rather than on the developer's machine —
 * the expensive place to find it. `adapter-static` already refused its
 * equivalent input this way.
 */
describe('railway outDir validation', () => {
  for (const outDir of ['out dir', 'out#dir', 'out: dir', 'out|dir', 'out$(whoami)']) {
    it(`refuses an outDir containing ${JSON.stringify(outDir)}`, async () => {
      await assert.rejects(
        async () => railway().build({ root: '.', outDir }),
        /RUV2001/,
        'an outDir that breaks the generated command must fail the build, not the deploy',
      )
    })
  }

  it('still accepts the ordinary shapes a project uses', async () => {
    for (const outDir of ['.ruvyxa', 'dist', 'build/out', 'my-app.out', 'a_b']) {
      const output = await railway().build({ root: '.', outDir })
      assert.ok(output.artifacts && output.artifacts.length > 0, outDir)
    }
  })
})
