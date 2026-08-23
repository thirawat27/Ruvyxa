/**
 * Ambient types for the non-TypeScript modules this project imports.
 *
 * TypeScript resolves `./hello-ruvyxa/page.mdx` only because of the declaration
 * below; the Ruvyxa compiler already knows how to build it.
 */

/**
 * The frontmatter every Markdown and MDX route in this blog declares.
 *
 * A `page.mdx` exports its frontmatter twice: as `frontmatter`, which
 * `app/blog/posts.ts` reads to build the index, and as `meta`, which the router
 * turns into the document's `<title>` and `<meta name="description">`. Writing
 * it once is what keeps the index and the document head from drifting apart.
 *
 * Every `.mdx` file in this starter is a post, so one shape describes them all.
 * Widen the fields — or split the declaration by directory — if that stops
 * being true.
 */
interface ContentFrontmatter {
  title: string
  description: string
  /** ISO 8601 date. Quote it in the frontmatter so YAML keeps it a string. */
  date: string
  tags?: readonly string[]
}

declare module '*.mdx' {
  import type { ComponentType } from 'react'

  /** Frontmatter exactly as written, after any remark or rehype plugin. */
  export const frontmatter: ContentFrontmatter
  /** The same object, read by the router as this route's metadata. */
  export const meta: ContentFrontmatter
  /** Headings in document order, with the `id` each one rendered with. */
  export const headings: readonly { depth: number; slug: string; text: string }[]
  export const contentFormat: 'md' | 'mdx'

  const MDXContent: ComponentType<{ components?: Readonly<Record<string, unknown>> }>
  export default MDXContent
}

declare module '*.md' {
  import type { ComponentType } from 'react'

  export const frontmatter: ContentFrontmatter
  export const meta: ContentFrontmatter
  export const headings: readonly { depth: number; slug: string; text: string }[]
  export const contentFormat: 'md' | 'mdx'

  const MarkdownContent: ComponentType<{ components?: Readonly<Record<string, unknown>> }>
  export default MarkdownContent
}
