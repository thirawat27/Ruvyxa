export default function BlogPost({ params }: { params: { slug: string } }) {
  return (
    <main className="page">
      <p className="eyebrow">Dynamic route segment</p>
      <h1>Blog: {params.slug}</h1>
      <p>This page is rendered from <code>app/blog/{'[slug]'}/page.tsx</code>.</p>
      <p>The <code>slug</code> parameter comes from the URL path:</p>
      <pre>params = {JSON.stringify(params, null, 2)}</pre>
      <a href="/blog">&larr; Back to blog</a>
    </main>
  )
}
