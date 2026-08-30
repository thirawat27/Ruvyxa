/**
 * What a click on `<Link>` hands to the browser, what it takes over, and what
 * it is willing to put in the `href` attribute in the first place.
 *
 * `<Link>` renders a real `<a href>`, so every href the browser has always
 * understood must keep working. The component only takes a click over when the
 * router can actually answer it: the moment it calls `preventDefault()` for a
 * navigation the router will then refuse, the link does nothing at all — no
 * soft navigation, and no browser navigation either.
 *
 * The attribute is its own sink, and a separate question from the click. A
 * `javascript:` href executes on middle-click, on a keyboard activation, and
 * before this file's `onClick` is even attached, so the click decision cannot
 * be the thing that closes it — only refusing to render the attribute can.
 *
 * Driven by calling the component and invoking the `onClick` it returns. There
 * is no DOM in this suite and none is needed: the click decision is which of
 * `preventDefault()` and `location.assign()` the handler reaches, and the
 * attribute decision is a prop on the element the component returned.
 */

import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import * as React from 'react'

import { Link } from '../dist/link.js'

// The refusal warning is development-only, and this suite asserts it fires.
// Read lazily by the component, so setting it here is in time; pinned because
// a suite whose answer depends on the shell that launched it is not a test.
process.env.NODE_ENV = 'development'

const internals = React.__CLIENT_INTERNALS_DO_NOT_USE_OR_WARN_USERS_THEY_CANNOT_UPGRADE

/**
 * The three hooks `<Link>` calls, with no renderer behind them.
 *
 * `useRef` is fresh per render, `useCallback` returns the callback it was
 * given, and `useEffect` never runs — a single render with no state changes
 * needs nothing more, and a real renderer would only add a DOM dependency.
 */
const HOOKS = {
  useRef: (initial) => ({ current: initial }),
  useCallback: (callback) => callback,
  useEffect: () => {},
}

/** Render `<Link {...props} />` once and return the `<a>` element it produced. */
function renderLink(props) {
  const previous = internals.H
  internals.H = HOOKS
  try {
    return Link({ children: 'label', ...props })
  } finally {
    internals.H = previous
  }
}

/**
 * Render `<Link href={href} />` and report the attribute it produced.
 *
 * Deliberately outside any `window` stub: this is the server render, where the
 * attribute is written into HTML that a browser will honour long before the
 * router exists to have an opinion about it.
 */
function renderedHref(href, props = {}) {
  const errors = []
  const previousError = console.error
  console.error = (...args) => errors.push(args.join(' '))
  try {
    return { href: renderLink({ href, ...props }).props.href, errors }
  } finally {
    console.error = previousError
  }
}

/** A left-click with no modifiers: the only kind `<Link>` may take over. */
function clickEvent(overrides = {}) {
  const event = {
    button: 0,
    defaultPrevented: false,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    prevented: 0,
    preventDefault() {
      this.prevented += 1
      this.defaultPrevented = true
    },
    ...overrides,
  }
  return event
}

/** Install a minimal browser-ish global environment and restore it after. */
function withGlobals(values, run) {
  const keys = [
    'window',
    'fetch',
    '__RUVYXA_ROUTES__',
    '__RUVYXA_ROOT__',
    '__RUVYXA_ROUTE_MANIFEST__',
    '__RUVYXA_ROUTE_PARAMS__',
    '__RUVYXA_REQUEST_PATH__',
    '__RUVYXA_ROUTE_PATTERN__',
    '__RUVYXA_ROUTE_ARTIFACTS__',
    '__RUVYXA_ROUTER_INSTANCE__',
    '__RUVYXA_INTERCEPTS__',
  ]
  const previous = new Map(
    keys.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]),
  )
  for (const key of keys) delete globalThis[key]
  Object.assign(globalThis, values)
  const restore = () => {
    for (const key of keys) delete globalThis[key]
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor)
    }
  }
  try {
    const result = run()
    if (result && typeof result.then === 'function') return result.finally(restore)
    restore()
    return result
  } catch (error) {
    restore()
    throw error
  }
}

/**
 * Click a `<Link>` in a document at `https://example.test/`, and report
 * everything the click was observed to do.
 *
 * The real router singleton is behind this — `<Link>` calls
 * `getRouterInstance()` itself — so the answer covers the component and the
 * router's own classification together, which is where the defect lived.
 */
