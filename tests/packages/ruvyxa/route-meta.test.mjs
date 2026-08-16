import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  clientEntrySource,
  metaSourceImports,
  nodeSsrEntrySource,
  routeMetaPrelude,
  routeTreeFunction,
} from '../../../packages/ruvyxa/runtime/entry-templates.mjs'

/**
 * Evaluate the emitted prelude against a minimal React stub.
 *
 * The helpers only ever call `createElement`, so a stub keeps this test on the
 * generated logic itself rather than on React's hoisting behavior, which React
 * already tests.
 */
function loadPrelude() {
  const React = {
    createElement: (type, props, ...children) => ({ type, props, children }),
  }
  const factory = new Function(
    'React',
    `${routeMetaPrelude()}\nreturn { __ruvyxaResolveMeta, __ruvyxaMetaElement, __ruvyxaApplyLang }`,
  )
  return { React, ...factory(React) }
}

/** Flatten the emitted metadata elements into `[type, props]` pairs. */
function emitted(elements) {
  if (!elements) return []
  return elements.map((child) => [child.type, child.props])
}

describe('route metadata resolution', () => {
  it('merges layouts root-to-leaf with the page winning', () => {
    const { __ruvyxaResolveMeta } = loadPrelude()
    const merged = __ruvyxaResolveMeta(
      [
        { meta: { title: 'Root', description: 'Root description', siteName: 'Ruvyxa' } },
        { meta: { title: 'Section' } },
        { meta: { title: 'Page', canonical: 'https://ruvyxa.dev/page' } },
      ],
      { path: '/page', params: {} },
    )

    assert.equal(merged.title, 'Page')
    assert.equal(merged.canonical, 'https://ruvyxa.dev/page')
    // A field the page did not set still comes from the layouts.
    assert.equal(merged.description, 'Root description')
    assert.equal(merged.siteName, 'Ruvyxa')
  })

  it('calls a meta function with the render context', () => {
    const { __ruvyxaResolveMeta } = loadPrelude()
    const merged = __ruvyxaResolveMeta([{ meta: ({ params }) => ({ title: params.slug }) }], {
      path: '/blog/hello',
      params: { slug: 'hello' },
    })
    assert.equal(merged.title, 'hello')
  })

  it('applies an ancestor titleTemplate but never a level to itself', () => {
    const { __ruvyxaResolveMeta } = loadPrelude()

    const child = __ruvyxaResolveMeta(
      [{ meta: { titleTemplate: '%s — Ruvyxa', title: 'Home' } }, { meta: { title: 'Docs' } }],
      { path: '/docs', params: {} },
    )
    assert.equal(child.title, 'Docs — Ruvyxa')

    // The layout declares both a template and its own title: a route that
    // renders only the layout's title must not be formatted by it, or every
    // untitled page would read "Home — Ruvyxa".
    const own = __ruvyxaResolveMeta([{ meta: { titleTemplate: '%s — Ruvyxa', title: 'Home' } }], {
      path: '/',
      params: {},
    })
    assert.equal(own.title, 'Home')
    assert.equal(own.titleTemplate, undefined)
  })

  it('ignores modules with no meta export and non-object metadata', () => {
    const { __ruvyxaResolveMeta } = loadPrelude()
    const merged = __ruvyxaResolveMeta(
      [{}, { meta: null }, { meta: 'nope' }, { meta: { title: 'Only' } }],
      {
        path: '/',
        params: {},
      },
    )
    assert.deepEqual(merged, { title: 'Only' })
  })
})

