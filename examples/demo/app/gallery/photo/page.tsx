import { Link } from '@ruvyxa/react'

export const meta = {
  title: 'Photo',
  description: 'The real page an interception stands in for.',
}

export default function Photo() {
  return (
    <main className="page">
      <p className="eyebrow">Intercepted route, rendered directly</p>
      <h1>Photo</h1>
      <p>
        This is <code>app/gallery/photo/page.tsx</code>. An interception is an overlay, never a
        replacement: the URL still has to render something on its own.
      </p>
      <p>
        <Link href="/gallery">Back to the gallery</Link>
      </p>
    </main>
  )
}