async function clickLink(props, eventOverrides) {
  const assigned = []
  const replaced = []
  const errors = []
  const previousError = console.error
  console.error = (...args) => errors.push(args.join(' '))
  const event = clickEvent(eventOverrides)
  const element = renderLink(props)

  try {
    await withGlobals(
      {
        window: {
          location: {
            href: 'https://example.test/',
            origin: 'https://example.test',
            pathname: '/',
            search: '',
            assign: (href) => assigned.push(href),
            replace: (href) => replaced.push(href),
          },
          history: { pushState() {}, replaceState() {}, back() {}, forward() {} },
          addEventListener() {},
          scrollTo() {},
        },
        // The route table is never reachable in this suite, which is what
        // makes an internal navigation fall through to a document load.
        fetch: () => Promise.reject(new Error('offline')),
        __RUVYXA_ROUTES__: {},
        __RUVYXA_ROOT__: { render() {} },
        __RUVYXA_ROUTE_MANIFEST__: { routes: [] },
      },
      async () => {
        element.props.onClick(event)
        // Two turns: `navigate` awaits the manifest request before it can fall
        // through to `location.assign`.
        await new Promise((resolve) => setTimeout(resolve, 0))
      },
    )
  } finally {
    console.error = previousError
  }

  return { assigned, replaced, errors, prevented: event.prevented, href: element.props.href }
}

describe('a Link click the router cannot answer', () => {
  // Regression: the handler called `preventDefault()` and *then* asked the
  // router to navigate. Once the router grew an allow-list of schemes, a
  // left-click on a link outside it suppressed the browser's own handling and
  // then refused — so the link did nothing at all, where before the fix the
  // browser had navigated.
  for (const href of ['web+foo:open', 'ircs://a.test/b']) {
    it(`leaves ${href} to the browser`, async () => {
      const result = await clickLink({ href })

      assert.equal(result.prevented, 0, 'the browser must keep its own handling of this href')
      assert.deepEqual(result.assigned, [], 'the router must not navigate a scheme it refuses')
      assert.equal(result.href, href, 'the anchor still carries the href verbatim')
      assert.deepEqual(
        result.errors,
        [],
        'nothing was refused: the browser handled the click, so the console stays quiet',
      )
    })
  }

  // The executable schemes are the exception, and the reason this suite grew a
  // second question. Handing the click back to the browser is the right answer
  // for `web+foo:` and wrong for `javascript:` — "the browser handles it" is
  // exactly the thing being prevented. The anchor carries no href, so there is
  // nothing left for the browser to handle.
  for (const href of ['javascript:void 0', 'data:text/plain,x']) {
    it(`renders no href at all for ${href}`, async () => {
      const result = await clickLink({ href })

      assert.equal(result.href, undefined, 'an executable href must not reach the anchor')
      assert.deepEqual(result.assigned, [], 'the router must not navigate a scheme it refuses')
      assert.equal(result.prevented, 0)
    })
  }
})

describe('the href a Link is willing to render', () => {
  // Every one distinct, because the warning fires once per href and a shared
  // string would make one test depend on whether another ran first.
  const refused = [
    ['a javascript: URL', 'javascript:alert(1)'],
    ['a mixed-case scheme', 'JavaScript:alert(2)'],
    ['a leading-whitespace scheme', '  javascript:alert(3)'],
    ['a tab-split scheme', 'jav\tascript:alert(4)'],
    ['a data: URL with no download', 'data:text/html,<script>alert(5)</script>'],
    ['a vbscript: URL', 'vbscript:msgbox(6)'],
  ]

  for (const [name, href] of refused) {
    it(`omits the attribute for ${name}`, () => {
      const result = renderedHref(href)

      assert.equal(result.href, undefined, 'the executable href must not reach the attribute')
      assert.equal(result.errors.length, 1, 'the author is told once why the link is inert')
      assert.match(result.errors[0], /Link/)
      assert.match(result.errors[0], /href/)
    })
  }

  it('warns once per href, however many times it is rendered', () => {
    const href = 'javascript:renderedThrice()'
    const first = renderedHref(href)
    const second = renderedHref(href)
    const third = renderedHref(href)

    assert.equal(first.errors.length, 1)
    assert.deepEqual(second.errors, [], 'a repeat render must not repeat the warning')
    assert.deepEqual(third.errors, [])
    assert.equal(second.href, undefined, 'the refusal itself does not depend on the warning')
  })

  // The whole point of a scheme allow-list living in the router rather than
  // here: these are ordinary anchors, and refusing one would break a link the
  // browser has handled since before this framework existed.
  const rendered = [
    ['a root-relative path', '/about'],
    ['a document-relative path', 'about/../contact'],
    ['a same-page anchor', '#section'],
    ['a query-only href', '?page=2'],
    ['a protocol-relative URL', '//other.test/x'],
    ['a cross-origin https URL', 'https://other.test/docs'],
    ['a mailto: address', 'mailto:a@b.test'],
    ['a tel: number', 'tel:+15550100'],
    ['an sms: number', 'sms:+15550100'],
    ['a custom scheme the browser may have a handler for', 'web+foo:open'],
    ['a scheme the router will not navigate', 'ircs://a.test/b'],
    ['a blob: URL', 'blob:https://example.test/9f0e'],
  ]

  for (const [name, href] of rendered) {
    it(`renders ${name} unchanged`, () => {
      const result = renderedHref(href)

      assert.equal(result.href, href)
      assert.deepEqual(result.errors, [], 'a legitimate href must not be warned about')
    })
  }
})

