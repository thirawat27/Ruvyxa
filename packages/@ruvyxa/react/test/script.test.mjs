/**
 * `<Script>`: what reaches the server HTML, and what the client injects.
 *
 * The strategy split is the whole point of the component, so both halves are
 * covered — `renderToStaticMarkup` for the server output, and `injectScript`
 * against a stub document for the client. A stub is enough because everything
 * worth asserting is a DOM call the component makes or declines to make.
 */

import assert from 'node:assert/strict'
import { describe, it, beforeEach } from 'node:test'

import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import * as scriptModule from '../dist/script.js'
import { Script, injectScript, resetInjectedScripts } from '../dist/script.js'

function stubDocument() {
  const appended = []
  return {
    appended,
    createElement() {
      const listeners = {}
      return {
        attributes: {},
        listeners,
        id: '',
        src: '',
        textContent: '',
        setAttribute(name, value) {
          this.attributes[name] = value
        },
        addEventListener(type, handler) {
          listeners[type] = handler
        },
      }
    },
    body: {
      append(node) {
        appended.push(node)
      },
    },
  }
}

function inject(doc, options) {
  injectScript(doc, {
    inlineCode: null,
    attributes: {},
    onLoad: () => {},
    onError: () => {},
    ...options,
  })
}

beforeEach(() => {
  resetInjectedScripts()
})

describe('server output', () => {
  it('renders beforeInteractive scripts into the HTML', () => {
    const html = renderToStaticMarkup(
      createElement(Script, { src: 'https://cdn.example.com/a.js', strategy: 'beforeInteractive' }),
    )
    assert.match(html, /<script src="https:\/\/cdn\.example\.com\/a\.js"/)
  })

  it('renders inline beforeInteractive code', () => {
    const html = renderToStaticMarkup(
      createElement(
        Script,
        { id: 'consent', strategy: 'beforeInteractive' },
        'window.__consent = true',
      ),
    )
    assert.match(html, /window\.__consent = true/)
  })

  it('emits nothing for the deferred strategies', () => {
    // They are appended by an effect after hydration; putting them in the
    // server HTML too would load each script twice.
    for (const strategy of ['afterInteractive', 'lazyOnload']) {
      const html = renderToStaticMarkup(
        createElement(Script, { src: 'https://cdn.example.com/a.js', strategy }),
      )
      assert.equal(html, '', `${strategy} must not reach the server HTML`)
    }
  })

  it('defaults to afterInteractive', () => {
    const html = renderToStaticMarkup(
      createElement(Script, { src: 'https://cdn.example.com/a.js' }),
    )
    assert.equal(html, '')
  })
})

