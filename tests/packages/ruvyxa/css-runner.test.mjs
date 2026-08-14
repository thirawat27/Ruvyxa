/**
 * Regression coverage for the global CSS PostCSS stage.
 *
 * The incident these tests exist for: a project declared `@tailwindcss/postcss`
 * in `postcss.config.mjs`, and the build copied `globals.css` to the output
 * untransformed. The browser cannot resolve `@import "tailwindcss"`, so every
 * page rendered with browser defaults while the markup carried correct class
 * names.
 *
 * `css-runner.mjs` is the stage that fixes it. It is invoked by
 * `crates/ruvyxa_dev_server/src/postcss.rs`, which decides *whether* a project
 * has PostCSS; these tests cover *what happens* when it does. The framework
 * never names a plugin, so the fixtures register their own.
 */

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const cssRunner = path.join(workspaceRoot, 'packages/ruvyxa/runtime/css-runner.mjs')

const workspaces = []
after(() =>
  Promise.all(workspaces.map((root) => rm(root, { recursive: true, force: true, maxRetries: 5 }))),
)

/**
 * Build a project fixture inside the monorepo.
 *
 * Deliberately not in the OS temp directory: `css-runner.mjs` resolves `postcss`
 * and every plugin from the *project*, walking up from its root, which is
 * exactly the resolution an installed app gets. A fixture outside the repo would
 * find nothing.
 */
async function withProject(files, run) {
  await mkdir(path.join(workspaceRoot, 'target'), { recursive: true })
  const root = await mkdtemp(path.join(workspaceRoot, 'target', 'ruvyxa-css-runner-'))
  workspaces.push(root)
  for (const [name, contents] of Object.entries(files)) {
    await writeFile(path.join(root, name), contents)
  }
  return run(root)
}

/** Invoke the runner the way the Rust CSS pipeline does and parse its report. */
async function runCss(root, { css, config = 'postcss.config.mjs', mode = 'production' }) {
  const cssFile = path.join(root, '__input.css')
  const requestFile = path.join(root, '__request.json')
  await writeFile(cssFile, css)
  await writeFile(
    requestFile,
    JSON.stringify({
      root,
      config: path.join(root, config),
      from: path.join(root, 'globals.css'),
      cssFile,
      mode,
    }),
  )

  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cssRunner, requestFile], {
      cwd: root,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (exitCode) => {
      try {
        resolve({ exitCode, report: JSON.parse(stdout.trim().split('\n').at(-1)), stderr })
      } catch (error) {
        reject(new Error(`invalid report: ${error.message}; stdout=${stdout}; stderr=${stderr}`))
      }
    })
  })
}

/** A local plugin, so the fixtures depend on no package beyond `postcss`. */
const RENAME_PLUGIN = `export default (options = {}) => ({
  postcssPlugin: 'rename',
  Rule(rule) { rule.selector = rule.selector.replace('.from', options.to ?? '.to') },
})
`

describe('global CSS PostCSS stage', () => {
  it('runs the plugin chain a config declares as a name-keyed object map', () =>
    withProject(
      {
        'rename.mjs': RENAME_PLUGIN,
        // The shape a Tailwind v4 project writes:
        // `{ '@tailwindcss/postcss': {} }`. The framework resolves the name from
        // the project and passes the declared options through.
        'postcss.config.mjs': `export default { plugins: { './rename.mjs': { to: '.renamed' } } }\n`,
      },
      async (root) => {
        const { exitCode, report } = await runCss(root, { css: '.from { color: red }' })
        assert.equal(exitCode, 0, JSON.stringify(report))
        assert.equal(report.ok, true)
        assert.match(report.css, /\.renamed/)
        assert.doesNotMatch(report.css, /\.from/)
      },
    ))

  it('runs a plugin chain declared as an array', () =>
    withProject(
      {
        'rename.mjs': RENAME_PLUGIN,
        'postcss.config.mjs': `import rename from './rename.mjs'
export default { plugins: [rename({ to: '.renamed' })] }
`,
      },
      async (root) => {
        const { exitCode, report } = await runCss(root, { css: '.from { color: red }' })
        assert.equal(exitCode, 0)
        assert.equal(report.ok, true)
        assert.match(report.css, /\.renamed/)
        assert.doesNotMatch(report.css, /\.from/)
      },
    ))

  it('passes the build mode to a config declared as a function', () =>
    withProject(
      {
        'postcss.config.mjs': `export default (context) => ({
  plugins: [
    {
      postcssPlugin: 'stamp-mode',
      Once(rootNode, { result }) {
        rootNode.append(\`.mode-\${context.mode} { color: blue }\`)
        result.messages.push({ type: 'dependency', file: '/watched/template.tsx' })
      },
    },
  ],
})
`,
      },
      async (root) => {
        const { report } = await runCss(root, { css: '.a { color: red }', mode: 'production' })
        assert.equal(report.ok, true)
        assert.match(report.css, /\.mode-production/)
        // Plugin-reported files become watch inputs; without them a dev edit
        // that only changes class names never regenerates the stylesheet.
        assert.deepEqual(report.dependencies, ['/watched/template.tsx'])
      },
    ))

  it('returns the stylesheet unchanged when the config registers no plugins', () =>
    withProject({ 'postcss.config.mjs': 'export default { plugins: {} }\n' }, async (root) => {
      const { report } = await runCss(root, { css: '.a { color: red }' })
      assert.equal(report.ok, true)
      assert.equal(report.css, '.a { color: red }')
    }))

  it('reads a JSON config and reports an uninstallable plugin by name', () =>
    withProject(
      { '.postcssrc.json': JSON.stringify({ plugins: { 'not-a-real-plugin': {} } }) },
      async (root) => {
        const { exitCode, report } = await runCss(root, {
          css: '.a { color: red }',
          config: '.postcssrc.json',
        })
        assert.equal(exitCode, 1)
        assert.equal(report.ok, false)
        assert.equal(report.code, 'RUV1405')
        assert.match(report.message, /not-a-real-plugin/)
      },
    ))

  it('fails loudly when a plugin throws instead of emitting raw CSS', () =>
    withProject(
      {
        'postcss.config.mjs': `export default {
  plugins: [{ postcssPlugin: 'explode', Once() { throw new Error('plugin exploded') } }],
}
`,
      },
      async (root) => {
        const { exitCode, report } = await runCss(root, { css: '.a { color: red }' })
        assert.equal(exitCode, 1)
        assert.equal(report.ok, false)
        assert.equal(report.code, 'RUV1406')
        assert.match(report.message, /plugin exploded/)
        // A silent fallback to untransformed CSS is what shipped an unstyled
        // page to production.
        assert.equal(report.css, undefined)
      },
    ))

  it('reports a stylesheet syntax error with its position', () =>
    withProject(
      {
        'postcss.config.mjs': `export default {
  plugins: [{ postcssPlugin: 'noop', Once() {} }],
}
`,
      },
      async (root) => {
        const { report } = await runCss(root, { css: '.a { color: red' })
        assert.equal(report.ok, false)
        assert.equal(report.code, 'RUV1406')
        assert.match(report.message, /globals\.css/)
      },
    ))
})