describe('a data: href, which download decides', () => {
  // `data:` is refused as a *destination* and permitted as a *file*, and the
  // `download` attribute is the whole difference between the two. Every
  // current engine has blocked top-level `data:` navigation since 2017, so a
  // `data:` href in an anchor cannot navigate anywhere — but it can still save
  // a file, and `<Link href="data:text/csv,…" download="report.csv">` is the
  // ordinary way an application hands the visitor a generated table.
  //
  // Distinct hrefs per case: the development warning fires once per href, so a
  // shared string would make one test's answer depend on whether another ran.
  const csv = (n) => `data:text/csv;charset=utf-8,name%2Cvalue%0Arow${n}%2C1`

  it('renders a data: URL when the link carries a download filename', () => {
    const href = csv(1)
    const result = renderedHref(href, { download: 'report.csv' })

    assert.equal(result.href, href, 'a downloadable data: URL is a legitimate anchor')
    assert.deepEqual(result.errors, [], 'nothing was refused, so nothing is reported')
  })

  it('renders a data: URL for a bare download attribute', () => {
    const href = csv(2)
    const result = renderedHref(href, { download: '' })

    assert.equal(result.href, href, 'the browser names the file itself when download is empty')
    assert.deepEqual(result.errors, [])
  })

  it('omits the same data: URL when the link carries no download', () => {
    const href = csv(3)
    const result = renderedHref(href)

    assert.equal(result.href, undefined, 'a data: destination is still refused')
    assert.equal(result.errors.length, 1, 'the author is told once why the link is inert')
    assert.match(result.errors[0], /download/, 'and told what would make it work')
  })

  // `download` says "save this rather than navigate to it", which is a claim
  // about a document, not a licence to execute. A `javascript:` href with a
  // download attribute still runs on Enter and on middle-click.
  for (const [name, href] of [
    ['javascript:', 'javascript:alert(7)'],
    ['vbscript:', 'vbscript:msgbox(8)'],
  ]) {
    it(`still refuses ${name} even with a download`, () => {
      const result = renderedHref(href, { download: 'x.txt' })

      assert.equal(result.href, undefined, 'download does not neuter an executable scheme')
      assert.equal(result.errors.length, 1)
    })
  }
})

describe('a Link click the router owns', () => {
  it('takes over a same-origin path', async () => {
    const result = await clickLink({ href: '/about' })

    assert.equal(result.prevented, 1)
    // No route table is reachable here, so the router falls through to a
    // document load — which is what proves it accepted the href at all.
    assert.deepEqual(result.assigned, ['https://example.test/about'])
  })

  it('takes over an allow-listed external scheme, and passes the parsed value', async () => {
    const result = await clickLink({ href: 'mailto:a@b.test' })

    assert.equal(result.prevented, 1)
    assert.deepEqual(result.assigned, ['mailto:a@b.test'])
  })

  it('takes over a cross-origin http URL', async () => {
    const result = await clickLink({ href: 'https://other.test/docs' })

    assert.equal(result.prevented, 1)
    assert.deepEqual(result.assigned, ['https://other.test/docs'])
  })
})

describe('the clicks Link has always declined', () => {
  const cases = [
    ['a modifier click', { href: '/about' }, { metaKey: true }],
    ['a middle click', { href: '/about' }, { button: 1 }],
    ['a click another handler already answered', { href: '/about' }, { defaultPrevented: true }],
    ['an explicit target', { href: '/about', target: '_blank' }, {}],
    ['a download', { href: '/file.zip', download: '' }, {}],
  ]

  for (const [name, props, overrides] of cases) {
    it(`leaves ${name} to the browser`, async () => {
      const result = await clickLink(props, overrides)

      assert.equal(result.prevented, 0)
      assert.deepEqual(result.assigned, [])
    })
  }

  it('still calls the caller onClick before deciding anything', async () => {
    const seen = []
    const result = await clickLink({
      href: 'web+foo:open',
      onClick: (event) => seen.push(event.button),
    })

    assert.deepEqual(seen, [0])
    assert.equal(result.prevented, 0)
  })
})
