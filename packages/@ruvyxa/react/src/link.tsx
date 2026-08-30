'use client'

// A link is interactive: it holds refs, subscribes to hover and
// viewport, and hands the click to the router. None of that exists in the
// react-server build, so a server component importing it must get a
// reference to the browser module rather than the module itself.

import {
  useCallback,
  useEffect,
  useRef,
  type AnchorHTMLAttributes,
  type MouseEvent,
  type ReactNode,
  type Ref,
} from 'react'

import { classifyNavigationTarget, getRouterInstance } from './router.js'
import type { RouteHref } from './route-types.js'

/**
 * The mode this bundle was built for.
 *
 * Declared here rather than taken from `@types/node`, which this package does
 * not depend on and should not — a browser module wants exactly one name out
 * of it. The bare identifier is deliberate: both linkers open every module
 * factory with `var process = globalThis.process || { env: { NODE_ENV:
 * "production" } }`, so `process.env.NODE_ENV` reads that shim in a browser
 * while `globalThis.process` is undefined there. That shim is what makes the
 * guard below correct at runtime everywhere; writing it as a braced
 * `!== 'production'` block is what additionally lets the bundler's production
 * fold delete it from an installed copy, which is the shape that fold looks
 * for.
 */
declare const process: { env: { NODE_ENV?: string } }

/** When to warm the target route's bundle. */
export type LinkPrefetch = boolean | 'hover' | 'viewport' | 'none'

/**
 * Props for {@link Link}.
 *
 * Every anchor attribute is forwarded, so `className`, `aria-*`, `rel`, and
 * `target` behave exactly as they do on a plain `<a>`.
 */
export interface LinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> {
  /**
   * Destination URL. Relative paths resolve against the current document.
   *
   * Narrowed to the project's real routes once `.ruvyxa/types/routes.d.ts` is
   * generated and included by `tsconfig.json`; plain `string` otherwise. Wrap a
   * URL computed at runtime in `route()` to satisfy the narrowed type.
   */
  href: RouteHref
  /** Replace the current history entry instead of pushing a new one. */
  replace?: boolean
  /** Scroll to the top after navigating. Defaults to `true`. */
  scroll?: boolean
  /** Animate this navigation with the browser View Transitions API when available. */
  viewTransition?: boolean
  /**
   * Warm the destination bundle ahead of the click.
   *
   * `"hover"` (the default) waits for pointer or keyboard focus. `"viewport"`
   * warms as soon as the link is scrolled into view. `false` and `"none"`
   * disable it.
   */
  prefetch?: LinkPrefetch
  children?: ReactNode
  ref?: Ref<HTMLAnchorElement>
}

/**
 * Every reason this click belongs to the browser rather than to the router.
 *
 * A modifier key, a non-primary button, an explicit `target`, or a `download`
 * means the user asked for something the router must not take over: a new tab,
 * a file transfer, a background window.
 *
 * The href is the fourth reason, and the one that is easy to miss. The router
 * navigates an allow-list of schemes and refuses everything else, so a
 * `web+foo:` or a custom app link is a href it will decline — and a click this
 * component suppresses and the router then declines is a link that does
 * nothing at all. Asked here, before `preventDefault()`, the anchor keeps the
 * browser's own handling of exactly those hrefs, which is what a plain `<a>`
 * has always done and what a middle-click on this one still does.
 *
 * One predicate rather than a check beside it: the caller's question is
 * "is this click mine?", and an answer split across two places is one a later
 * reader has to reassemble — which is how the `preventDefault()` and the
 * refusal ended up on opposite sides of the decision in the first place.
 */
function shouldLetBrowserHandle(
  event: MouseEvent<HTMLAnchorElement>,
  href: string,
  target?: string,
  download?: unknown,
): boolean {
  if (event.defaultPrevented) return true
  if (event.button !== 0) return true
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return true
  if (target && target !== '_self') return true
  // `download` turns the anchor into a file transfer, not a navigation.
  if (download !== undefined) return true
  return classifyNavigationTarget(href).kind === 'refused'
}