describe('client injection', () => {
  it('appends one script with its attributes', () => {
    const doc = stubDocument()
    inject(doc, {
      key: 'a',
      src: 'https://cdn.example.com/a.js',
      id: 'a',
      attributes: { async: true, crossOrigin: 'anonymous', defer: false },
    })

    assert.equal(doc.appended.length, 1)
    const [element] = doc.appended
    assert.equal(element.src, 'https://cdn.example.com/a.js')
    assert.equal(element.id, 'a')
    assert.equal(element.attributes.async, '', 'a boolean attribute is present, not "true"')
    assert.equal(element.attributes['cross-origin'], 'anonymous')
    assert.ok(!('defer' in element.attributes), 'a false boolean attribute is omitted entirely')
  })

  it('injects a given key only once', () => {
    // Two routes rendering the same analytics tag must not load it twice, and a
    // navigation back to a route must not re-run its script.
    const doc = stubDocument()
    inject(doc, { key: 'analytics', src: 'https://cdn.example.com/a.js' })
    inject(doc, { key: 'analytics', src: 'https://cdn.example.com/a.js' })
    assert.equal(doc.appended.length, 1)
  })

  it('releases the key when the script fails, so a later render can retry', () => {
    const doc = stubDocument()
    const errors = []
    injectScript(doc, {
      key: 'flaky',
      src: 'https://cdn.example.com/flaky.js',
      inlineCode: null,
      attributes: {},
      onLoad: () => {},
      onError: (error) => errors.push(error),
    })
    doc.appended[0].listeners.error('boom')

    assert.deepEqual(errors, ['boom'])
    inject(doc, { key: 'flaky', src: 'https://cdn.example.com/flaky.js' })
    assert.equal(doc.appended.length, 2, 'a failed script must not hold its key forever')
  })

  it('reports load synchronously for inline code', () => {
    // There is no load event on an inline script; a caller waiting on `onLoad`
    // would otherwise wait forever.
    const doc = stubDocument()
    let loaded = false
    injectScript(doc, {
      key: 'inline',
      inlineCode: 'window.x = 1',
      attributes: {},
      onLoad: () => {
        loaded = true
      },
      onError: () => {},
    })
    assert.equal(doc.appended[0].textContent, 'window.x = 1')
    assert.ok(loaded)
  })

  /**
   * The key used to be claimed at injection and read as "loaded", so a second
   * `<Script src=X onLoad>` mounting while X was still downloading had its
   * `onLoad` called on the spot — before `window.Stripe` existed.
   */
  it('reports load to a second caller only once the script has actually loaded', () => {
    const doc = stubDocument()
    const loaded = []
    inject(doc, { key: 'stripe', src: 'https://js.stripe.com/v3', onLoad: () => loaded.push(1) })
    inject(doc, { key: 'stripe', src: 'https://js.stripe.com/v3', onLoad: () => loaded.push(2) })

    assert.equal(doc.appended.length, 1, 'the second caller must not inject a second copy')
    assert.deepEqual(loaded, [], 'nothing has loaded yet')

    doc.appended[0].listeners.load()
    assert.deepEqual(loaded, [1, 2], 'every caller waiting on the script hears it load, once')

    inject(doc, { key: 'stripe', src: 'https://js.stripe.com/v3', onLoad: () => loaded.push(3) })
    assert.deepEqual(loaded, [1, 2, 3], 'a caller arriving after load is told immediately')
    assert.equal(doc.appended.length, 1)
  })

  it('reports an error to every caller waiting on the same script', () => {
    const doc = stubDocument()
    const errors = []
    const loaded = []
    for (const caller of [1, 2]) {
      inject(doc, {
        key: 'flaky',
        src: 'https://cdn.example.com/flaky.js',
        onLoad: () => loaded.push(caller),
        onError: (error) => errors.push([caller, error]),
      })
    }
    doc.appended[0].listeners.error('boom')

    assert.deepEqual(errors, [
      [1, 'boom'],
      [2, 'boom'],
    ])
    assert.deepEqual(loaded, [])
  })

  it('stops reporting to a caller that unsubscribed before the script loaded', () => {
    // An unmounted component must not have its `onLoad` run.
    const doc = stubDocument()
    const loaded = []
    const unsubscribe = injectScript(doc, {
      key: 'widget',
      src: 'https://cdn.example.com/widget.js',
      inlineCode: null,
      attributes: {},
      onLoad: () => loaded.push('unmounted'),
      onError: () => {},
    })
    inject(doc, {
      key: 'widget',
      src: 'https://cdn.example.com/widget.js',
      onLoad: () => loaded.push('mounted'),
    })
    assert.equal(typeof unsubscribe, 'function')
    unsubscribe()
    doc.appended[0].listeners.load()

    assert.deepEqual(loaded, ['mounted'])
  })
})

describe('lazyOnload scheduling', () => {
  function stubWindow() {
    const listeners = new Map()
    return {
      listeners,
      addEventListener(type, handler) {
        listeners.set(type, handler)
      },
      removeEventListener(type, handler) {
        if (listeners.get(type) === handler) listeners.delete(type)
      },
    }
  }

  const settle = () => new Promise((resolve) => setTimeout(resolve, 20))

  it('runs the task once the window has loaded and the browser is idle', async () => {
    const win = stubWindow()
    let ran = 0
    scriptModule.whenLoadedAndIdle(win, { readyState: 'loading' }, () => {
      ran += 1
    })
    await settle()
    assert.equal(ran, 0, 'nothing runs before the window load event')

    win.listeners.get('load')()
    await settle()
    assert.equal(ran, 1)
  })

  /**
   * The cleanup used to remove only the `load` listener. Once `load` had fired
   * the idle callback was already queued, so an unmount in that gap still
   * injected the script and ran the unmounted component's `onLoad`.
   */
  it('cancels the queued idle task when cleaned up after the window load event', async () => {
    const win = stubWindow()
    let ran = 0
    const cancel = scriptModule.whenLoadedAndIdle(win, { readyState: 'loading' }, () => {
      ran += 1
    })
    win.listeners.get('load')()
    cancel()
    await settle()
    assert.equal(ran, 0, 'a cleaned-up schedule must not inject')
  })

  it('removes the load listener when cleaned up before the window load event', async () => {
    const win = stubWindow()
    const cancel = scriptModule.whenLoadedAndIdle(win, { readyState: 'loading' }, () => {})
    cancel()
    assert.equal(win.listeners.has('load'), false)
  })

  it('cancels an idle task scheduled on an already-loaded document', async () => {
    let ran = 0
    const cancel = scriptModule.whenLoadedAndIdle(stubWindow(), { readyState: 'complete' }, () => {
      ran += 1
    })
    cancel()
    await settle()
    assert.equal(ran, 0)
  })
})
