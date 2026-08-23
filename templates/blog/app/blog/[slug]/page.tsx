import { Link } from '@ruvyxa/react'
import type { MetaFactory } from '@ruvyxa/react'
import type { GetStaticParams, PageProps } from 'ruvyxa'

import { findPost, formatDate, posts } from '../posts'

// Tells the build which slugs to pre-render. Each returned object is one URL:
// /blog/getting-started, /blog/rendering-strategies, and so on.
export const getStaticParams: GetStaticParams<{ slug: string }> = () =>
  posts.map((post) => ({ slug: post.slug }))

export const meta: MetaFactory = ({ params }) => {
  const post = findPost(params.slug)
  return post ? { title: post.title, description: post.excerpt } : { title: 'Post not found' }
}

export default function BlogPost({ params }: Readonly<PageProps<{ slug: string }>>) {
  const post = findPost(params.slug)

  if (!post) {
    return (
      <section aria-labelledby="missing-title">
        <h1 id="missing-title">Post not found</h1>
        <p>No post matches that address.</p>
        <Link href="/blog">Back to all posts</Link>
      </section>
    )
  }

  return (
    <article aria-labelledby="post-title">
      <h1 id="post-title">{post.title}</h1>
      <p className="post-date">
        <time dateTime={post.date}>{formatDate(post.date)}</time>
      </p>
      {post.content.map((paragraph) => (
        <p key={paragraph}>{paragraph}</p>
      ))}
      <Link href="/blog">Back to all posts</Link>
    </article>
  )
}
