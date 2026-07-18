const posts: Record<string, { title: string; date: string; content: string }> = {
  'getting-started': {
    title: 'Getting Started with Ruvyxa',
    date: '2025-01-15',
    content:
      'Ruvyxa is a modern full-stack framework that combines the power of Rust with the flexibility of React. In this post, we will walk through setting up your first project and exploring the core concepts.',
  },
  'server-components': {
    title: 'Understanding Server Components',
    date: '2025-01-10',
    content:
      'Server components allow you to render parts of your application on the server, reducing the amount of JavaScript sent to the client. Ruvyxa makes this seamless with its file-based routing system.',
  },
  'styling-guide': {
    title: 'Styling in Ruvyxa',
    date: '2025-01-05',
    content:
      'Ruvyxa supports multiple styling approaches including global CSS, CSS Modules, and imported stylesheets. This guide covers best practices for each approach.',
  },
  deployment: {
    title: 'Deploying Your Ruvyxa App',
    date: '2025-01-01',
    content:
      'Once your application is ready, you can deploy it to various platforms. Ruvyxa provides adapters for Node, Bun, Vercel, Netlify, Cloudflare, and static hosting.',
  },
}

export const staticParams = Object.keys(posts)

export default function BlogPost({ params }: { params: { slug: string } }) {
  const post = posts[params.slug]

  if (!post) {
    return (
      <section aria-labelledby="not-found-title">
        <h1 id="not-found-title">Post Not Found</h1>
        <p>The blog post you are looking for does not exist.</p>
        <a href="/blog">Back to blog</a>
      </section>
    )
  }

  return (
    <article aria-labelledby="post-title">
      <h1 id="post-title">{post.title}</h1>
      <p className="post-date">
        <time dateTime={post.date}>
          {new Date(post.date).toLocaleDateString('en-US', {
            year: 'numeric',
            month: 'long',
            day: 'numeric',
          })}
        </time>
      </p>
      <p>{post.content}</p>
      <a href="/blog">Back to all posts</a>
    </article>
  )
}
