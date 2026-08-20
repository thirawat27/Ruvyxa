import {
  useCallback,
  useEffect,
  useRef,
  type AnchorHTMLAttributes,
  type MouseEvent,
  type ReactNode,
  type Ref,
} from 'react'

import { getRouterInstance } from './router.js'
import type { RouteHref } from './route-types.js'

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
 * A click on a link with a modifier key, a non-primary button, or an explicit
 * `target` means the user asked the browser for something the router must not
 * take over: a new tab, a download, a background window.
 */
function shouldLetBrowserHandle(event: MouseEvent<HTMLAnchorElement>, target?: string): boolean {
  if (event.defaultPrevented) return true
  if (event.button !== 0) return true
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return true
  if (target && target !== '_self') return true
  return false
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
      if (shouldLetBrowserHandle(event, target)) return
      // `download` turns the anchor into a file transfer, not a navigation.
      if (rest.download !== undefined) return

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
      href={href}
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
