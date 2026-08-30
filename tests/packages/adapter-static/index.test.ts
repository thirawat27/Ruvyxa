import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { static as staticOutput } from '../../../packages/@ruvyxa/adapter-static/dist/index.js'

describe('static', () => {
  it('returns static deployment output', async () => {
    const output = await staticOutput().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'static-site', path: 'static' },
        { kind: 'file', path: 'static/_headers' },
      ],
    )

    // Without this file even the content-hashed bundles are served with a
    // revalidate-every-request default on hosts that read `_headers`.
    const headers = output.artifacts?.find((artifact) => artifact.kind === 'file')
    const contents = headers && 'contents' in headers ? String(headers.contents) : ''
    assert.match(contents, /\/__ruvyxa\/client\/\*\n {2}Cache-Control: public, max-age=31536000/)
    assert.match(contents, /\/\*\.png\n {2}Cache-Control: public, max-age=3600/)
    assert.match(contents, /\/\*\n {2}X-Content-Type-Options: nosniff/)
    assert.match(contents, / {2}X-Frame-Options: DENY/)

    assert.deepEqual(
      {
        name: output.name,
        target: output.target,
        platform: output.platform,
        entry: output.entry,
        assetsDir: output.assetsDir,
        clientDir: output.clientDir,
        chunkManifest: output.chunkManifest,
      },
      {
        name: 'static',
        target: 'static',
        platform: 'static',
        entry: '.ruvyxa/static',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
      },
    )
  })

  it('materializes custom output inside the build directory', async () => {
    const output = await staticOutput({ outputDir: 'deploy/public' }).build({
      root: '.',
      outDir: '.ruvyxa',
    })

    assert.equal(output.entry, '.ruvyxa/deploy/public')
    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'static-site', path: 'deploy/public' },
        { kind: 'file', path: 'deploy/public/_headers' },
      ],
    )
  })

  it('rejects output paths that escape the build directory', () => {
    assert.throws(() => staticOutput({ outputDir: '../public' }), /inside the build output/)
    assert.throws(() => staticOutput({ outputDir: 'C:\\public' }), /inside the build output/)
    assert.throws(() => staticOutput({ outputDir: 'assets' }), /overlaps protected build output/)
  })

  // The client build report sits at the build root as `client-report.json`
  // rather than inside the published `client/` directory, so `client` no longer
  // covers it: an `outputDir` named after it would have the static site
  // clobber the report the pre-renderer and every adapter function read.
  it('rejects an output directory that would clobber the client build report', () => {
    assert.throws(
      () => staticOutput({ outputDir: 'client-report.json' }),
      /overlaps protected build output/,
    )
    assert.throws(
      () => staticOutput({ outputDir: 'client-report.json/nested' }),
      /overlaps protected build output/,
    )
  })
})

/**
 * The refusal list and the build output it protects.
 *
 * `outputDir` may not be spelled after anything the build writes, or the static
 * site is written over the files the pre-renderer and every adapter function
 * read. The list lives in the adapter and the directories live in
 * `crates/ruvyxa_cli/src/build.rs`, so nothing connected the two: a directory
 * added to the build was writable from here until somebody noticed.
 *
 * Not an equality check, because the two sets are deliberately different. The
 * adapter's own destinations are in the build's list and must stay *writable*;
 * the compile cache is not in it and must stay protected. What has to hold is
 * the direction that matters: everything the build writes is refused unless it
 * is a destination this adapter tells authors to use.
 */
describe('the protected build output', () => {
  const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')

  /** A `const NAME: [&str; N] = [ … ];` list of string literals, from Rust. */
  function rustList(source: string, name: string): string[] {
    const start = source.indexOf(`const ${name}`)
    assert.notEqual(start, -1, `${name} is no longer declared in build.rs`)
    const open = source.indexOf('[', source.indexOf('=', start))
    const close = source.indexOf('];', open)
    assert.ok(close > open, `${name} is no longer a bracketed list`)
    return [...source.slice(open, close).matchAll(/"([^"]+)"/g)].map((match) => match[1])
  }

  /** The adapter's own list, read from its source rather than exported for this. */
  function adapterList(): string[] {
    const source = readFileSync(
      path.join(repoRoot, 'packages/@ruvyxa/adapter-static/src/index.ts'),
      'utf8',
    )
    const start = source.indexOf('const PROTECTED_BUILD_OUTPUT')
    assert.notEqual(start, -1, 'PROTECTED_BUILD_OUTPUT is no longer declared')
    const open = source.indexOf('[', start)
    const close = source.indexOf(']', open)
    return [...source.slice(open, close).matchAll(/'([^']+)'/g)].map((match) => match[1])
  }

  it('refuses everything the build writes, except this adapter’s own destinations', () => {
    const buildRs = readFileSync(path.join(repoRoot, 'crates/ruvyxa_cli/src/build.rs'), 'utf8')
    const dirs = rustList(buildRs, 'BUILD_OUTPUT_DIRS')
    const files = rustList(buildRs, 'BUILD_OUTPUT_FILES')
    const protectedNames = adapterList()

    assert.ok(dirs.length > 0 && files.length > 0, 'the Rust lists were not read')

    // Where adapter output is meant to go. Refusing these would refuse the
    // directory this adapter's own error message recommends.
    const destinations = new Set(['deploy', 'static'])

    const writable = [...dirs, ...files].filter(
      (name) => !destinations.has(name) && !protectedNames.includes(name),
    )
    assert.deepEqual(
      writable,
      [],
      'the build writes these and the static adapter would let an outputDir overwrite them',
    )

    // The compile cache is not in either Rust list and is protected anyway.
    assert.ok(protectedNames.includes('cache'), 'the compile cache has to stay protected')

    // And the destinations really are still writable, or the recommendation in
    // the RUV2001 message names a directory the adapter refuses.
    for (const destination of destinations) {
      assert.ok(
        !protectedNames.includes(destination),
        `${destination} is where adapter output goes; refusing it contradicts RUV2001`,
      )
    }
  })
})
