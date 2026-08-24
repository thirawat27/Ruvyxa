/**
 * The module-lane table, replayed against the Node module graph.
 *
 * `crates/ruvyxa_bundler/src/references.rs` replays the same file. The two had
 * disagreed, and the looser one was this graph — the one behind `ruvyxa dev`.
 * See `tests/fixtures/module-lane-conformance.json` for what that cost.
 */
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { readFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const compilerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs')

const { compileBundleWithMetadata, moduleLane, toImportPath } = await import(
  `file://${compilerModule.replaceAll('\\', '/')}`
)

const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/module-lane-conformance.json'), 'utf8'),
)

describe('module lane assignment', () => {
  for (const testCase of fixture.cases) {
    const label = testCase.$why ?? `${testCase.file} is ${testCase.lane}`
    it(label, () => {
      assert.equal(
        moduleLane(path.join('/project', testCase.file), testCase.source),
        testCase.lane,
        `${testCase.file} must be a ${testCase.lane} module`,
      )
    })
  }

  it('reads the lanes the fixture declares rather than a private list', () => {
    // The fixture is the contract; a host that invented its own extra lane
    // would pass every case above and still disagree with the other host.
    for (const [directive, lane] of Object.entries(fixture.directiveLanes)) {
      assert.equal(moduleLane('/project/app/x.ts', `'${directive}'\nexport const q = 1`), lane)
    }
    for (const [stem, lane] of Object.entries(fixture.fileStemLanes)) {
      assert.equal(moduleLane(`/project/app/${stem}.ts`, 'export const q = 1'), lane)
    }
    assert.equal(moduleLane('/project/app/anything.ts', 'export const q = 1'), fixture.defaultLane)
  })
})

describe('client bundle lane crossings', () => {
  /** Compile one helper into a browser bundle and report whether it was allowed. */
  async function compileIntoClientBundle(relative, source) {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-module-lane-'))
    try {
      const helper = path.join(root, relative)
      await mkdir(path.dirname(helper), { recursive: true })
      await writeFile(helper, source, 'utf8')
      await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: `import { q } from ${JSON.stringify(toImportPath(helper))}\nexport default function Widget() { return q }\n`,
        sourcefile: 'app/Widget.tsx',
        outfile: path.join(root, 'out.js'),
        platform: 'browser',
        external: ['react', 'react/jsx-runtime'],
      })
      return null
    } catch (error) {
      return String(error.message)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  }

  const rejectedLanes = new Set(
    Object.values(fixture.invalidCrossings)
      .filter((crossing) => crossing.bothHosts && crossing.from === 'client')
      .map((crossing) => crossing.to),
  )

  for (const testCase of fixture.cases) {
    // Only the plain stem cases: a directive case would need its source to
    // survive into the bundle, and lane assignment is already covered above.
    if (testCase.$why || testCase.file.endsWith('.d.ts')) continue
    const mustReject = rejectedLanes.has(testCase.lane)

    it(`${mustReject ? 'refuses' : 'accepts'} ${testCase.file} in a client bundle`, async () => {
      const failure = await compileIntoClientBundle(testCase.file, testCase.source)
      if (mustReject) {
        assert.ok(
          failure?.includes('RUV1007'),
          `${testCase.file} is a ${testCase.lane} module and must not reach the browser; got ${failure ?? 'no error'}`,
        )
      } else {
        assert.equal(failure, null, `${testCase.file} is browser-safe: ${failure}`)
      }
    })
  }

  it('refuses any file under the project server directory whatever it is named', async () => {
    const directory = fixture.serverDirectory.replace(/\/$/, '')
    const failure = await compileIntoClientBundle(`${directory}/queries.ts`, 'export const q = 1\n')
    assert.ok(failure?.includes('RUV1007'), `got ${failure ?? 'no error'}`)
  })

  it('records which crossing only the Rust bundler can see', () => {
    // Honest about the gap rather than implying a parity that does not exist:
    // this graph has no hook on the server compile, so an action module
    // importing a client module is caught by `references.rs` alone.
    assert.equal(fixture.invalidCrossings.actionToClient.bothHosts, false)
  })
})

describe('marker packages', () => {
  /**
   * Compile a server module that declares a marker and report the bundle.
   *
   * A marker package is not installed here on purpose: the point is that the
   * emitted bundle must not name it, so a deployed function directory with no
   * node_modules of its own still starts.
   */
  async function compileServerBundle(source) {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-marker-'))
    try {
      const entry = path.join(root, 'app', 'page.tsx')
      await mkdir(path.dirname(entry), { recursive: true })
      await writeFile(entry, source, 'utf8')
      const { outfile } = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: source,
        sourcefile: 'app/page.tsx',
        filePath: entry,
        outfile: path.join(root, 'out.mjs'),
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })
      return await readFile(outfile, 'utf8')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  }

  for (const marker of fixture.markerPackages) {
    it(`drops ${marker} from a server bundle`, async () => {
      const code = await compileServerBundle(
        [`import ${JSON.stringify(marker)}`, 'export const q = 1', ''].join('\n'),
      )
      assert.ok(
        !code.includes(marker),
        `${marker} is a declaration for the boundary checker, not a runtime dependency: ${code}`,
      )
    })
  }

  it('still emits an ordinary external import', async () => {
    const code = await compileServerBundle(
      ["import { readFile } from 'node:fs'", 'export const q = readFile', ''].join('\n'),
    )
    assert.ok(code.includes('node:fs'), `an ordinary dependency must survive: ${code}`)
  })
})
