const recentPosts = [
  {
    slug: 'getting-started',
    title: 'Getting Started with Ruvyxa',
    date: '2025-01-15',
    excerpt: 'Learn how to build modern web applications with the Ruvyxa framework.',
  },
  {
    slug: 'server-components',
    title: 'Understanding Server Components',
    date: '2025-01-10',
    excerpt: 'A deep dive into server-side rendering and how Ruvyxa handles it.',
  },
  {
    slug: 'styling-guide',
    title: 'Styling in Ruvyxa',
    date: '2025-01-05',
    excerpt: 'Explore the various ways to style your Ruvyxa application.',
  },
]

export default function Home() {
  return (
    <section aria-labelledby="home-title">
      <h1 id="home-title">Welcome to My Blog</h1>
      <p>Thoughts on web development, design, and building with modern frameworks.</p>

      <h2>Recent Posts</h2>
      <ul className="post-list" aria-label="Recent blog posts">
        {recentPosts.map((post) => (
          <li key={post.slug} className="post-item">
            <p className="post-date">
              <time dateTime={post.date}>
                {new Date(post.date).toLocaleDateString('en-US', {
                  year: 'numeric',
                  month: 'long',
                  day: 'numeric',
                })}
              </time>
            </p>
            <h3 className="post-title">
              <a href={`/blog/${post.slug}`}>{post.title}</a>
            </h3>
            <p className="post-excerpt">{post.excerpt}</p>
          </li>
        ))}
      </ul>
    </section>
  )
}
