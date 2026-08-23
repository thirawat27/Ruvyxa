import { Counter } from './counter'

export const meta = { title: 'Server components' }

/**
 * The server-components half of the smoke, deliberately *dynamic*.
 *
 * `force-dynamic` is the whole point: a pre-rendered server-components route
 * proves nothing about a deployment, because the payload is already inside the
 * file the adapter copies and no renderer runs. This one is rendered by the
 * emitted function on every request, so it exercises the parts a deployed build
 * has to carry for itself — the `react-server` graph, the SSR registry that
 * turns a reference id back into a component, and the payload data block the
 * browser hydrates from.
 *
 * Every adapter refused this shape with RUV2213 until the route registry
 * learned to render through the server-components pipeline.
 */
export const serverComponents = true
export const dynamic = 'force-dynamic'

export default async function Page() {
  // Awaited in the server graph, so the value can only have come from a render
  // that actually ran on the server rather than from a baked file.
  const generated = await Promise.resolve('server')
  return (
    <main>
      <p data-smoke="rsc">rendered on the {generated}</p>
      <Counter />
    </main>
  )
}
