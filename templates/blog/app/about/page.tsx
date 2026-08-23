import type { Meta } from '@ruvyxa/react'

export const meta: Meta = {
  title: 'About',
  description: 'Learn more about this blog and its author.',
}

export default function About() {
  return (
    <section aria-labelledby="about-title">
      <h1 id="about-title">About</h1>
      <p>
        This blog is built with Ruvyxa, a full-stack web framework with a Rust compiler and server
        and a TypeScript runtime. It is a starting point for content-focused sites.
      </p>

      <h2>How it works</h2>
      <p>
        Posts live in <code>app/blog/posts.ts</code>. The index at <code>app/blog/page.tsx</code>{' '}
        lists them, and <code>app/blog/[slug]/page.tsx</code> renders one — its{' '}
        <code>getStaticParams</code> export tells the build which posts to pre-render, so every post
        is a file on disk before anyone asks for it.
      </p>

      <h2>About the author</h2>
      <p>
        Replace this section with your own bio. Tell readers who you are, what you write about, and
        how they can get in touch.
      </p>
    </section>
  )
}
