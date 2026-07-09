/**
 * SSG with Dynamic Segments — uses getStaticParams.
 *
 * This page has a dynamic [slug] segment but declares `getStaticParams`
 * to tell Ruvyxa which paths to pre-render at build time.
 *
 * Detection: `export function getStaticParams` → SSG with dynamic params.
 */

import type { GetStaticParams, PageProps } from "ruvyxa/config"

const posts = [
  { slug: "hello-world", title: "Hello World", content: "Welcome to the Ruvyxa blog!" },
  { slug: "rendering-strategies", title: "Rendering Strategies", content: "SSG, ISR, CSR, PPR, and SSR — choose the right one for each page." },
  { slug: "performance", title: "Performance Tips", content: "How to get the most out of Ruvyxa's build-time optimizations." },
]

/**
 * Tell Ruvyxa which slugs to pre-render at build time.
 * Each returned object maps to one URL: /ssg-blog/hello-world, etc.
 */
export const getStaticParams: GetStaticParams<{ slug: string }> = async () => {
  return posts.map(post => ({ slug: post.slug }))
}

export default function BlogPost({ params }: PageProps<{ slug: string }>) {
  const post = posts.find(p => p.slug === params.slug)

  if (!post) {
    return (
      <main className="page-wide">
        <h1>Post Not Found</h1>
        <p>No blog post with slug "{params.slug}" exists.</p>
        <a href="/ssg-blog">← Back to blog</a>
      </main>
    )
  }

  return (
    <main className="page-wide">
      <h1>{post.title}</h1>
      <p>{post.content}</p>

      <section>
        <h2>About this page</h2>
        <p>
          This page was pre-rendered at build time using <code>getStaticParams</code>.
          The slug "<code>{params.slug}</code>" was resolved at build time.
        </p>
      </section>

      <p className="badge">Strategy: SSG (dynamic)</p>
      <a href="/ssg-blog">← Back to blog</a>
    </main>
  )
}
