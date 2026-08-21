/**
 * The generated-entry preludes, held to one behaviour across both bundlers.
 *
 * A route's client entry is emitted by whichever module graph built it — the
 * Rust bundler in `crates/ruvyxa_bundler/src/output.rs` for `ruvyxa build`, and
 * `packages/ruvyxa/runtime/entry-templates.mjs` for the Node path — so the two
 * carry hand-maintained copies of the same source text. Only
 * `CLIENT_BOOTSTRAP_PRELUDE` was gated (`client-bootstrap.test.mjs`); the
 * routing context and the error / not-found boundary were held by a doc comment
 * on each side saying they mirrored each other, which is the arrangement
 * `AGENTS.md` names as the thing that drifts.
 *
 * Behaviour rather than bytes: the two copies differ in statement terminators
 * (the Rust literal is written with semicolons, the JavaScript template without
 * them, and Prettier owns the second), and a byte comparison would fail on the
 * formatting while passing on a boundary that swallowed an error. Both sources
 * are executed against the same stand-in React and asked the same questions.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  routeBoundaryPrelude,
  routeContextPrelude,
  routeMetaPrelude,
  routeShellFunction,
  wrapperLevels,
  wrapperLoop,
  routeTreeFunction,
} from '../../../packages/ruvyxa/runtime/entry-templates.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const outputRs = readFileSync(
  path.join(workspaceRoot, 'crates/ruvyxa_bundler/src/output.rs'),
  'utf8',
)

/** Read a `const NAME: &str = r#"..."#;` raw literal out of the Rust bundler. */
function rustRawLiteral(name) {
  const match = new RegExp(`const ${name}: &str = r#"([\\s\\S]*?)"#;`).exec(outputRs)
  assert.ok(match, `${name} not found in output.rs`)
  return match[1]
}

/** Read a `const NAME: &str = "...";` literal. Rejects escapes rather than guessing. */
function rustStringLiteral(name) {
  const match = new RegExp(`const ${name}: &str = "([^"]*)";`).exec(outputRs)
  assert.ok(match, `${name} not found in output.rs`)
  assert.doesNotMatch(match[1], /\\/, `${name} grew an escape this reader does not decode`)
  return match[1]
}

/**
 * Enough React for a prelude to run against.
 *
 * The preludes are emitted into a bundle that has React in scope and use only
 * `Component` and `createElement`; anything more here would be testing this
 * file rather than them.
 */
function stubReact() {
  return {
    Component: class {
      constructor(props) {
        this.props = props
      }
      setState(next) {
        this.state = { ...this.state, ...(typeof next === 'function' ? next(this.state) : next) }
      }
    },
    createElement: (type, props) => ({ type, props }),
  }
}

const HOSTS = {
  'the Node entry templates': {
    context: routeContextPrelude(),
    boundary: routeBoundaryPrelude(),
    meta: routeMetaPrelude({ lang: false }),
    metaWithLang: routeMetaPrelude({ lang: true }),
  },
  'the Rust bundler': {
    context: rustStringLiteral('ROUTE_CONTEXT_PRELUDE'),
    boundary: rustRawLiteral('ROUTE_BOUNDARY_PRELUDE'),
    // The Rust side keeps the `<html lang>` rewrite in its own literal and
    // appends it to server entries only, which is the same shape the `lang`
    // option produces on the Node side.
    meta: rustRawLiteral('META_PRELUDE'),
    metaWithLang: rustRawLiteral('META_PRELUDE') + rustRawLiteral('META_LANG_PRELUDE'),
  },
}

