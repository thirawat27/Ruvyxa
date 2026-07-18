const posts = [
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
  {
    slug: 'deployment',
    title: 'Deploying Your Ruvyxa App',
    date: '2025-01-01',
    excerpt: 'Step-by-step guide to deploying Ruvyxa applications to various platforms.',
  },
]

export const meta = {
  title: 'Blog - My Ruvyxa Blog',
  description: 'All blog posts.',
}

export default function BlogIndex() {
  return (
    <section aria-labelledby="blog-title">
      <h1 id="blog-title">Blog</h1>
      <p>All posts on web development, design, and building with Ruvyxa.</p>

      <ul className="post-list" aria-label="All blog posts">
        {posts.map((post) => (
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
