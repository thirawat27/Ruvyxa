import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

import { useActionState } from 'react'
import { useFormStatus } from 'react-dom'

const manifest = async (specifier) =>
  JSON.parse(await readFile(new URL(specifier, import.meta.url), 'utf8'))

test('declares and exercises the stable React 19 action APIs', async () => {
  assert.equal(typeof useActionState, 'function')
  assert.equal(typeof useFormStatus, 'function')
  const { peerDependencies } = await manifest('../package.json')
  assert.match(peerDependencies.react, /^\^19\./)
  assert.match(peerDependencies['react-dom'], /^\^19\./)
})

// `ruvyxa` declares the same two peers, which is what lets `npm install ruvyxa`
// install React on its own. One requirement written in two manifests drifts, and
// it drifts silently: an app that satisfies the looser range and not the tighter
// one installs cleanly and fails at render. Comparing the two ranges rather than
// pinning a literal here also keeps the version in the manifests, so a bump is
// one edit instead of three.
test('the framework package declares the same React peer range', async () => {
  const integration = await manifest('../package.json')
  const framework = await manifest('../../../ruvyxa/package.json')

  assert.equal(framework.peerDependencies.react, integration.peerDependencies.react)
  assert.equal(framework.peerDependencies['react-dom'], integration.peerDependencies['react-dom'])
})