describe('route metadata elements', () => {
  it('emits the tags Lighthouse checks for', () => {
    const { __ruvyxaMetaElement } = loadPrelude()
    const tags = emitted(
      __ruvyxaMetaElement({
        title: 'Ruvyxa',
        description: 'The React framework with a native heart.',
        canonical: 'https://ruvyxa.dev/',
      }),
    )

    assert.deepEqual(tags.find(([type]) => type === 'title')[1].children, 'Ruvyxa')
    const description = tags.find(
      ([type, props]) => type === 'meta' && props.name === 'description',
    )
    assert.equal(description[1].content, 'The React framework with a native heart.')
    const canonical = tags.find(([type]) => type === 'link')
    assert.deepEqual([canonical[1].rel, canonical[1].href], ['canonical', 'https://ruvyxa.dev/'])
  })

  it('renders nothing when a route declares no metadata', () => {
    const { __ruvyxaMetaElement } = loadPrelude()
    assert.equal(__ruvyxaMetaElement({}), null)
    assert.equal(__ruvyxaMetaElement(null), null)
  })

  it('turns noindex into a robots directive and keeps an explicit one', () => {
    const { __ruvyxaMetaElement } = loadPrelude()
    const shorthand = emitted(__ruvyxaMetaElement({ title: 'Draft', noindex: true }))
    assert.equal(
      shorthand.find(([type, props]) => type === 'meta' && props.name === 'robots')[1].content,
      'noindex, nofollow',
    )

    const explicit = emitted(
      __ruvyxaMetaElement({ title: 'Draft', noindex: true, robots: 'noindex' }),
    )
    assert.equal(
      explicit.find(([type, props]) => type === 'meta' && props.name === 'robots')[1].content,
      'noindex',
    )
  })

  it('emits hreflang alternates and skips incomplete entries', () => {
    const { __ruvyxaMetaElement } = loadPrelude()
    const tags = emitted(
      __ruvyxaMetaElement({
        title: 'Docs',
        alternates: [{ hreflang: 'th', href: 'https://ruvyxa.dev/th' }, { hreflang: 'en' }, null],
      }),
    )
    const alternates = tags.filter(([type, props]) => type === 'link' && props.rel === 'alternate')
    assert.equal(alternates.length, 1)
    assert.deepEqual(
      [alternates[0][1].hrefLang, alternates[0][1].href],
      ['th', 'https://ruvyxa.dev/th'],
    )
  })

  it('derives social tags from the same fields', () => {
    const { __ruvyxaMetaElement } = loadPrelude()
    const tags = emitted(
      __ruvyxaMetaElement({
        title: 'Ruvyxa',
        description: 'Native heart.',
        canonical: 'https://ruvyxa.dev/',
        image: 'https://ruvyxa.dev/og.png',
        imageAlt: 'Ruvyxa',
        siteName: 'Ruvyxa',
      }),
    )
    const byProperty = (property) =>
      tags.find(([type, props]) => type === 'meta' && props.property === property)?.[1].content
    const byName = (name) =>
      tags.find(([type, props]) => type === 'meta' && props.name === name)?.[1].content

    assert.equal(byProperty('og:title'), 'Ruvyxa')
    assert.equal(byProperty('og:type'), 'website')
    assert.equal(byProperty('og:url'), 'https://ruvyxa.dev/')
    assert.equal(byProperty('og:image:alt'), 'Ruvyxa')
    // The option is `card`; the emitted attribute keeps the name X's crawler reads.
    assert.equal(byName('twitter:card'), 'summary_large_image')
    assert.equal(byName('twitter:image'), 'https://ruvyxa.dev/og.png')
  })

  it('gives every emitted element a key so React does not warn', () => {
    const { __ruvyxaMetaElement } = loadPrelude()
    const tags = emitted(
      __ruvyxaMetaElement({ title: 'A', description: 'B', canonical: 'https://a/' }),
    )
    const keys = tags.map(([, props]) => props.key)
    assert.ok(keys.every(Boolean), JSON.stringify(keys))
    assert.equal(new Set(keys).size, keys.length)
  })
})

