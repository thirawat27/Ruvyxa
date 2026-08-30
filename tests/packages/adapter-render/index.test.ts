import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { render } from '../../../packages/@ruvyxa/adapter-render/dist/index.js'

describe('render', () => {
  it('produces a standalone full-stack deployment and Render Blueprint', async () => {
    const adapter = render({ serviceName: 'shop-web' })
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })

    assert.equal(output.platform, 'render')
    assert.equal(output.target, 'node')
    assert.equal(output.runtime, 'node')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'function', path: 'deploy/render/server', scope: undefined },
        { kind: 'static-site', path: 'deploy/render/public', scope: undefined },
        { kind: 'file', path: 'deploy/render/render.yaml', scope: undefined },
        { kind: 'file', path: 'deploy/render/README.md', scope: undefined },
        { kind: 'file', path: 'render.yaml', scope: 'project' },
      ],
    )

    const blueprintArtifact = output.artifacts?.find((artifact) => artifact.path === 'render.yaml')
    assert.equal(blueprintArtifact?.skipIfExists, true)
    const blueprint =
      blueprintArtifact && 'contents' in blueprintArtifact ? String(blueprintArtifact.contents) : ''
    assert.match(blueprint, /name: "shop-web"/)
    assert.match(blueprint, /runtime: node/)
    assert.match(blueprint, /key: NODE_VERSION\s+value: ">=24\.19\.0 <25"/)
    assert.match(blueprint, /plan: free/)
    assert.match(blueprint, /buildCommand: npm run build/)
    assert.match(blueprint, /startCommand: node \.ruvyxa\/deploy\/render\/server\/index\.mjs/)
  })

  it('rejects unsafe Blueprint service names', () => {
    assert.throws(() => render({ serviceName: 'bad\nname' }), /serviceName.*lowercase letters/)
  })

  // Render runs the start command from the repository root, so the path has to
  // follow this build's outDir rather than the `.ruvyxa` default.
  it('points the start command at the configured out directory', async () => {
    const output = await render().build({ root: '/srv/app', outDir: '/srv/app/build' })
    const artifact = output.artifacts?.find((item) => item.path === 'render.yaml')
    const blueprint = artifact && 'contents' in artifact ? String(artifact.contents) : ''
    assert.match(blueprint, /startCommand: node build\/deploy\/render\/server\/index\.mjs/)
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
describe('render outDir validation', () => {
  for (const outDir of ['out dir', 'out#dir', 'out: dir', 'out|dir', 'out$(whoami)']) {
    it(`refuses an outDir containing ${JSON.stringify(outDir)}`, async () => {
      await assert.rejects(
        async () => render({ serviceName: 'shop-web' }).build({ root: '.', outDir }),
        /RUV2001/,
        'an outDir that breaks the generated command must fail the build, not the deploy',
      )
    })
  }

  it('still accepts the ordinary shapes a project uses', async () => {
    for (const outDir of ['.ruvyxa', 'dist', 'build/out', 'my-app.out', 'a_b']) {
      const output = await render({ serviceName: 'shop-web' }).build({ root: '.', outDir })
      assert.ok(output.artifacts && output.artifacts.length > 0, outDir)
    }
  })
})
