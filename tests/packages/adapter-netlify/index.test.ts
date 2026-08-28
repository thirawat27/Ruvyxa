import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { execFileSync } from 'node:child_process'
import { globSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

import type { AdapterArtifact } from '../../../packages/@ruvyxa/core/dist/types.js'
import { DEFAULT_SECURITY_HEADERS } from '../../../packages/@ruvyxa/core/dist/utils.js'
import { nonPublishableStrategies } from '../../../packages/@ruvyxa/core/dist/deploy-manifest.js'
import { netlify } from '../../../packages/@ruvyxa/adapter-netlify/dist/index.js'
import {
  deployFunction,
  echoManifest,
  echoRouteModules,
  ECHO_BINARY_BODY,
  ECHO_COOKIES,
} from '../../deployed-function.ts'

/**
 * The `includedFiles` globs of an emitted handler, resolved the way Netlify
 * resolves them.
 *
 * A function directory is staged with one prerendered document in it, the
 * config is read out of the generated source, and the globs are matched with
 * `cwd` set to the directory holding the entry file — which is what
 * `zip-it-and-ship-it` does with an in-source `includedFiles`. Returns the
 * matched paths, so a glob that reaches nothing is an empty array rather than a
 * passing substring.
 */
function resolveIncludedFiles(handlerSource: string, functionPath: string): string[] {
  const declaration = /export const config = (\{[\s\S]*?\n\});/.exec(handlerSource)
  assert.ok(declaration, `no config export in the handler for ${functionPath}`)
  const config = JSON.parse(declaration[1]) as { includedFiles?: string[] }
  assert.ok(Array.isArray(config.includedFiles), `no includedFiles for ${functionPath}`)

  const root = mkdtempSync(path.join(os.tmpdir(), 'ruvyxa-netlify-included-'))
  try {
    // The layout `materializeFunction` writes: the entry beside the runtime
    // modules, with the deploy-time prerender output in a subdirectory.
    const functionDir = path.join(root, functionPath)
    mkdirSync(path.join(functionDir, 'prerender'), { recursive: true })
    writeFileSync(path.join(functionDir, 'index.mjs'), 'export default () => {}\n')
    writeFileSync(path.join(functionDir, 'prerender', 'index.html'), '<!doctype html>')
    // Directories are dropped the way `getPathsOfIncludedFiles` drops them, so
    // what is left is what would actually be copied into the bundle.
    return globSync(config.includedFiles, { cwd: functionDir })
      .filter((match) => statSync(path.join(functionDir, match)).isFile())
      .map((match) => match.replaceAll('\\', '/'))
      .sort()
  } finally {
    rmSync(root, { force: true, recursive: true })
  }
}

describe('netlify', () => {
  it('returns serverless deployment output with function artifacts', async () => {
    // The Frameworks API is the other half of the pair — see the defaults test
    // below — so it is asked for by name here rather than assumed.
    const output = await netlify({ frameworksApi: true, projectConfig: false }).build({
      root: '.',
      outDir: '.ruvyxa',
    })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/netlify/publish', scope: undefined },
        { kind: 'function', path: 'deploy/netlify/functions/ruvyxa-handler', scope: undefined },
        { kind: 'file', path: 'deploy/netlify/netlify.toml', scope: undefined },
        { kind: 'function', path: '.netlify/v1/functions/ruvyxa-handler', scope: 'project' },
        { kind: 'file', path: '.netlify/v1/config.json', scope: 'project' },
      ],
    )

    // Every static-site artifact must tolerate builds with no prerendered
    // pages (API-only or all-SSR apps) instead of failing with RUV2202.
    assert.ok(
      output.artifacts
        ?.filter((artifact) => artifact.kind === 'static-site')
        .every((artifact) => artifact.optional === true),
    )

    // Verify netlify.toml includes functions directory
    const toml = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/netlify/netlify.toml',
    )
    assert.match(toml && 'contents' in toml ? String(toml.contents) : '', /functions = "functions"/)
    assert.match(
      toml && 'contents' in toml ? String(toml.contents) : '',
      /for = "\/__ruvyxa\/client\/\*"[\s\S]*Cache-Control = "public, max-age=31536000, immutable"/,
    )

    // Frameworks API config carries the immutable cache header for hashed
    // client bundles; Netlify discovers it at .netlify/v1/config.json.
    const frameworksConfigArtifact = output.artifacts?.find(
      (artifact) => artifact.path === '.netlify/v1/config.json',
    )
    assert.ok(frameworksConfigArtifact)
    const frameworksConfig = JSON.parse(
      frameworksConfigArtifact && 'contents' in frameworksConfigArtifact
        ? String(frameworksConfigArtifact.contents)
        : '{}',
    )
    // First, and on everything: Netlify publishes pre-rendered documents and
    // public files itself, so the function — where `createHandler` sets these —
    // is never invoked for them, and a deployed SSG page carried none of them.
    assert.deepEqual(frameworksConfig.headers[0], {
      for: '/*',
      values: DEFAULT_SECURITY_HEADERS,
    })
    assert.deepEqual(frameworksConfig.headers[1], {
      for: '/__ruvyxa/client/*',
      values: { 'Cache-Control': 'public, max-age=31536000, immutable' },
    })
    // Public assets are not content-hashed, so they revalidate hourly rather
    // than inheriting Netlify's per-request default. The rules deliberately
    // skip js/css: on hosts whose `*` crosses path separators they would also
    // match the hashed bundles and downgrade their immutable header.
    const assetRules = frameworksConfig.headers.slice(2)
    assert.ok(assetRules.length > 0)
    assert.ok(
      assetRules.every(
        (rule: { for: string; values: Record<string, string> }) =>
          rule.values['Cache-Control'] === 'public, max-age=3600, must-revalidate',
      ),
    )
    assert.ok(assetRules.some((rule: { for: string }) => rule.for === '/*.png'))
    assert.equal(
      assetRules.some((rule: { for: string }) => rule.for === '/*.js'),
      false,
    )

    // Verify function artifacts share the handler source
    for (const functionPath of [
      'deploy/netlify/functions/ruvyxa-handler',
      '.netlify/v1/functions/ruvyxa-handler',
    ]) {
      const functionArtifact: AdapterArtifact | undefined = (output.artifacts ?? []).find(
        (artifact) => artifact.kind === 'function' && artifact.path === functionPath,
      )
      assert.ok(functionArtifact, functionPath)
      // Annotated because the narrowed property and the binding share a name,
      // which TypeScript reads as a circular initializer and widens to `any`.
      const handlerSource: string =
        'handlerSource' in functionArtifact ? String(functionArtifact.handlerSource) : ''
      assert.notEqual(handlerSource, '', functionPath)
      assert.match(handlerSource, /createHandler/)
      assert.match(handlerSource, /loadRouteModule/)
      assert.doesNotMatch(handlerSource, /\.\/server\/app/)
      assert.match(handlerSource, /export default/)

      // The ISR cache reads and writes files by request path, so it must go
      // through the shared containment helper rather than joining the raw
      // pathname onto the cache directory.
      assert.match(handlerSource, /prerenderRelativePath/)
      assert.doesNotMatch(handlerSource, /ISR cache write failures/)
      assert.doesNotMatch(handlerSource, /path\.join\(prerenderDir, pathname/)
      // Netlify Functions v2 config export
      assert.match(handlerSource, /export const config/)
      assert.match(handlerSource, /"preferStatic": true/)
      assert.match(handlerSource, /"path": "\/\*"/)

      // Framework attribution: Netlify reads `generator` to tell a
      // framework-emitted function from a hand-written one, and shows `name`
      // in the site UI.
      assert.match(handlerSource, /"generator": "ruvyxa\/\d+\.\d+\.\d+/)
      assert.match(handlerSource, /"name": "Ruvyxa SSR"/)

      // The deploy-time prerender output is not in the module graph, so it
      // only survives esbuild bundling if the config declares it — and a glob
      // that names nothing declares nothing while looking like it does.
      //
      // Resolved rather than matched, because the base directory is the whole
      // question and a string comparison cannot see it. `zip-it-and-ship-it`
      // sets `includedFilesBasePath = dirname(mainFile)` for an in-source
      // `includedFiles` (`runtimes/node/in_source_config/index.js`) and hands
      // that to `getPathsOfIncludedFiles`, which globs with `cwd: basePath` —
      // so the paths are relative to the function's own directory, not to the
      // site base. Both artifacts named a site-relative path, which resolved
      // to `<function>/functions/ruvyxa-handler/prerender/**`: nothing, in
      // every deploy workflow, with a green build behind it.
      assert.deepEqual(
        resolveIncludedFiles(handlerSource, functionPath),
        ['prerender/index.html'],
        `includedFiles must reach the prerender directory of ${functionPath}`,
      )

      // Netlify bundles the function with esbuild, so the manifest has to be
      // part of the module graph. Reading a sibling manifest.json at runtime
      // crashed the deployed function with ENOENT /var/task/manifest.json.
      assert.match(handlerSource, /import manifest from '\.\/manifest\.mjs'/)
      assert.doesNotMatch(handlerSource, /readFileSync\(manifestPath/)
    }

    // preferStatic serves a published page without invoking the function, so
    // ISR and PPR pages must stay out of the publish directory to revalidate.
    //
    // Compared against the derived rule, not a literal: this list used to be
    // written out in six places, and the value of deriving it is exactly that
    // no copy of it has to be kept correct — including this one.
    for (const artifact of output.artifacts?.filter((item) => item.kind === 'static-site') ?? []) {
      assert.deepEqual(artifact.excludeStrategies, nonPublishableStrategies())
      assert.ok(artifact.excludeStrategies?.includes('isr'))
      assert.ok(artifact.excludeStrategies?.includes('ppr'))
    }

    // Opt-in project netlify.toml embeds project-relative paths only — the
    // file is committed, so an absolute build-machine path would break every
    // other machine (and Windows backslashes are TOML escapes).
    const optIn = await netlify({ projectConfig: true }).build({
      root: 'D:\\work\\site',
      outDir: 'D:\\work\\site\\.ruvyxa',
    })
    const projectToml = optIn.artifacts?.find((artifact) => artifact.path === 'netlify.toml')
    assert.ok(projectToml)
    assert.equal(projectToml?.skipIfExists, true)
    assert.equal(projectToml?.scope, 'project')
    const projectTomlContents =
      projectToml && 'contents' in projectToml ? String(projectToml.contents) : ''
    assert.match(projectTomlContents, /publish = "\.ruvyxa\/deploy\/netlify\/publish"/)
    assert.match(projectTomlContents, /functions = "\.ruvyxa\/deploy\/netlify\/functions"/)
    assert.doesNotMatch(projectTomlContents, /D:\\/)

    // frameworksApi: false drops the .netlify/v1 artifacts
    assert.deepEqual(
      (
        await netlify({ frameworksApi: false, projectConfig: false }).build({
          root: '.',
          outDir: '.ruvyxa',
        })
      ).artifacts?.map(({ path }) => path),
      [
        'deploy/netlify/publish',
        'deploy/netlify/functions/ruvyxa-handler',
        'deploy/netlify/netlify.toml',
      ],
    )

    // Verify adapter metadata
    assert.deepEqual(
      {
        name: output.name,
        target: output.target,
        platform: output.platform,
        entry: output.entry,
        assetsDir: output.assetsDir,
        clientDir: output.clientDir,
        chunkManifest: output.chunkManifest,
        functionsDir: output.functionsDir,
      },
      {
        name: 'netlify',
        target: 'serverless',
        platform: 'netlify',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
        functionsDir: '.ruvyxa/netlify/functions',
      },
    )
  })

  it('declares supported strategies', () => {
    const adapter = netlify()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
  })

  // Everything above reads the artifact list and the generated source text.
  // This runs the function, because a wrapper that folds two Set-Cookie headers
  // into one string, or loses binary bytes, matches every regex a text
  // assertion could write.
  it('serves a request through the deployed function bundle', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-netlify-handler-'))
    try {
      const output = await netlify({ frameworksApi: false }).build({ root, outDir: '.ruvyxa' })
      const artifact = output.artifacts?.find((item) => item.kind === 'function')
      assert.ok(artifact && 'handlerSource' in artifact && artifact.handlerSource)

      const bundle = await deployFunction(root, {
        handlerSource: String(artifact.handlerSource),
        manifest: echoManifest(),
        routeModules: echoRouteModules(),
      })
      const handler = bundle.default as (
        request: Request,
        context: unknown,
      ) => Promise<Response> | Response

      // Netlify Functions v2 hands the function a Web Request and takes a Web
      // Response back, so the whole contract is exercisable here.
      const echoed = await handler(
        new Request('https://example.test/api/echo', {
          method: 'POST',
          headers: { 'content-type': 'text/plain' },
          body: 'streamed-payload',
        }),
        {},
      )
      assert.equal(echoed.status, 200)
      assert.equal(await echoed.text(), 'streamed-payload')
      assert.deepEqual(echoed.headers.getSetCookie(), ECHO_COOKIES)

      const binary = await handler(
        new Request('https://example.test/api/echo', {
          method: 'POST',
          headers: { 'x-binary': '1' },
        }),
        {},
      )
      assert.deepEqual(Buffer.from(await binary.arrayBuffer()), ECHO_BINARY_BODY)

      // A path the manifest does not carry must 404 rather than throw: the
      // platform turns an exception into a 500 with no diagnostic.
      const missing = await handler(
        new Request('https://example.test/api/absent', { method: 'POST' }),
        {},
      )
      assert.equal(missing.status, 404)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // The exported `config` is what Netlify reads to route requests to the
  // function at all, so an object shape it cannot parse means the function is
  // deployed and never invoked.
  it('exports a Functions v2 config the platform can route with', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-netlify-config-'))
    try {
      const output = await netlify({ frameworksApi: false }).build({ root, outDir: '.ruvyxa' })
      const artifact = output.artifacts?.find((item) => item.kind === 'function')
      assert.ok(artifact && 'handlerSource' in artifact && artifact.handlerSource)

      const bundle = await deployFunction(root, {
        handlerSource: String(artifact.handlerSource),
        manifest: echoManifest(),
        routeModules: echoRouteModules(),
      })

      const config = bundle.config as { path?: unknown; preferStatic?: unknown }
      assert.ok(config, 'the function must export a config object')
      assert.equal(config.path, '/*')
      assert.equal(config.preferStatic, true)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})

describe('netlify durable cache', () => {
  const handlerSource = async () => {
    const output = await netlify({ frameworksApi: false }).build({ root: '.', outDir: '.ruvyxa' })
    const artifact = output.artifacts?.find((item) => item.kind === 'function')
    return String((artifact as { handlerSource?: string } | undefined)?.handlerSource ?? '')
  }

  it('emits a handler node can parse', async () => {
    // The generated source is a template literal, so one unescaped backtick in
    // a comment ends the string early and emits JavaScript that does not parse.
    // Nothing else here would notice: every assertion below is a text match,
    // and a broken file matches text just as well as a working one.
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-netlify-parse-'))
    try {
      const file = path.join(root, 'index.mjs')
      await writeFile(file, await handlerSource())
      execFileSync(process.execPath, ['--check', file], { stdio: 'pipe' })
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('restates a cacheable answer for Netlify, durably and by tag', async () => {
    const source = await handlerSource()
    // `durable` is read by Netlify alone, which is why the header has to be
    // repeated rather than the existing `Cache-Control` reused.
    assert.match(source, /netlify-cdn-cache-control', 'public, durable, ' \+ cacheControl/)
    assert.match(source, /headers\.set\('cache-tag', cacheTag/)
    // Only a shared-cacheable response — never one rendered for one visitor.
    assert.match(source, /cacheControl\.includes\('s-maxage'\)/)
  })

  it('gives two different paths two different tags', async () => {
    // The tag was `pathname.replace(/[^A-Za-z0-9]+/g, '-')`, which is not a
    // mapping but a fold: `/a/b`, `/a-b` and `/a_b` all became `ruvyxa-a-b`, so
    // `revalidatePath('/a/b')` dropped the other two from Netlify's edge as
    // well. Nothing fails when that happens — the pages simply re-render, and
    // the site pays for renders nobody asked for.
    //
    // Run rather than matched: a text assertion passes just as happily against
    // a lossy expression as against an injective one.
    const source = await handlerSource()
    const body = /function cacheTag\(pathname\) \{[\s\S]*?\n\}/.exec(source)
    assert.ok(body, 'the emitted handler must declare cacheTag')
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-netlify-tag-'))
    try {
      const file = path.join(root, 'cache-tag.mjs')
      await writeFile(
        file,
        `import { createHash } from 'node:crypto'\n${body[0]}\nexport { cacheTag }\n`,
      )
      const { cacheTag } = (await import(pathToFileURL(file).href)) as {
        cacheTag: (pathname: string) => string
      }
      const paths = ['/', '/a/b', '/a-b', '/a_b', '/a.b', '/blog/hello-world', '/blog/hello_world']
      const tags = paths.map(cacheTag)
      assert.equal(new Set(tags).size, paths.length, tags.join(' '))
      // A tag list is comma-separated and the tag is one token.
      for (const tag of tags) assert.match(tag, /^ruvyxa-[0-9a-f]{32}$/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('purges the tag when revalidatePath() forces a write', async () => {
    const source = await handlerSource()
    assert.match(source, /forced === true\) return purgeDurableCache/)
    assert.match(source, /await import\('@netlify\/functions'\)/)
    // A project without the package must still boot: the import is where the
    // revalidation happens, not at module load.
    assert.doesNotMatch(source, /^import .*@netlify\/functions/m)
  })
})

describe('netlify project configuration', () => {
  const paths = async (options = {}) =>
    ((await netlify(options).build({ root: '.', outDir: '.ruvyxa' })).artifacts ?? []).map(
      (artifact) => artifact.path,
    )

  it('writes the root netlify.toml by default, because nothing else can name the publish directory', async () => {
    // The Frameworks API has no key for the publish directory, and a build
    // plugin cannot supply one without a netlify.toml of its own: Netlify
    // installs a plugin from its UI only when the plugin is in Netlify's own
    // directory. So the file is the mechanism, and generating it once beats
    // asking every project to write it by hand.
    assert.ok((await paths()).includes('netlify.toml'))
  })

  it('never ships both halves at once', async () => {
    // Two functions, both declaring `path: '/*'` — the one the netlify.toml
    // functions directory holds and the one under .netlify/v1/functions — is a
    // site where which of them answers a request has no defined answer.
    const byDefault = await paths()
    assert.equal(
      byDefault.some((entry) => entry.startsWith('.netlify/v1/')),
      false,
    )

    const frameworksApi = await paths({ projectConfig: false })
    assert.ok(frameworksApi.includes('.netlify/v1/functions/ruvyxa-handler'))
    assert.equal(frameworksApi.includes('netlify.toml'), false)

    // Both, only when the project insists — it has to be possible to say.
    const both = await paths({ frameworksApi: true })
    assert.ok(both.includes('netlify.toml'))
    assert.ok(both.includes('.netlify/v1/config.json'))
  })
})