for (const [host, preludes] of Object.entries(HOSTS)) {
  describe(`route context prelude emitted by ${host}`, () => {
    const source = preludes.context

    function run(globals) {
      const React = stubReact()
      let created = 0
      React.createContext = (value) => ({ __context: true, value, id: (created += 1) })
      new Function('React', 'globalThis', source)(React, globals)
      return globals
    }

    it('publishes the context on globalThis rather than importing it', () => {
      // A generated entry cannot depend on @ruvyxa/react: an application may
      // render plain React pages and never install it.
      assert.doesNotMatch(source, /\bimport\b/)
      const globals = run({})
      assert.equal(globals.__RUVYXA_ROUTE_CONTEXT__?.__context, true)
    })

    it('reuses a context another bundle already published', () => {
      // Two route bundles evaluate this in the same document. Creating a second
      // context would leave a layout providing one and a page reading another.
      const existing = { __context: true, value: null, id: 'existing' }
      const globals = run({ __RUVYXA_ROUTE_CONTEXT__: existing })
      assert.equal(globals.__RUVYXA_ROUTE_CONTEXT__, existing)
    })
  })

  describe(`route boundary prelude emitted by ${host}`, () => {
    const source = preludes.boundary

    function boundaryClass(globals = {}) {
      const React = stubReact()
      const factory = new Function('React', 'globalThis', `${source}\nreturn __ruvyxaBoundary`)
      return { Boundary: factory(React, globals), React }
    }

    function mounted(props, globals = {}) {
      const { Boundary, React } = boundaryClass(globals)
      const instance = new Boundary(props)
      instance.state = { error: null }
      return { instance, React, Boundary }
    }

    it('derives its state from a thrown error', () => {
      const { Boundary } = boundaryClass()
      const error = new Error('render failed')
      assert.deepEqual(Boundary.getDerivedStateFromError(error), { error })
    })

    it('renders its children while nothing has thrown', () => {
      const { instance } = mounted({ children: 'CHILD' })
      assert.equal(instance.render(), 'CHILD')
    })

    it('renders the not-found special for a notFound() signal', () => {
      // `notFound()` stamps an own property rather than throwing a subclass,
      // because the signal crosses a bundle boundary where `instanceof` fails.
      const NotFound = () => null
      const { instance } = mounted({ children: 'CHILD', notFound: NotFound })
      instance.state = { error: Object.assign(new Error('nf'), { __ruvyxaNotFound: true }) }
      assert.deepEqual(instance.render(), { type: NotFound, props: null })
    })

    it('rethrows a notFound() signal no route can render', () => {
      // Swallowing it would answer 200 with an empty page instead of letting an
      // ancestor boundary, or the server, turn it into a 404.
      const error = Object.assign(new Error('nf'), { __ruvyxaNotFound: true })
      const { instance } = mounted({ children: 'CHILD' })
      instance.state = { error }
      assert.throws(() => instance.render(), error)
    })

    it('hands the error fallback an error, a reset, and a retry', () => {
      const Fallback = () => null
      const error = new Error('boom')
      const { instance } = mounted({ children: 'CHILD', errorFallback: Fallback })
      instance.state = { error }

      const element = instance.render()
      assert.equal(element.type, Fallback)
      assert.equal(element.props.error, error)
      assert.equal(typeof element.props.reset, 'function')
      assert.equal(typeof element.props.retry, 'function')
    })

    it('rethrows an ordinary error when the route declared no fallback', () => {
      const error = new Error('boom')
      const { instance } = mounted({ children: 'CHILD' })
      instance.state = { error }
      assert.throws(() => instance.render(), error)
    })

    it('clears the error on reset', () => {
      const { instance } = mounted({ children: 'CHILD' })
      instance.state = { error: new Error('boom') }
      instance.reset()
      assert.equal(instance.state.error, null)
      assert.equal(instance.render(), 'CHILD')
    })

    it('re-fetches the route through the router before clearing', async () => {
      // A plain reset re-renders the payload that just failed, so it recovers
      // only from a fault in the client tree. A page whose data failed to load
      // needs the request repeated.
      let retried = 0
      const globals = { __RUVYXA_ROUTER_INSTANCE__: { retry: () => (retried += 1) } }
      const { instance } = mounted({ children: 'CHILD' }, globals)
      instance.state = { error: new Error('boom') }

      await instance.retry()
      assert.equal(retried, 1)
      assert.equal(instance.state.error, null)
    })

    it('degrades to a plain reset with no router mounted', () => {
      // Server-rendered pages and tests run this without a router; doing
      // nothing at all would leave the boundary stuck on its error.
      const { instance } = mounted({ children: 'CHILD' }, {})
      instance.state = { error: new Error('boom') }

      const result = instance.retry()
      assert.ok(result instanceof Promise)
      assert.equal(instance.state.error, null)
    })

    it('keeps the boundary up when the retry itself fails', async () => {
      const failure = new Error('retry failed')
      const globals = {
        __RUVYXA_ROUTER_INSTANCE__: { retry: () => Promise.reject(failure) },
      }
      const { instance } = mounted({ children: 'CHILD' }, globals)
      instance.state = { error: new Error('boom') }

      await instance.retry()
      assert.equal(instance.state.error, failure)
    })
  })

  describe(`route metadata prelude emitted by ${host}`, () => {
    /**
     * Load a metadata prelude and hand back the functions it declares.
     *
     * The prelude is source text destined for a bundle that has React in
     * scope, so running it is the only check that means anything: matching it
     * against a regular expression would pass on a prelude that merged
     * metadata in the wrong order.
     */
    function metaHelpers(source) {
      const React = stubReact()
      const factory = new Function(
        'React',
        `${source}\nreturn { resolve: __ruvyxaResolveMeta, element: __ruvyxaMetaElement, applyLang: typeof __ruvyxaApplyLang === "function" ? __ruvyxaApplyLang : null }`,
      )
      return factory(React)
    }

    const { resolve, element } = metaHelpers(preludes.meta)
    const source = (meta) => ({ meta })

    it('merges root layout to page, most specific last', () => {
      const merged = resolve(
        [source({ title: 'Site', description: 'Root' }), source({ title: 'Page' })],
        {},
      )
      assert.equal(merged.title, 'Page')
      assert.equal(merged.description, 'Root')
    })

    it('lets an undeclared key fall through rather than clearing it', () => {
      // `undefined` is what an optional field reads as; treating it as a value
      // would let a page erase the description its layout set.
      const merged = resolve(
        [source({ description: 'Root' }), source({ description: undefined })],
        {},
      )
      assert.equal(merged.description, 'Root')
    })

    it('resolves a meta function against the render context', () => {
      const merged = resolve([source((ctx) => ({ title: ctx.params.slug }))], {
        params: { slug: 'a' },
      })
      assert.equal(merged.title, 'a')
    })

    it('applies a title template only to a title declared below it', () => {
      // A layout template formats the titles of its pages, not its own —
      // otherwise the layout title comes out as "Site | Site".
      const withPage = resolve(
        [source({ title: 'Site', titleTemplate: '%s | Site' }), source({ title: 'Blog' })],
        {},
      )
      assert.equal(withPage.title, 'Blog | Site')

      const withoutPage = resolve([source({ title: 'Site', titleTemplate: '%s | Site' })], {})
      assert.equal(withoutPage.title, 'Site')
    })

    it('drops the template from the resolved metadata', () => {
      const merged = resolve(
        [source({ titleTemplate: '%s | Site' }), source({ title: 'Blog' })],
        {},
      )
      assert.equal(merged.titleTemplate, undefined)
    })

    it('substitutes a title containing a dollar pattern literally', () => {
      // `String.replace` reads `$&` in the replacement as the whole match. A
      // title is user content, so the replacement goes through a function.
      const merged = resolve(
        [source({ titleTemplate: '%s | Site' }), source({ title: '$& and $1' })],
        {},
      )
      assert.equal(merged.title, '$& and $1 | Site')
    })

    it('emits nothing for metadata with no renderable field', () => {
      assert.equal(element(null), null)
      assert.equal(element({}), null)
    })

    it('emits the document title, description, and canonical link', () => {
      const children = element({ title: 'Hello', description: 'About', canonical: 'https://x/y' })
      assert.equal(children.find((child) => child.type === 'title').props.children, 'Hello')
      assert.ok(children.some((child) => child.type === 'link' && child.props.rel === 'canonical'))
      assert.ok(children.some((child) => child.props?.name === 'description'))
    })

    it('turns noindex into a robots directive', () => {
      const children = element({ title: 'Hello', noindex: true })
      assert.equal(
        children.find((child) => child.props?.name === 'robots').props.content,
        'noindex, nofollow',
      )
    })

    it('prefers an explicit robots value over noindex', () => {
      const children = element({ title: 'Hello', noindex: true, robots: 'index, follow' })
      assert.equal(
        children.find((child) => child.props?.name === 'robots').props.content,
        'index, follow',
      )
    })

    it('emits one alternate link per well-formed entry and skips the rest', () => {
      const children = element({
        title: 'Hello',
        alternates: [{ href: 'https://x/th', hreflang: 'th' }, { href: 'https://x/missing' }, null],
      })
      const alternates = children.filter((child) => child.props?.rel === 'alternate')
      assert.equal(alternates.length, 1)
      assert.equal(alternates[0].props.hrefLang, 'th')
    })

    it('picks the twitter card size from whether an image is present', () => {
      const card = (meta) =>
        element(meta).find((child) => child.props?.name === 'twitter:card').props.content
      assert.equal(card({ title: 'Hello', image: 'https://x/i.png' }), 'summary_large_image')
      assert.equal(card({ title: 'Hello' }), 'summary')
    })

    it('gives every emitted element a distinct key', () => {
      // React warns on duplicate keys, and these are passed as an array.
      const children = element({ title: 'Hello', description: 'About', image: 'https://x/i.png' })
      const keys = children.map((child) => child.props.key)
      assert.equal(new Set(keys).size, keys.length)
    })

    describe('with the server-only language rewrite', () => {
      const { applyLang } = metaHelpers(preludes.metaWithLang)

      it('is absent from the client half of the prelude', () => {
        // Shipping the rewrite to the browser would be dead bytes on every
        // route bundle: only a server entry has a document string to rewrite.
        assert.equal(metaHelpers(preludes.meta).applyLang, null)
        assert.equal(typeof applyLang, 'function')
      })

      it('replaces an existing lang attribute', () => {
        assert.equal(
          applyLang('<!doctype html><html lang="en"><head></head></html>', 'th'),
          '<!doctype html><html lang="th"><head></head></html>',
        )
      })

      it('adds a lang attribute when the tag carries none', () => {
        assert.match(applyLang('<html data-x="1"><head></head></html>', 'th'), /<html lang="th"/)
      })

      it('escapes a locale that would otherwise close the attribute', () => {
        // The locale reaches here from a route parameter. `"` would close the
        // attribute and `<` would open a tag; `>` inside a quoted value is
        // inert, which is why it is left alone.
        const html = applyLang('<html lang="en"></html>', '"><script>alert(1)</script>')
        assert.doesNotMatch(html, /<script>/)
        assert.equal(html, '<html lang="&quot;>&lt;script>alert(1)&lt;/script>"></html>')
      })

      it('leaves a document it cannot rewrite untouched', () => {
        assert.equal(applyLang('<p>no html tag</p>', 'th'), '<p>no html tag</p>')
        assert.equal(applyLang('<html></html>', ''), '<html></html>')
        assert.equal(applyLang(null, 'th'), null)
      })
    })
  })
}

