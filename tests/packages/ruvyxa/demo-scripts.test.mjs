import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const demoPackageUrl = new URL('../../../examples/demo/package.json', import.meta.url)

describe('demo command contract', () => {
  it('exposes every documented CLI workflow through npm scripts', async () => {
    const demoPackage = JSON.parse(await readFile(fileURLToPath(demoPackageUrl), 'utf8'))
    const command = 'cargo run -p ruvyxa_cli --'
    const expectedScripts = {
      dev: `${command} dev --root .`,
      build: `${command} build --root .`,
      start: `${command} start --root .`,
      preview: `${command} start --root . --port 3000`,
      typecheck: 'tsc --noEmit',
      check: `${command} check --root .`,
      routes: `${command} routes --root .`,
      'routes:json': `${command} routes --root . --json`,
      analyze: `${command} analyze --root .`,
      'analyze:html': `${command} analyze --root . --html`,
      adds: `${command} adds --root .`,
      doctor: `${command} doctor --root .`,
      clean: `${command} clean --root .`,
      trace: `${command} trace --root .`,
      bench: `${command} bench --root .`,
      'test:parity': `${command} test:parity --root .`,
    }

    assert.deepEqual(
      Object.fromEntries(
        Object.keys(expectedScripts).map((name) => [name, demoPackage.scripts[name]]),
      ),
      expectedScripts,
    )
    assert.equal(demoPackage.scripts.parity, expectedScripts['test:parity'])
  })
})
