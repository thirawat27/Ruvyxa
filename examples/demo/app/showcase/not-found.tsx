import { Link } from '@ruvyxa/react'

export default function ShowcaseNotFound() {
  return (
    <main className="page">
      <p className="eyebrow">404</p>
      <h1>Item not found</h1>
      <p>No showcase item matches that name.</p>
      <p>
        <Link href="/showcase">Back to the showcase</Link>
      </p>
    </main>
  )
}
