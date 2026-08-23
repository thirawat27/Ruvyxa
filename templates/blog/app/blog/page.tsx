import { Link } from '@ruvyxa/react'
import type { Meta } from '@ruvyxa/react'

import { formatDate, posts } from './posts'

export const meta: Meta = {
  title: 'Blog',
  description: 'Every post, newest first.',
}

export default function BlogIndex() {
  return (
    <section aria-labelledby="blog-title">
      <h1 id="blog-title">Blog</h1>
      <p>Posts on web development, design, and building with Ruvyxa.</p>

      <ul className="post-list" aria-label="All posts">
        {posts.map((post) => (
          <li key={post.href} className="post-item">
            <p className="post-date">
              <time dateTime={post.date}>{formatDate(post.date)}</time>
            </p>
            <h2 className="post-title">
              <Link href={post.href}>{post.title}</Link>
            </h2>
            <p className="post-excerpt">{post.description}</p>
            {post.tags && (
              <ul className="post-tags" aria-label="Tags">
                {post.tags.map((tag) => (
                  <li key={tag}>{tag}</li>
                ))}
              </ul>
            )}
          </li>
        ))}
      </ul>
    </section>
  )
}
