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

describe('default image max width', () => {
  const table = fixture('dynamic-image-conformance.json')

  /**
   * `defaultMaxWidth` had a Rust replay and no JavaScript one.
   *
   * `dynamic_image.rs` asserts it, so the two Rust declarations stay level. The
   * JavaScript side had three more — one in `@ruvyxa/adapter-cloudflare`, two in
   * `@ruvyxa/adapter-vercel` — each written into a deployed function as a
   * literal, and each outside every gate. Raising the Rust default would have
   * left the deployed optimizer refusing widths the native host resizes, on
   * exactly the two targets that can optimize at all.
   */
  it('is one number, and every adapter that emits it reads that one', async () => {
    const { DEFAULT_IMAGE_MAX_WIDTH } = await import(
      `file://${path.join(workspaceRoot, 'packages/@ruvyxa/core/dist/utils.js').replaceAll('\\', '/')}`
    )
    assert.equal(DEFAULT_IMAGE_MAX_WIDTH, table.defaultMaxWidth)

    const emitted = await Promise.all(
      ['adapter-vercel', 'adapter-cloudflare'].map(async (name) => {
        const module = await import(
          `file://${path.join(workspaceRoot, 'packages/@ruvyxa', name, 'dist/index.js').replaceAll('\\', '/')}`
        )
        const output = await module.default().build({ root: '.', outDir: '.ruvyxa' })
        return [
          name,
          String(output.artifacts.find((item) => item.kind === 'function').handlerSource),
        ]
      }),
    )
    for (const [name, source] of emitted) {
      assert.match(
        source,
        new RegExp(`maxWidth \\?\\? ${table.defaultMaxWidth}`),
        `${name} must fall back to the shared default width`,
      )
      // And no other width literal pretending to be it.
      assert.equal(
        (source.match(/\?\? \d{4}\b/g) ?? []).every((hit) => hit === `?? ${table.defaultMaxWidth}`),
        true,
        `${name} carries a width default the fixture does not name`,
      )
    }
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

  /**
   * Which header names an adapter's emitted handler treats as its platform's
   * ingress, held to the shared contract in both directions.
   *
   * `clientIpHeaders` reached exactly two adapters — the two whose platform
   * headers were being *removed* from an unconditional list — and never the
   * other two serverless targets, which have documented ingress of their own.
   * Nothing asked an adapter to answer the question, so Netlify and Firebase
   * quietly collapsed every per-client control to one bucket. This is what asks.
   *
   * Both directions matter and for opposite reasons. A missing declaration
   * makes identity fall through to a right-to-left `X-Forwarded-For` scan on a
   * platform whose real client is not there; a declaration for a header the
   * platform does *not* overwrite is worse, because one client rotating a
   * fabricated value then gets a fresh bucket per request.
   */
  it('declares the ingress headers the shared contract names, and no others', () => {
    for (const adapter of fixture('adapter-contract.json').adapters) {
      assert.ok(
        Array.isArray(adapter.clientIpHeaders),
        `adapter ${adapter.name} must declare clientIpHeaders, even as an empty list`,
      )
      const source = adapterSource(adapter.name)
      const declared = [...source.matchAll(/clientIpHeaders: \[([^\]]*)\]/g)].map((match) =>
        [...match[1].matchAll(/'([^']+)'/g)].map((name) => name[1]),
      )
      // An adapter with two entry points declares the same list at each, and a
      // list stated twice differently is the divergence this is guarding.
      for (const list of declared) {
        assert.deepEqual(
          list,
          adapter.clientIpHeaders,
          `adapter ${adapter.name} passes clientIpHeaders the shared contract does not name`,
        )
      }
      assert.equal(
        declared.length > 0,
        adapter.clientIpHeaders.length > 0,
        adapter.clientIpHeaders.length > 0
          ? `adapter ${adapter.name} declares no clientIpHeaders, so every visitor shares one rate-limit bucket on a platform that has an ingress`
          : `adapter ${adapter.name} declares clientIpHeaders the contract says its platform does not write, which one client can rotate for a fresh bucket per request`,
      )
    }
  })
})

/**
 * `onDemandRevalidation` is a platform fact, so the fixture states it — but the
 * half of it this repository owns is checkable, and that half is what drifts.
 *
 * An adapter claims the capability either because the store its handler writes
 * *is* the cache a reader is served from, or because the handler implements the
 * platform's own purge. The second kind is code, and code that is deleted or
 * never written leaves the flag saying a deployment revalidates on demand when
 * it does not.
 */
describe('adapter on-demand revalidation', () => {
  const contract = fixture('adapter-contract.json')

  /** The adapters whose claim rests on a purge call rather than on their store. */
  const PURGES = new Map([
    ['vercel', /'x-prerender-revalidate'/],
    ['netlify', /purgeCache\(\{ tags:/],
  ])

  it('is declared for every adapter', () => {
    for (const adapter of contract.adapters) {
      assert.equal(
        typeof adapter.onDemandRevalidation,
        'boolean',
        `adapter ${adapter.name} must declare onDemandRevalidation`,
      )
    }
  })

  it('is earned by the adapters that implement a purge', () => {
    for (const [name, marker] of PURGES) {
      const adapter = contract.adapters.find((entry) => entry.name === name)
      assert.equal(adapter?.onDemandRevalidation, true, `${name} should claim the capability`)
      const source = adapterSource(name)
      assert.match(source, marker, `adapter-${name} no longer purges its platform's cache`)
      // A purge that is never reached is the same as no purge: only a forced
      // write may trigger one, and the handler has to be told which it is.
      //
      // The binding is deliberately not pinned. The writer used to be passed to
      // `createHandler` inline and is now a named `platformWritePrerendered`,
      // so a project's own `cache.handler` can stand in front of it — a change
      // to where the function is bound, not to what it is handed. Matching the
      // parameter list rather than the spelling is what keeps this assertion
      // about the claim it exists for.
      assert.match(
        source,
        /[Ww]ritePrerendered\s*[:=]\s*\(pathname, html, revalidate, forced\)/,
        `adapter-${name} must receive the forced flag to know when to purge`,
      )
    }
  })

  it('is refused by the adapters whose platform cache they cannot reach', () => {
    // Firebase Hosting and Amplify's CloudFront both cache the response and
    // expose no per-path purge inside the function, so neither may claim it —
    // and neither may quietly start claiming it without the code to back it up.
    for (const name of ['firebase', 'aws']) {
      const adapter = contract.adapters.find((entry) => entry.name === name)
      assert.equal(adapter?.onDemandRevalidation, false, `${name} cannot drop a cached document`)
      assert.doesNotMatch(
        adapterSource(name),
        /purgeCache|x-prerender-revalidate/,
        `adapter-${name} appears to purge now; the contract has to say so`,
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