/**
 * Schemes that run code when the browser follows them.
 *
 * This is not a second copy of the router's allow-list, and writing it as one
 * would be wrong in both directions. `classifyNavigationTarget` answers "may
 * the _router_ navigate here?", and most of what it refuses makes a perfectly
 * good anchor — `web+foo:`, `ircs:`, an app deep link — which the browser
 * passes to a registered handler and this component must leave alone. The
 * question only the rendered attribute can ask is narrower and has a different
 * answer: does putting this string in `href` hand the browser a *script*?
 *
 * These two do, from every path a href is followed by: a left-click, a
 * middle-click, Enter, and the context menu. `download` does not change that,
 * which is why the set is consulted before it.
 */
const SCRIPT_PROTOCOLS = new Set(['javascript:', 'vbscript:'])

/**
 * Whether `href` could carry a scheme at all — a `:` ahead of the first `/`,
 * `?`, or `#`.
 *
 * Deliberately incapable of naming a scheme, because a second thing that knows
 * which schemes matter is a second thing that can drift from the parser. This
 * only ever answers "is there anything here for the parser to find?", and
 * everything it admits is still handed to the parser, which stays the only
 * authority on what the scheme actually is.
 *
 * Safe as an over-approximation because a URL's scheme is the text before the
 * first `:` and may contain neither `/`, `?`, nor `#`: a href with none of
 * those `:` -preceded has no scheme under any parser. The normalisation the
 * parser performs first cannot change that answer either — it removes leading
 * spaces and C0 controls and interior tabs and newlines, none of which is a
 * `:`, `/`, `?`, or `#`, and removal preserves the order of the ones that
 * remain.
 *
 * It exists for one reason: `renderableHref` runs on every render of every
 * link, and the overwhelmingly common href is relative, which the parser
 * answers by *throwing*. Constructing and unwinding an exception per link per
 * render pass is real work to reach a conclusion this settles by reading a few
 * characters.
 */