describe('html lang rewriting', () => {
  it('replaces an existing lang attribute on the document element', () => {
    const { __ruvyxaApplyLang } = loadPrelude()
    const html = '<!doctype html><html lang="en"><head></head><body>สวัสดี</body></html>'
    assert.match(__ruvyxaApplyLang(html, 'th'), /<html lang="th">/)
  })

  it('adds lang when the document element has none, keeping other attributes', () => {
    const { __ruvyxaApplyLang } = loadPrelude()
    const html = '<!doctype html><html data-theme="dark"><body></body></html>'
    const output = __ruvyxaApplyLang(html, 'th')
    assert.match(output, /<html lang="th" data-theme="dark">/)
  })

  it('treats $-substitution characters in a lang value as literal text', () => {
    // `String.replace` reads `$&`, `` $` ``, `$'`, and `$1` out of a
    // *replacement string*, and the escaping above turns `&` into `&amp;` — it
    // cannot neutralize a `$`. Building the new tag by concatenation therefore
    // let a lang value substitute the matched `<html …>` tag into itself. `lang`
    // reaches this from a route parameter, so it is not developer-only input.
    const { __ruvyxaApplyLang } = loadPrelude()

    const replaced = __ruvyxaApplyLang('<html lang="en"><body></body></html>', '$&x')
    assert.match(replaced, /<html lang="\$&amp;x">/)
    assert.equal(replaced.match(/<html/g).length, 1, replaced)

    const added = __ruvyxaApplyLang('<html data-theme="dark"><body></body></html>', "$'y")
    assert.match(added, /<html lang="\$'y" data-theme="dark">/)
    assert.equal(added.match(/<html/g).length, 1, added)
  })

  it('escapes a lang value so it cannot close the attribute', () => {
    const { __ruvyxaApplyLang } = loadPrelude()
    const html = '<html lang="en"></html>'
    const output = __ruvyxaApplyLang(html, '"><script>alert(1)</script>')
    // The quote that would close the attribute and the tag opener that would
    // start a script are both neutralized; `>` inside a quoted value is inert.
    assert.doesNotMatch(output, /<script>/)
    assert.match(output, /lang="&quot;>&lt;script>/)
  })

  it('leaves the document alone when no lang is declared', () => {
    const { __ruvyxaApplyLang } = loadPrelude()
    const html = '<html lang="en"></html>'
    assert.equal(__ruvyxaApplyLang(html, undefined), html)
    assert.equal(__ruvyxaApplyLang(html, ''), html)
  })

  it('only touches the first html tag, not body text that mentions one', () => {
    const { __ruvyxaApplyLang } = loadPrelude()
    const html = '<html lang="en"><body>&lt;html lang="fr"&gt;</body></html>'
    const output = __ruvyxaApplyLang(html, 'th')
    assert.equal(output.match(/lang="th"/g).length, 1)
    assert.match(output, /&lt;html lang="fr"&gt;/)
  })
})

describe('metadata wiring into generated entries', () => {
  it('re-imports page and layouts as namespaces, least specific first', () => {
    const { imports, metaNames } = metaSourceImports([
      './layout.js',
      './section/layout.js',
      './page.js',
    ])
    assert.deepEqual(metaNames, ['__ruvyxaMeta0', '__ruvyxaMeta1', '__ruvyxaMeta2'])
    assert.deepEqual(imports, [
      'import * as __ruvyxaMeta0 from "./layout.js"',
      'import * as __ruvyxaMeta1 from "./section/layout.js"',
      'import * as __ruvyxaMeta2 from "./page.js"',
    ])
  })

  it('namespaces identifiers per route for a multi-route module', () => {
    const { metaNames } = metaSourceImports(['./page.js'], '__ruvyxaMeta3_')
    assert.deepEqual(metaNames, ['__ruvyxaMeta3_0'])
  })

  it('passes metadata as a provider child rather than wrapping the tree', () => {
    const tree = routeTreeFunction({
      name: '__ruvyxaTree',
      pageName: 'Page',
      layoutNames: ['Layout0'],
      routePath: '/',
      metaNames: ['__ruvyxaMeta0', '__ruvyxaMeta1'],
    })
    const layoutAt = tree.indexOf('[Layout0].reverse()')
    const metaAt = tree.indexOf('__ruvyxaMetaElement')
    assert.ok(layoutAt !== -1 && metaAt > layoutAt, tree)
    assert.match(tree, /__ruvyxaResolveMeta\(\[__ruvyxaMeta0, __ruvyxaMeta1\], ctx\)/)
    // No wrapper element per render, and no dependency on React.Fragment —
    // a generated entry runs against whatever React the app installed.
    assert.match(tree, /\}, __ruvyxaMetaElement\(.*\), tree\)/)
    assert.doesNotMatch(tree, /React\.Fragment/)
  })

  it('emits no metadata code for a caller that passes no sources', () => {
    const tree = routeTreeFunction({
      name: '__ruvyxaTree',
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
    })
    assert.doesNotMatch(tree, /__ruvyxaMeta/)

    const client = clientEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      requestPathLiteral: '"/"',
      paramsLiteral: '{}',
    })
    assert.doesNotMatch(client, /__ruvyxaResolveMeta/)
  })

  it('keeps the lang rewrite out of the browser bundle', () => {
    // The browser hydrates into a document whose lang the server already set,
    // so the helper would be dead bytes on every route bundle.
    const client = clientEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      requestPathLiteral: '"/"',
      paramsLiteral: '{}',
      metaNames: ['__ruvyxaMeta0'],
    })
    assert.match(client, /__ruvyxaMetaElement/)
    assert.doesNotMatch(client, /__ruvyxaApplyLang/)
  })

  it('applies lang once around every server render path', () => {
    const source = nodeSsrEntrySource({
      imports: ['import Page from "./page.js"'],
      pageName: 'Page',
      layoutNames: [],
      routePath: '/',
      metaNames: ['__ruvyxaMeta0'],
    })

    // One wrapper around the whole document keeps the streaming, non-streaming,
    // and recovery paths from each needing their own rewrite.
    assert.match(
      source,
      /export async function render\(ctx\) \{\n {2}const html = await __ruvyxaRenderDocument\(ctx\)/,
    )
    assert.match(
      source,
      /return __ruvyxaApplyLang\(html, __ruvyxaResolveMeta\(\[__ruvyxaMeta0\], ctx\)\.lang\)/,
    )
    assert.equal(source.match(/__ruvyxaApplyLang\(/g).length, 2) // definition + single call
  })
})
