import { Link } from '@ruvyxa/react'

import { formatDate, posts } from './blog/posts'

const recentPosts = posts.slice(0, 3)

export default function Home() {
  return (
    <section aria-labelledby="home-title">
      <h1 id="home-title">Welcome to My Blog</h1>
      <p>Thoughts on web development, design, and building with modern frameworks.</p>

      <h2>Recent posts</h2>
      <ul className="post-list" aria-label="Recent posts">
        {recentPosts.map((post) => (
          <li key={post.href} className="post-item">
            <p className="post-date">
              <time dateTime={post.date}>{formatDate(post.date)}</time>
            </p>
            <h3 className="post-title">
              <Link href={post.href}>{post.title}</Link>
            </h3>
            <p className="post-excerpt">{post.description}</p>
          </li>
        ))}
      </ul>

      <p>
        <Link href="/blog">Read every post →</Link>
      </p>
    </section>
  )
}