/**
 * Strip statement terminators so the two generators can be compared.
 *
 * The Rust literals carry semicolons and the JavaScript templates do not —
 * Prettier owns the second and neither reaches a reader. Only a `;` that ends a
 * line is removed, which is why the fixture forbids one inside a route path.
 */
function withoutTerminators(source) {
  return source.replace(/;$/gm, '')
}

describe('route composition emitted by the Node entry templates', () => {
  const fixture = JSON.parse(
    readFileSync(
      path.join(workspaceRoot, 'tests/fixtures/entry-composition-conformance.json'),
      'utf8',
    ),
  )

  it('names the identifiers the Rust bundler hardcodes', () => {
    // The Node generator takes these as arguments; the Rust one writes them
    // into its format strings. The fixture is where the two meet.
    const outputRsNames = Object.values(fixture.names)
    for (const name of outputRsNames) {
      assert.ok(outputRs.includes(name), `output.rs never emits ${name}`)
    }
  })

  for (const testCase of fixture.cases) {
    it(testCase.$why, () => {
      const { input, names } = { ...testCase, names: fixture.names }
      const generated =
        testCase.kind === 'tree'
          ? routeTreeFunction({
              name: names.tree,
              pageName: names.page,
              layoutNames: input.layoutNames,
              routePath: input.routePath,
              metaNames: input.metaNames,
              errorName: input.errorName,
              loadingName: input.loadingName,
              notFoundName: input.notFoundName,
              levels: input.wrapperLevels ?? [],
            })
          : routeShellFunction({
              name: names.shell,
              layoutNames: input.layoutNames,
              routePath: input.routePath,
              loadingName: input.loadingName,
              metaNames: input.metaNames,
              levels: input.wrapperLevels ?? [],
            })

      assert.equal(withoutTerminators(generated), testCase.source.join('\n'))
      assert.doesNotMatch(
        input.routePath,
        /;/,
        'a route path with a semicolon breaks the comparison',
      )
    })
  }

  it('merges a layout and a template on the directory that holds them', () => {
    // The rule that keeps `layout > template` correct at every level.
    // Flattening it into "every template inside every layout" is the tempting
    // shortcut and it is wrong: Layout1 would end up outside Template0, when it
    // belongs inside it, and a template providing context would stop reaching
    // the layout beneath it.
    //
    // Held to the same answer as `route_wrapper_levels()` in
    // `crates/ruvyxa_bundler/src/output.rs`; the two generators emit the same
    // bundle for the same route and a project renders through whichever built it.
    assert.deepEqual(
      wrapperLevels(
        ['/p/app/layout.tsx', '/p/app/dash/layout.tsx'],
        ['/p/app/template.tsx', '/p/app/dash/reports/template.tsx'],
      ),
      [
        { layout: 'Layout0', template: 'Template0', slots: null },
        { layout: 'Layout1', template: null, slots: null },
        { layout: null, template: 'Template1', slots: null },
      ],
    )

    // Windows separators name the same directories.
    assert.deepEqual(
      wrapperLevels(['C:\\p\\app\\layout.tsx'], ['C:\\p\\app\\template.tsx']),
      [{ layout: 'Layout0', template: 'Template0', slots: null }],
      'a Windows separator names the same directory a forward slash does',
    )
  })

  it('leaves a route without templates on the loop it always had', () => {
    // The feature existing must not change one byte of an ordinary route's
    // bundle.
    assert.equal(
      wrapperLoop(
        ['Layout0', 'Layout1'],
        wrapperLevels(['/p/app/layout.tsx', '/p/app/a/layout.tsx'], []),
      ),
      `  for (const Layout of [Layout0, Layout1].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }`,
    )
  })

  it('composes the boundary inside the Suspense, not around it', () => {
    // Executed rather than matched: a synchronous throw has to reach the
    // boundary before React sees a suspended subtree, or an error page renders
    // as the loading fallback instead.
    const full = fixture.cases.find((entry) => entry.input.errorName && entry.input.loadingName)
    const lines = full.source
    const boundaryLine = lines.findIndex((line) => line.includes(fixture.names.boundary))
    const suspenseLine = lines.findIndex((line) => line.includes('React.Suspense'))
    const layoutLine = lines.findIndex((line) => line.includes('.reverse()'))
    const providerLine = lines.findIndex((line) =>
      line.includes(`${fixture.names.context}.Provider`),
    )

    assert.ok(boundaryLine > 0 && boundaryLine < suspenseLine, 'boundary must wrap before Suspense')
    assert.ok(suspenseLine < layoutLine, 'Suspense must wrap before the layouts')
    assert.ok(layoutLine < providerLine, 'the provider is outermost')
  })

  it('passes metadata as a sibling of the layouts rather than a wrapper', () => {
    // A layout that suspends must not be able to hold the document title back
    // past the flushed shell, so the elements go in as an extra provider child.
    const withMeta = fixture.cases.find((entry) => entry.input.metaNames.length > 0)
    const providerCall = withMeta.source.at(-2)
    assert.match(providerCall, new RegExp(`\\}, ${fixture.names.metaElement}\\(.*\\), tree\\)$`))
  })
})
