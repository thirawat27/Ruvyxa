/**
 * The shared tables the serverless handler keeps its own copy of.
 *
 * `tests/fixtures/static-asset-conformance.json` and
 * `tests/fixtures/security-headers-conformance.json` were each written because
 * a table existed in two languages and drifted. Both were then replayed by the
 * Rust host and by `@ruvyxa/core` — and neither by
 * `packages/ruvyxa/runtime/serverless-handler.mjs`, which holds a third copy of
 * each and is the code that actually runs in every deployed serverless build.
 *
 * A copy outside the gate is the arrangement the fixtures exist to prevent, so
 * the handler replays them here.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')

const { DEFAULT_SECURITY_HEADERS, isStaticAssetPath, createHandler } = await import(
  `file://${handlerPath.replaceAll('\\', '/')}`
)

const handlerSource = readFileSync(handlerPath, 'utf8')

function fixture(name) {
  return JSON.parse(readFileSync(path.join(workspaceRoot, 'tests/fixtures', name), 'utf8'))
}

describe('serverless handler static asset table', () => {
  const { staticAssetExtensions } = fixture('static-asset-conformance.json')

  it('recognises every extension the shared table names', () => {
    for (const extension of staticAssetExtensions) {
      assert.equal(
        isStaticAssetPath(`/assets/file.${extension}`),
        true,
        `.${extension} is in the shared table but this host routes it as a page`,
      )
    }
  })

  it('holds no extension the shared table does not name', () => {
    // Behaviour alone cannot see an extra entry — there is no candidate to
    // probe with — so the declared set is read from the source, the same way
    // the client bootstrap contract reads its Rust mirror.
    const declared = handlerSource.match(
      /const STATIC_ASSET_EXTENSIONS = new Set\(\[([\s\S]*?)\]\)/,
    )
    assert.ok(declared, 'STATIC_ASSET_EXTENSIONS not found in serverless-handler.mjs')
    const entries = [...declared[1].matchAll(/'([^']+)'/g)].map((match) => match[1])

    assert.deepEqual(
      [...entries].sort(),
      [...staticAssetExtensions].sort(),
      'an extension this host serves and the shared table omits makes the same URL a page elsewhere',
    )
  })

  it('still routes an extension nobody declared as a page', () => {
    // The table decides which misses are refused outright rather than handed to
    // a dynamic route. Treating an undeclared extension as an asset would let
    // `/[slug]` stop answering a real page whose slug happens to contain a dot.
    for (const pathname of ['/blog/v1.2.3', '/docs/readme.txt', '/x/file.unknown']) {
      assert.equal(isStaticAssetPath(pathname), false, pathname)
    }
  })
})

describe('serverless handler security header table', () => {
  const { headers } = fixture('security-headers-conformance.json')

  it('sends the shared default for every header the fixture names', () => {
    // Names are compared case-insensitively because HTTP header names are;
    // values exactly, because a changed value stops being strippable by the
    // hosts that only remove a header still holding its default.
    const declared = new Map(
      Object.entries(DEFAULT_SECURITY_HEADERS).map(([name, value]) => [name.toLowerCase(), value]),
    )

    for (const [name, value] of Object.entries(headers)) {
      assert.equal(
        declared.get(name.toLowerCase()),
        value,
        `${name} differs here from the shared default, so the same site is protected differently depending on where it is deployed`,
      )
    }
  })

  it('adds no default header the shared table does not name', () => {
    const expected = Object.keys(headers).map((name) => name.toLowerCase())
    assert.deepEqual(
      Object.keys(DEFAULT_SECURITY_HEADERS)
        .map((name) => name.toLowerCase())
        .sort(),
      [...expected].sort(),
    )
  })
})

describe('serverless handler dynamic image bounds', () => {
  const table = fixture('dynamic-image-conformance.json')

  /** A handler whose optimizer records what the endpoint resolved. */
  function imageHandler(options = {}) {
    const calls = []
    const handler = createHandler({
      routes: [],
      importPage: async () => ({}),
      importApi: async () => ({}),
      optimizeImage: async (_request, input) => {
        calls.push(input)
        return new Response('image', { headers: { 'content-type': 'image/webp' } })
      },
      ...options,
    })
    const request = (query) => handler(new Request(`https://example.test/__ruvyxa/image?${query}`))
    return { calls, request }
  }

  it('answers an absent q with the project quality the native host uses', async () => {
    // The value a project configures, carried here as `runtime.image.quality`.
    // `dynamic_image.rs` resolves the same absent parameter to the same number,
    // so one URL is one image wherever the project is deployed.
    const configured = table.quality.min + 1
    const { calls, request } = imageHandler({ imageQuality: configured })

    assert.equal((await request('src=%2Fhero.jpg&w=640')).status, 200)
    assert.deepEqual(calls, [{ src: '/hero.jpg', width: 640, quality: configured }])

    // An explicit q still wins; the configured value is a default, not a cap.
    await request(`src=%2Fhero.jpg&w=640&q=${table.quality.max}`)
    assert.equal(calls.at(-1).quality, table.quality.max)
  })

  it('falls back to the shared default when the manifest carries no quality', async () => {
    // A function bundle built before `runtime.image.quality` existed. The
    // fallback has to name the number that policy would have carried, or an
    // upgrade silently re-encodes at a quality nobody configured.
    const { calls, request } = imageHandler()
    await request('src=%2Fhero.jpg&w=640')
    assert.equal(calls.at(-1).quality, table.defaultQuality)
  })

  it('clamps a configured quality into the shared range rather than refusing', async () => {
    for (const [configured, expected] of [
      [table.quality.min - 1, table.quality.min],
      [table.quality.max + 1, table.quality.max],
    ]) {
      const { calls, request } = imageHandler({ imageQuality: configured })
      assert.equal((await request('src=%2Fhero.jpg&w=640')).status, 200)
      assert.equal(calls.at(-1).quality, expected, `configured ${configured}`)
    }
  })

  it('holds the shared width and quality bounds', async () => {
    const { calls, request } = imageHandler()
    const accepted = async (query) => (await request(query)).status === 200

    assert.equal(await accepted(`src=%2Fa.png&w=${table.width.min}`), true)
    assert.equal(await accepted(`src=%2Fa.png&w=${table.width.max}`), true)
    assert.equal(await accepted(`src=%2Fa.png&w=${table.width.min - 1}`), false)
    assert.equal(await accepted(`src=%2Fa.png&w=${table.width.max + 1}`), false)

    assert.equal(await accepted(`src=%2Fa.png&w=640&q=${table.quality.min}`), true)
    assert.equal(await accepted(`src=%2Fa.png&w=640&q=${table.quality.max}`), true)
    assert.equal(await accepted(`src=%2Fa.png&w=640&q=${table.quality.min - 1}`), false)
    assert.equal(await accepted(`src=%2Fa.png&w=640&q=${table.quality.max + 1}`), false)

    // A refused request never reaches the optimizer, so a platform is never
    // billed for a transform this host had already decided against.
    assert.equal(calls.length, 4)
  })

  it('publishes the quality from the build and reads it in every adapter that can optimize', () => {
    // Behaviour cannot see either half: the handler's fallback answers when the
    // value is missing, so a build that stops emitting it, or an adapter that
    // stops forwarding it, looks exactly like a project that configured 82. Both
    // ends are read from their own source instead, the same way the static-asset
    // table's membership is.
    const buildSource = readFileSync(
      path.join(workspaceRoot, 'crates/ruvyxa_cli/src/build.rs'),
      'utf8',
    )
    const imagePolicy = buildSource.match(/"image": \{([\s\S]*?)\n {16}\}/)
    assert.ok(imagePolicy, 'runtime image policy not found in build.rs')
    assert.match(
      imagePolicy[1],
      /"quality": config\.images\.quality/,
      'ruvyxa build stopped publishing image.quality, so every deployment falls back to the constant',
    )

    for (const { name } of adaptersServingImages()) {
      const source = adapterSource(name)
      const optimizers = source.match(/optimizeImage: runtimePolicy\.image\?\.onDemand/g) ?? []
      const forwarded = source.match(/imageQuality: runtimePolicy\.image\?\.quality/g) ?? []
      assert.equal(
        forwarded.length,
        optimizers.length,
        `adapter-${name} generates ${optimizers.length} handler(s) that can optimize but forwards the configured quality to ${forwarded.length}`,
      )
      assert.ok(optimizers.length > 0, `adapter-${name} no longer supplies an optimizer`)
    }
  })

  it('records onDemandImages for exactly the adapters that pass an optimizer', () => {
    // `onDemandImages` in the adapter contract is what `ruvyxa build`'s warning,
    // `ruvyxa test:parity`, and the generated matrix in both language guides all
    // read. It was not a field at all, and the answer was restated as "no
    // adapter serves this" in three places while two adapters did — so a Vercel
    // project was told its working responsive images answered 404. Only the
    // adapter source decides it, so the table is compared with the source.
    const contract = fixture('adapter-contract.json')
    for (const adapter of contract.adapters) {
      assert.equal(
        typeof adapter.onDemandImages,
        'boolean',
        `adapter ${adapter.name} must declare onDemandImages`,
      )
      const source = adapterSource(adapter.name)
      const passesOptimizer = source.includes('optimizeImage: runtimePolicy.image?.onDemand')
      assert.equal(
        adapter.onDemandImages,
        passesOptimizer,
        passesOptimizer
          ? `adapter ${adapter.name} passes an optimizer but the contract says it serves no on-demand images, so the build warns a working deployment is broken`
          : `adapter ${adapter.name} passes no optimizer but the contract claims it serves on-demand images, so nothing warns that /__ruvyxa/image answers 404 there`,
      )
    }
  })
})

/** The adapter package source that decides whether it can optimize. */
function adapterSource(name) {
  return readFileSync(
    path.join(workspaceRoot, 'packages/@ruvyxa', `adapter-${name}`, 'src/index.ts'),
    'utf8',
  )
}

function adaptersServingImages() {
  return fixture('adapter-contract.json').adapters.filter(
    (adapter) => adapter.onDemandImages === true,
  )
}
