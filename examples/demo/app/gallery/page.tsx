import { Link } from '@ruvyxa/react'

export const meta = {
  title: 'Gallery',
  description: 'A route whose @modal slot intercepts /gallery/photo.',
}

export default function Gallery() {
  return (
    <main className="page">
      <p className="eyebrow">Intercepting routes</p>
      <h1>Gallery</h1>
      <p>
        Following the link below is a soft navigation, so <code>@modal/(.)photo</code> opens over
        this page and this page stays mounted. Opening <code>/gallery/photo</code> directly — a
        reload, a shared link, a new tab — renders the real page instead.
      </p>
      <p>
        <Link href="/gallery/photo">Open the photo</Link>
      </p>
    </main>
  )
}
