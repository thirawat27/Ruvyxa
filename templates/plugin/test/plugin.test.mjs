import assert from 'node:assert/strict'
import test from 'node:test'

import { createPluginHarness } from 'ruvyxa/plugin-harness'

import plugin from '../dist/index.js'

/**
 * Plugin behaviour lives inside `register(api)`, which the framework calls once
 * at startup. `createPluginHarness` runs it against recording sockets and
 * exposes the same entry points the server does, so a plugin is testable as a
 * plain unit — no server to boot, no sockets to hand-roll.
 */

test('declares its name', () => {
  assert.equal(plugin.name, '__PLUGIN_NAME__')
})

test('adds its header to every response', async () => {
  const harness = await createPluginHarness(plugin)

  const response = await harness.respond(new Response('ok'), '/')
  assert.equal(response.headers.get('x-__PLUGIN_NAME__'), 'active')

  const nested = await harness.respond(new Response('ok'), '/api/items')
  assert.equal(nested.headers.get('x-__PLUGIN_NAME__'), 'active')
})

test('registers cleanly: no routes, no diagnostics', async () => {
  const harness = await createPluginHarness(plugin)

  assert.deepEqual([...harness.routes], [])
  assert.deepEqual([...harness.diagnostics], [])
})
