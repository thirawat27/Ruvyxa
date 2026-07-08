export default function BlogIndex() {
  return (
    <main className="page">
      <p className="eyebrow">Route listing</p>
      <h1>Blog</h1>
      <p>Dynamic routes use the <code>param</code> folder syntax. Try these:</p>
      <ul>
        <li><a href="/blog/hello-world">/blog/hello-world</a></li>
        <li><a href="/blog/ruvyxa-v1">/blog/ruvyxa-v1</a></li>
        <li><a href="/blog/deep-dive">/blog/deep-dive</a></li>
      </ul>
    </main>
  )
}
