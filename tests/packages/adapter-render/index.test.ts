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
})
