import { Link } from '@ruvyxa/react'
import type { Meta } from '@ruvyxa/react'

export const meta: Meta = {
  title: 'About',
  description: 'Who writes here, and how the site is put together.',
}

export default function About() {
  return (
    <section aria-labelledby="about-title">
      <h1 id="about-title">About</h1>
      <p>
        Replace this page with your own. Tell readers who you are, what you write about, and how to
        reach you.
      </p>

      <h2>How this site works</h2>
      <p>
        Every post is a Markdown file under <code>app/blog/</code>. The folder is the URL, the
        frontmatter is the document metadata, and <code>app/blog/posts.ts</code> reads that same
        frontmatter to build the index — so nothing is written down twice.
      </p>
      <p>
        The build publishes more than the pages: <code>sitemap.xml</code> and{' '}
        <code>robots.txt</code> come from the route manifest, and <code>rss.xml</code>,{' '}
        <code>content.json</code>, <code>search-index.json</code>, and <code>llms.txt</code> come
        from the posts themselves. Set <code>site.url</code> in <code>ruvyxa.config.ts</code> before
        deploying, because every absolute URL in those files is built from it.
      </p>

      <h2>Start here</h2>
      <p>
        <Link href="/blog/hello-ruvyxa">Hello, Ruvyxa</Link> walks through adding your first post.
      </p>
    </section>
  )
}
