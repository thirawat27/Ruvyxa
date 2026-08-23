import type { RouteHref } from '@ruvyxa/react'

import { frontmatter as authoringInMdx } from './authoring-in-mdx/page.mdx'
import { frontmatter as choosingARenderingStrategy } from './choosing-a-rendering-strategy/page.mdx'
import { frontmatter as deployingYourBlog } from './deploying-your-blog/page.mdx'
import { frontmatter as helloRuvyxa } from './hello-ruvyxa/page.mdx'

export interface Post extends ContentFrontmatter {
  /**
   * Where the post is served. With `typedRoutes` on this is checked against the
   * routes that exist, so a folder renamed without its entry is a compile
   * error rather than a link that 404s.
   */
  href: RouteHref
}

/**
 * Every published post, newest first.
 *
 * The frontmatter is imported rather than restated: a title lives in the post
 * it belongs to, and the index reads it from there. Publishing a new post is a
 * new folder plus one line here — the order things appear in is a decision, and
 * it is worth being able to read it.
 *
 * Sorted by comparing the ISO strings directly. `localeCompare` would order by
 * the building machine's locale, so two machines could produce two different
 * pages from the same source.
 */
const published = [
  { href: '/blog/hello-ruvyxa', ...helloRuvyxa },
  { href: '/blog/authoring-in-mdx', ...authoringInMdx },
  { href: '/blog/choosing-a-rendering-strategy', ...choosingARenderingStrategy },
  { href: '/blog/deploying-your-blog', ...deployingYourBlog },
] satisfies readonly Post[]

function newestFirst(left: Post, right: Post): number {
  if (left.date === right.date) return 0
  return left.date < right.date ? 1 : -1
}

export const posts: readonly Post[] = [...published].sort(newestFirst)

const displayDate = new Intl.DateTimeFormat('en-US', {
  timeZone: 'UTC',
  year: 'numeric',
  month: 'long',
  day: 'numeric',
})

/**
 * Format a post's ISO date for display.
 *
 * `timeZone: 'UTC'` is load-bearing. `new Date('2026-08-18')` is midnight UTC,
 * and formatting it in the machine's own zone gives the 17th anywhere west of
 * Greenwich — so a page pre-rendered on a server in one zone would disagree
 * with the browser that hydrates it, and React would report the mismatch.
 */
export function formatDate(iso: string): string {
  return displayDate.format(new Date(iso))
}
