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
  },
  'the Rust bundler': {
    context: rustStringLiteral('ROUTE_CONTEXT_PRELUDE'),
    boundary: rustRawLiteral('ROUTE_BOUNDARY_PRELUDE'),
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
}
