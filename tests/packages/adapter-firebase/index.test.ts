import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { firebase } from '../../../packages/@ruvyxa/adapter-firebase/dist/index.js'

describe('firebase', () => {
  it('emits Hosting, Functions v2, cache, and rewrite artifacts', async () => {
    const adapter = firebase({ functionName: 'webApp', region: 'asia-east1' })
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })

    assert.equal(output.platform, 'firebase')
    assert.equal(output.target, 'serverless')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/firebase/public', scope: undefined },
        { kind: 'function', path: 'deploy/firebase/functions', scope: undefined },
        {
          kind: 'file',
          path: 'deploy/firebase/functions/package.json',
          scope: undefined,
        },
        { kind: 'file', path: 'deploy/firebase/firebase.json', scope: undefined },
        { kind: 'file', path: 'deploy/firebase/README.md', scope: undefined },
        { kind: 'file', path: 'firebase.json', scope: 'project' },
      ],
    )

    const firebaseJson = output.artifacts?.find((artifact) => artifact.path === 'firebase.json')
    assert.equal(firebaseJson?.skipIfExists, true)
    const config = JSON.parse(
      firebaseJson && 'contents' in firebaseJson ? String(firebaseJson.contents) : '{}',
    )
    assert.equal(config.hosting.public, '.ruvyxa/deploy/firebase/public')
    assert.deepEqual(config.hosting.rewrites, [
      {
        source: '**',
        function: { functionId: 'webApp', region: 'asia-east1', pinTag: true },
      },
    ])
    assert.equal(config.functions[0].runtime, 'nodejs24')
    assert.equal(config.hosting.headers[0].headers[0].value, 'public, max-age=31536000, immutable')

    const handler = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = String(handler && 'handlerSource' in handler ? handler.handlerSource : '')
    assert.match(source, /firebase-functions\/v2\/https/)
    assert.match(source, /export const webApp = onRequest/)
    assert.match(source, /os\.tmpdir\(\)/)
    assert.match(source, /prerenderRelativePath/)
    assert.doesNotMatch(source, /ISR cache write failures/)
    assert.match(source, /getSetCookie/)

    const packageArtifact = output.artifacts?.find((artifact) =>
      artifact.path.endsWith('functions/package.json'),
    )
    const packageJson = JSON.parse(
      packageArtifact && 'contents' in packageArtifact ? String(packageArtifact.contents) : '{}',
    )
    assert.equal(packageJson.engines.node, '24')
    assert.equal(packageJson.dependencies['firebase-functions'], '^7.3.0')
  })

  it('validates function and region identifiers', () => {
    assert.doesNotThrow(() => firebase({ region: 'europe-west10' }))
    assert.throws(() => firebase({ functionName: 'bad-name' }), /functionName/)
    assert.throws(() => firebase({ region: 'moon-1' }), /region/)
  })
})
