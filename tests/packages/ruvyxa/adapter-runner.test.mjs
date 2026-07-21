import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const adapterRunner = path.join(workspaceRoot, 'packages/ruvyxa/runtime/adapter-runner.mjs')

describe('adapter runner', () => {
  it('materializes static deployment artifacts from a static-only build', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender', 'about'), { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'static-site', path: 'deploy/site' },
          { kind: 'file', path: 'deploy/site/_headers', contents: 'X-Frame-Options: DENY\\n' }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/', render: { strategy: 'ssg' } },
            { kind: 'page', path: '/about', render: { strategy: 'csr' } },
          ],
        }),
      )
      await writeFile(path.join(outputDir, 'assets', 'app.css'), 'body {}')
      await writeFile(path.join(outputDir, 'client', 'app.js'), 'export {}')
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await writeFile(
        path.join(outputDir, 'prerender', 'about', 'index.html'),
        '<main>about</main>',
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [
        { kind: 'static-site', path: 'deploy/site' },
        { kind: 'file', path: 'deploy/site/_headers' },
      ])
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/index.html'), 'utf8'),
        '<main>home</main>',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/about/index.html'), 'utf8'),
        '<main>about</main>',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/assets/app.css'), 'utf8'),
        'body {}',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/_headers'), 'utf8'),
        'X-Frame-Options: DENY\n',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects routes the adapter declares it does not support', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { name: 'static', supports: ['ssg', 'csr'], build() { return { artifacts: [{ kind: 'static-site', path: 'deploy/site' }] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [{ kind: 'api', path: '/api/health', render: { strategy: 'ssr' } }],
        }),
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2202 adapter static supports ssg, csr/)
      assert.match(result.parsed.message, /\/api\/health \(api\)/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // Regression: the static-only rule used to live in `materializeStaticSite`
  // and applied to every `static-site` artifact, so the vercel/netlify/
  // cloudflare adapters -- which emit that artifact for the static layer beside
  // a serverless function -- could never build an app with an API or SSR route.
  it('allows a hybrid adapter to emit a static-site artifact alongside SSR and API routes', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { name: 'vercel', supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'], build() { return { artifacts: [{ kind: 'static-site', path: 'deploy/vercel/static' }] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/', render: { strategy: 'ssg' } },
            { kind: 'page', path: '/blog/[slug]', render: { strategy: 'ssr' } },
            { kind: 'page', path: '/isr-page', render: { strategy: 'isr' } },
            { kind: 'api', path: '/api/health' },
          ],
        }),
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [{ kind: 'static-site', path: 'deploy/vercel/static' }])
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/vercel/static/index.html'), 'utf8'),
        '<main>home</main>',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('materializes allowlisted project-scope artifacts at the project root', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
          { kind: 'file', path: '.vercel/output/config.json', scope: 'project', contents: '{"version":3}' },
          { kind: 'file', path: 'netlify.toml', scope: 'project', skipIfExists: true, contents: 'generated' },
          { kind: 'file', path: 'wrangler.jsonc', scope: 'project', skipIfExists: true, contents: '{"name":"app"}' }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({ routes: [{ kind: 'page', path: '/', render: { strategy: 'ssg' } }] }),
      )
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      // Stale output from an earlier build must be replaced, and a
      // user-authored netlify.toml must never be overwritten.
      await mkdir(path.join(root, '.vercel/output/static'), { recursive: true })
      await writeFile(path.join(root, '.vercel/output/static/stale.js'), 'stale')
      await writeFile(path.join(root, 'netlify.toml'), 'user-authored')

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [
        { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
        { kind: 'file', path: '.vercel/output/config.json', scope: 'project' },
        { kind: 'file', path: 'netlify.toml', scope: 'project', skipped: true },
        { kind: 'file', path: 'wrangler.jsonc', scope: 'project' },
      ])
      assert.equal(
        await readFile(path.join(root, '.vercel/output/static/index.html'), 'utf8'),
        '<main>home</main>',
      )
      assert.equal(
        await readFile(path.join(root, '.vercel/output/config.json'), 'utf8'),
        '{"version":3}',
      )
      assert.equal(await readFile(path.join(root, 'netlify.toml'), 'utf8'), 'user-authored')
      assert.equal(await readFile(path.join(root, 'wrangler.jsonc'), 'utf8'), '{"name":"app"}')
      await assert.rejects(readFile(path.join(root, '.vercel/output/static/stale.js')))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects project-scope artifacts outside the allowlist', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'file', path: 'package.json', scope: 'project', contents: '{}' }
        ] } } } }`,
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /project-scope adapter artifact path is not allowlisted/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects artifacts that overlap protected build output', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'file', path: 'manifest.json', contents: '{}' }
        ] } } } }`,
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /overlaps protected build output/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})

function runRunner(root, outputDir) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [adapterRunner, root, outputDir], { stdio: 'pipe' })
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
    child.on('close', (code) => {
      try {
        const parsed = JSON.parse(stdout)
        if (code === 0 && parsed.ok) resolve(parsed)
        else reject(new Error(`adapter runner failed (${code}): ${stdout || stderr}`))
      } catch (error) {
        reject(
          new Error(`invalid runner JSON: ${error.message}; stdout=${stdout}; stderr=${stderr}`),
        )
      }
    })
  })
}

function runRunnerResult(root, outputDir) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [adapterRunner, root, outputDir], { stdio: 'pipe' })
    let stdout = ''
    child.stdout.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.on('error', reject)
    child.on('close', (exitCode) => {
      try {
        resolve({ exitCode, parsed: JSON.parse(stdout) })
      } catch (error) {
        reject(new Error(`invalid runner JSON: ${error.message}; stdout=${stdout}`))
      }
    })
  })
}
