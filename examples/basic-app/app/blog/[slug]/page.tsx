export default function BlogPost({ params }: { params: { slug: string } }) {
  return (
    <main className="page">
      <p className="eyebrow">Dynamic route</p>
      <h1>Blog Post: {params.slug}</h1>
      <p>This route matches /blog/:slug and receives route params during SSR.</p>
    </main>
  )
}
