'use client'

// Loading and unloading a script tag is an effect against a live
// document, which is a browser concern and has no react-server counterpart.

/**
 * Third-party scripts with a loading strategy.
 *
 * A raw `<script src>` in a page body blocks the parser and downloads on the
 * critical path, which is the wrong trade for analytics, chat widgets, and
 * consent banners — the scripts this component exists for. `<Script>` moves
 * that decision into one prop and guarantees the script is fetched once per
 * document no matter how many routes render it.
 *
 * ## Strategies
 *
 * - `beforeInteractive` — rendered into the server HTML as a real `<script>`.
 *   The only strategy that runs before hydration, and the only one that works
 *   on a page with `export const hydrate = false`, which ships no client
 *   runtime for an effect to run in. Use it for the few scripts that must
 *   observe the first paint: consent gating, A/B bucketing, polyfills.
 * - `afterInteractive` (default) — appended to `<body>` after hydration.
 * - `lazyOnload` — appended once the browser is idle after `load`.
 *
 * ## Deduplication
 *
 * Client-side navigation re-renders route trees, and two routes may each render
 * the same analytics tag. Injection is therefore keyed by `id` — falling back
 * to `src` — in a module-level registry, so the second render is a no-op rather
 * than a second copy of a script that registers global handlers.
 */

import { useEffect, useRef, type ReactNode, type ScriptHTMLAttributes } from 'react'

/** When a script is fetched and executed. */
export type ScriptStrategy = 'beforeInteractive' | 'afterInteractive' | 'lazyOnload'

export interface ScriptProps extends Omit<
  ScriptHTMLAttributes<HTMLScriptElement>,
  'onLoad' | 'onError' | 'children'
> {
  /** URL of an external script. Omit it and pass inline code instead. */
  src?: string
  /**
   * Stable identity for deduplication and for inline scripts.
   *
   * Required for inline scripts: they have no `src` to key on, and two inline
   * scripts with different code are not interchangeable.
   */
  id?: string
  /** @default 'afterInteractive' */
  strategy?: ScriptStrategy
  /** Inline script source. Ignored when `src` is set. */
  children?: ReactNode
  /** Called once the script has executed. Never called for inline scripts. */
  onLoad?: () => void
  /** Called if the script fails to load. */
  onError?: (error: unknown) => void
}

/**
 * Scripts this document has already injected, keyed by `id` or `src`.
 *
 * Deliberately per-document rather than per-component: the point is to survive
 * the unmount and remount that a client-side navigation performs.
 */
const injected = new Set<string>()

/** Reset the dedupe registry. Exported for tests, which share one document. */
export function resetInjectedScripts(): void {
  injected.clear()
}

function scriptKey(props: Pick<ScriptProps, 'id' | 'src'>): string | null {
  return props.id ?? props.src ?? null
}

/**
 * Run `task` when the browser is idle, or soon after `load` where
 * `requestIdleCallback` is missing (Safari until 2022 ships without it).
 */
function whenIdle(task: () => void): () => void {
  const idle = (globalThis as { requestIdleCallback?: (callback: () => void) => number })
    .requestIdleCallback
  if (typeof idle === 'function') {
    const handle = idle(task)
    return () => {
      const cancel = (globalThis as { cancelIdleCallback?: (handle: number) => void })
        .cancelIdleCallback
      cancel?.(handle)
    }
  }
  const timer = setTimeout(task, 1)
  return () => clearTimeout(timer)
}

/** Everything {@link injectScript} needs, resolved from the component's props. */
interface InjectOptions {
  /** Dedupe identity: the `id`, or the `src` when there is no `id`. */
  key: string
  src?: string
  id?: string
  inlineCode: string | null
  attributes: Record<string, unknown>
  onLoad: () => void
  onError: (error: unknown) => void
}

/**
 * Append one script element, at most once per document per key.
 *
 * Separated from the component so the dedupe and attribute rules can be
 * exercised directly against a stub document — the same approach the router
 * tests take. The effect's only remaining job is *when* to call this.
 */