const SCHEME_LIKE = /^[^/?#]*:/

/**
 * The scheme the browser will follow `href` with, or `null` when it has none.
 *
 * Parsed rather than string-matched, because the browser's own parser is the
 * only thing that agrees with the browser: it drops leading spaces and control
 * characters, strips every tab and newline from inside the scheme, and
 * lower-cases the result. So `"  JaV\tascRipt:alert(1)"` is a `javascript:`
 * URL, and a hand-written `startsWith` says it is not.
 *
 * Parsed with no base on purpose. A relative href throws, and "it carries no
 * scheme of its own" is exactly the answer wanted; it also means this works in
 * a server render, where there is no document to resolve against — and the
 * server render is where the attribute matters most, because the HTML it
 * writes is live in the browser before any of this file's JavaScript is.
 */
function schemeOf(href: string): string | null {
  if (!SCHEME_LIKE.test(href)) return null
  try {
    return new URL(href).protocol
  } catch {
    return null
  }
}

/**
 * Why this scheme may not be rendered, or `null` when it may.
 *
 * Two rules, and they are not the same rule with a different list.
 *
 * `javascript:` and `vbscript:` are executable wherever they appear, so they
 * are refused unconditionally.
 *
 * `data:` is not executable in an anchor — every current engine has blocked
 * top-level `data:` navigation since 2017 (Chrome 60, Firefox 59), so a
 * `data:text/html` href cannot become a document and cannot run its script.
 * What is left is a destination that does nothing, which is worth telling an
 * author about, and a *file*, which is worth rendering:
 * `<Link href="data:text/csv,…" download="report.csv">` is the ordinary way an
 * application hands a visitor a generated table, and refusing it buys no
 * security while breaking a legitimate page. `download` is exactly the
 * attribute that separates the two, and the click handler already reads it the
 * same way — a `download` anchor is a file transfer, never a navigation.
 */
function refusalReason(scheme: string, download: unknown): string | null {
  if (SCRIPT_PROTOCOLS.has(scheme)) {
    return (
      `a "${scheme}" URL executes as script, so the anchor is rendered with ` +
      'no href and does nothing. Use a <button> with onClick for an action, ' +
      'or a plain <a> if this URL is really what you meant.'
    )
  }
  if (scheme === 'data:' && download === undefined) {
    return (
      'browsers refuse to navigate to a "data:" URL, so this link would do ' +
      'nothing at all; the anchor is rendered with no href to say so. Add a ' +
      'download attribute to save it as a file, which is what a data: href ' +
      'is for, or serve the content from a real URL.'
    )
  }
  return null
}

/**
 * Hrefs already reported, so a refused link in a list of a thousand rows says
 * so once rather than a thousand times.
 */
const reportedHrefs = new Set<string>()

/**
 * The href this anchor may carry: `href` itself, or nothing at all when the
 * scheme is refused.
 *
 * The click handler cannot be the place for this. A `javascript:` href runs on
 * a middle-click, on Enter, from the context menu, and in the HTML the server
 * rendered — every one of those paths goes around `onClick`, and the last one
 * happens before there is an `onClick` to go around.
 *
 * The attribute is omitted rather than replaced with `"#"`, because the two
 * say different things. `#` is a link to the top of this page, which the
 * author did not write and a reader would have to debug; an `<a>` with no
 * `href` is not a link at all — not focusable, not activated by Enter, nothing
 * for a middle-click to open. Inert is the honest rendering of a destination
 * that was refused.
 *
 * `download` is a second input rather than a second decision at the call site,
 * for the same reason `shouldLetBrowserHandle` is one predicate: the answer to
 * "may this href be rendered?" depends on it, and an answer assembled from two
 * places is one a later reader has to reassemble.
 */
function renderableHref(href: string, download: unknown): string | undefined {
  const scheme = schemeOf(href)
  if (scheme === null) return href
  const reason = refusalReason(scheme, download)
  if (reason === null) return href
  if (process.env.NODE_ENV !== 'production') {
    if (!reportedHrefs.has(href)) {
      reportedHrefs.add(href)
      console.error(`Ruvyxa <Link> refused to render href ${JSON.stringify(href)}: ${reason}`)
    }
  }
  return undefined
}

/**
 * Navigate between Ruvyxa routes without a document load.
 *
 * Renders a real `<a href>`, so the link is crawlable, middle-clickable, and
 * still works before hydration or with JavaScript disabled. Client-side
 * navigation is a progressive enhancement layered on top of that.
 *
 * @example
 * ```tsx
 * import { Link } from "@ruvyxa/react"
 *
 * export default function Nav() {
 *   return (
 *     <nav>
 *       <Link href="/">Home</Link>
 *       <Link href="/blog/hello" prefetch="viewport">Hello</Link>
 *     </nav>
 *   )
 * }
 * ```
 */
export function Link({
  href,
  replace = false,
  scroll = true,
  viewTransition = false,
  prefetch = 'hover',
  children,
  onClick,
  onMouseEnter,
  onFocus,
  target,
  ref,
  ...rest
}: Readonly<LinkProps>) {
  const anchorRef = useRef<HTMLAnchorElement | null>(null)
  // Which href was prefetched, rather than a bare "yes" plus an effect to clear
  // it when the href changes. Recording the value closes the window that split
  // version left open: between an href change and the reset effect running, the
  // guard still said "already prefetched" and the new destination was skipped.
  const prefetchedHref = useRef<string | null>(null)

  const warm = useCallback(() => {
    if (prefetchedHref.current === href) return
    prefetchedHref.current = href
    getRouterInstance().prefetch(href)
  }, [href])

  useEffect(() => {
    if (prefetch !== 'viewport') return
    const element = anchorRef.current
    if (!element || typeof IntersectionObserver === 'undefined') return

    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            warm()
            observer.disconnect()
          }
        }
      },
      { rootMargin: '200px' },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [prefetch, warm])

  const handleClick = useCallback(
    (event: MouseEvent<HTMLAnchorElement>) => {
      onClick?.(event)
      if (shouldLetBrowserHandle(event, href, target, rest.download)) return

      const router = getRouterInstance()
      event.preventDefault()
      void router.navigate(href, { replace, scroll, viewTransition })
    },
    [href, onClick, replace, rest.download, scroll, target, viewTransition],
  )

  const shouldWarmOnPointer = prefetch === true || prefetch === 'hover'

  return (
    <a
      {...rest}
      href={renderableHref(href, rest.download)}
      target={target}
      ref={(node) => {
        anchorRef.current = node
        if (typeof ref === 'function') ref(node)
        else if (ref) ref.current = node
      }}
      onClick={handleClick}
      onMouseEnter={(event) => {
        if (shouldWarmOnPointer) warm()
        onMouseEnter?.(event)
      }}
      onFocus={(event) => {
        if (shouldWarmOnPointer) warm()
        onFocus?.(event)
      }}
    >
      {children}
    </a>
  )
}
