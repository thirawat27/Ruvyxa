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

  it('rejects static deployment artifacts when dynamic routes or APIs are present', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [{ kind: 'static-site', path: 'deploy/site' }] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [{ kind: 'api', path: '/api/health', render: { strategy: 'ssr' } }],
        }),
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2202 static adapter output requires SSG or CSR pages/)
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
