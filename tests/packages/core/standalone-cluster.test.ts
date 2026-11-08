import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  clusterSource,
  crashLoopExceeded,
  recentCrashes,
  resolveWorkerCount,
  respawnDelay,
} from '../../../packages/@ruvyxa/core/dist/standalone-cluster.js'

describe('resolveWorkerCount', () => {
  it('defaults to one process', () => {
    assert.equal(resolveWorkerCount(undefined, 8), 1)
    assert.equal(resolveWorkerCount('  ', 8), 1)
  })
  it('reads a positive integer', () => {
    assert.equal(resolveWorkerCount('4', 8), 4)
    assert.equal(resolveWorkerCount(' 2 ', 8), 2)
  })
  it('reads auto as the available parallelism, never below one', () => {
    assert.equal(resolveWorkerCount('auto', 6), 6)
    assert.equal(resolveWorkerCount('AUTO', 0), 1)
  })
  it('refuses anything else with RUV1025', () => {
    for (const value of ['0', '-1', '1.5', 'two', '04']) {
      assert.throws(() => resolveWorkerCount(value, 8), /^Error: RUV1025 /, value)
    }
  })
})

describe('crashLoopExceeded', () => {
  it('allows five crashes inside the window and refuses the sixth', () => {
    const now = 100_000
    const five = [now - 50_000, now - 40_000, now - 30_000, now - 20_000, now - 10_000]
    assert.equal(crashLoopExceeded(five, now), false)
    assert.equal(crashLoopExceeded([...five, now], now), true)
  })
  it('forgets crashes older than the window', () => {
    const now = 200_000
    const old = [1, 2, 3, 4, 5, 6]
    assert.equal(crashLoopExceeded(old, now), false)
  })
})

describe('respawn backoff', () => {
  it('doubles with each recent crash and caps at thirty seconds', () => {
    assert.deepEqual(
      [1, 2, 3, 4, 5, 6, 7, 20].map(respawnDelay),
      [500, 1000, 2000, 4000, 8000, 16_000, 30_000, 30_000],
    )
  })

  it('resets once the earlier crashes leave the window', () => {
    // The supervisor's own sequence: append, prune to the window, derive.
    let crashes: number[] = []
    const crashAt = (now: number) => {
      crashes = recentCrashes([...crashes, now], now)
      return respawnDelay(crashes.length)
    }
    assert.equal(crashAt(0), 500)
    assert.equal(crashAt(1_000), 1000)
    assert.equal(crashAt(2_000), 2000)
    assert.equal(crashAt(3_000), 4000)
    // Two minutes later: the four above are outside the window, so this is a
    // first crash again rather than the fifth, and the record holds one entry.
    assert.equal(crashAt(123_000), 500)
    assert.deepEqual(crashes, [123_000])
  })

  it('keeps the crash-loop rule over the pruned record', () => {
    let crashes: number[] = []
    let exceeded = false
    for (let crash = 0; crash < 6; crash += 1) {
      const now = crash * 10_000
      crashes = recentCrashes([...crashes, now], now)
      exceeded = crashLoopExceeded(crashes, now)
    }
    assert.equal(exceeded, true)
  })

  it('hints at more workers only where the runtime can run them', () => {
    const hint = /serving with one process; set RUVYXA_WORKERS/
    const node = clusterSource('node')
    assert.match(node, hint)
    assert.match(
      node,
      /WORKER_COUNT === 1 &&\s+\(process\.env\.RUVYXA_WORKERS \?\? ''\)\.trim\(\) === ''/,
    )
    assert.match(
      clusterSource('bun'),
      /os\.availableParallelism\(\) > 1 &&\s+process\.platform === 'linux'/,
    )
    assert.doesNotMatch(clusterSource('deno'), hint)
  })

  it('is the rule the generated supervisor runs', () => {
    for (const runtime of ['node', 'bun'] as const) {
      const source = clusterSource(runtime)
      assert.ok(source.includes(recentCrashes.toString()), runtime)
      assert.ok(source.includes(respawnDelay.toString()), runtime)
      assert.match(source, /crashes = recentCrashes\(\[\.\.\.crashes, now\], now\);/)
      assert.match(source, /const delay = respawnDelay\(crashes\.length\);/)
    }
  })
})