export function injectScript(
  doc: Pick<Document, 'createElement' | 'body'>,
  options: InjectOptions,
): void {
  const { key, src, id, inlineCode, attributes, onLoad, onError } = options
  // Re-checked at injection time: `lazyOnload` defers past the point where
  // another render could have claimed the same key.
  if (injected.has(key)) return
  injected.add(key)

  const element = doc.createElement('script')
  for (const [attribute, value] of Object.entries(attributes)) {
    if (value === undefined || value === null || typeof value === 'function') continue
    if (attribute === 'dangerouslySetInnerHTML') continue
    // A boolean attribute is present or absent, never `="false"`.
    if (value === false) continue
    if (value === true) {
      element.setAttribute(attributeName(attribute), '')
      continue
    }
    // An object has no meaningful attribute form. Stringifying it would write
    // "[object Object]" into the DOM and hide the caller's mistake behind a
    // value that looks deliberate.
    if (typeof value === 'object') continue
    element.setAttribute(attributeName(attribute), String(value))
  }
  if (src) {
    element.src = src
    element.addEventListener('load', () => onLoad())
    element.addEventListener('error', (event) => {
      // A failed third-party script must not leave its key claimed, or a later
      // render can never retry it.
      injected.delete(key)
      onError(event)
    })
  } else if (inlineCode !== null) {
    element.textContent = inlineCode
  }
  if (id) element.id = id
  doc.body.append(element)
  // An inline script has executed by the time `append` returns; there is no
  // load event to wait for.
  if (!src) onLoad()
}

/**
 * Load a third-party script without putting it on the critical path.
 *
 * @example
 * ```tsx
 * <Script src="https://plausible.io/js/script.js" strategy="lazyOnload" />
 * <Script id="consent" strategy="beforeInteractive">{`window.__consent = true`}</Script>
 * ```
 */
export function Script({
  src,
  id,
  strategy = 'afterInteractive',
  children,
  onLoad,
  onError,
  ...rest
}: Readonly<ScriptProps>) {
  // Held in refs so a caller passing a fresh arrow function every render does
  // not re-run the effect and inject the script twice.
  const handlers = useRef({ onLoad, onError })
  handlers.current = { onLoad, onError }

  const inlineCode = typeof children === 'string' ? children : null

  useEffect(() => {
    if (strategy === 'beforeInteractive') return
    if (typeof document === 'undefined') return

    const key = scriptKey({ id, src })
    if (key === null) {
      handlers.current.onError?.(
        new Error('<Script> needs `src` or `id`: without one it cannot be deduplicated.'),
      )
      return
    }
    if (injected.has(key)) {
      // Already loaded by an earlier render. Report success so a caller waiting
      // on `onLoad` is not left hanging on a navigation back to this route.
      handlers.current.onLoad?.()
      return
    }

    let cancelIdle: (() => void) | null = null

    const inject = () =>
      injectScript(document, {
        key,
        src,
        id,
        inlineCode,
        attributes: rest as Record<string, unknown>,
        onLoad: () => handlers.current.onLoad?.(),
        onError: (error) => handlers.current.onError?.(error),
      })

    if (strategy === 'lazyOnload') {
      if (document.readyState === 'complete') {
        cancelIdle = whenIdle(inject)
      } else {
        const onWindowLoad = () => {
          cancelIdle = whenIdle(inject)
        }
        window.addEventListener('load', onWindowLoad, { once: true })
        return () => window.removeEventListener('load', onWindowLoad)
      }
    } else {
      inject()
    }

    return () => {
      cancelIdle?.()
      // The element is deliberately left in the document. Removing a script tag
      // does not undo what it did — the globals, listeners, and timers it
      // installed outlive it — so tearing it down on unmount would only mean a
      // second copy runs the next time the route renders.
    }
    // `rest` is spread into attributes; comparing it by identity would re-run on
    // every render. The attributes of a given script are not expected to change,
    // and the dedupe registry makes a missed update a no-op rather than a
    // duplicate injection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, src, strategy, inlineCode])

  if (strategy !== 'beforeInteractive') return null

  // Rendered identically on the server and the client, so hydration matches.
  if (src) return <script {...rest} id={id} src={src} />
  if (inlineCode !== null) {
    return <script {...rest} id={id} dangerouslySetInnerHTML={{ __html: inlineCode }} />
  }
  return null
}

/**
 * Map a React prop name to the HTML attribute name.
 *
 * `className` and `htmlFor` are the two React renames that can plausibly appear
 * on a script tag; everything else — `async`, `defer`, `nonce`, `crossOrigin`,
 * `data-*` — is either identical or camelCase for a hyphenated attribute.
 */
function attributeName(prop: string): string {
  if (prop === 'className') return 'class'
  if (prop === 'htmlFor') return 'for'
  if (prop.startsWith('data') && prop !== 'data') return prop
  return prop.replace(/([A-Z])/g, '-$1').toLowerCase()
}
