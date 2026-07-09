/**
 * SSG Blog Index — lists all blog posts.
 * This is a static page (no dynamic segments) → automatically SSG.
 */

export default function BlogIndex() {
  const posts = [
    { slug: "hello-world", title: "Hello World" },
    { slug: "rendering-strategies", title: "Rendering Strategies" },
    { slug: "performance", title: "Performance Tips" },
  ]

  return (
    <main className="page-wide">
      <h1>SSG Blog</h1>
      <p>
        All pages in this section are pre-rendered at build time using Static
        Site Generation. Dynamic routes use <code>getStaticParams</code>.
      </p>

      <ul>
        {posts.map(post => (
          <li key={post.slug}>
            <a href={`/ssg-blog/${post.slug}`}>{post.title}</a>
          </li>
        ))}
      </ul>

      <p className="badge">Strategy: SSG</p>
    </main>
  )
}
